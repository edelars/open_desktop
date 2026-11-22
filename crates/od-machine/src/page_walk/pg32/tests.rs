//! Tests of the 32-bit paging walk (S2-25 remainder). There is no hardware oracle for it (the Oracle
//! machine's guests run in long mode): the walk is checked against the od_isa model
//! (translate32_with, S2-25 and S2-25j) on the same tables, and by hand against the SDM (Vol. 3A
//! section 5.3, Tables 5-2 to 5-5 and section 5.7, Rev. 093 numbering).

use super::*;
use crate::ap_legacy_exec::BoardRam;
use crate::{MachineMemoryMap, MemoryRegionSpec, RamLimits};
use od_isa::legacy_mode::ProtectedSegment;
use od_isa::memory::{AccessOrigin, InstructionMode};
use od_isa::paging::{self as isa, PageRefusal, PageSize, Paging32Params, WalkFeatures};
use od_isa::real_mode_exec::TableRegister;

const P: u64 = 1;
const RW: u64 = 1 << 1;
const US: u64 = 1 << 2;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
const PS: u64 = 1 << 7;

const PD: u64 = 0x2000;
const PT: u64 = 0x3000;
/// The page table of PD[2] in the rights test.
const PT2: u64 = 0x4000;
const IMAGE: usize = 0x5000;
const RAM_END: u64 = 0x2_0000;
/// An address no region of the test memory covers.
const HOLE: u64 = 0x4_0000;

fn at(address: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(address)
}

fn range(start: u64, len: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(at(start), len).unwrap()
}

/// RAM below 0x2_0000 and a ROM page above it.
fn memory() -> PhysicalMemory {
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

/// Writes the 4-byte entry at physical address address (4-aligned) of image.
fn put(image: &mut [u8], address: u64, value: u64) {
    image[address as usize..address as usize + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

fn entry(memory: &PhysicalMemory, address: u64) -> u64 {
    let mut bytes = [0u8; 4];
    memory.load_bytes(at(address), &mut bytes).unwrap();
    u64::from(u32::from_le_bytes(bytes))
}

/// PD[0] -> PT (P, R/W, U/S); PD[1] = 4 MiB page at 0x40_0000 (read as a PT pointer without PSE: it
/// has PS and a frame, no PT address); PT[i] = page 0x10 + i (physical 0x1_0000 + 0x1000 * i), every
/// entry P, R/W and U/S. PD[1] lies in the high dword of its 8-byte cell, PT[1] and PT[3] too.
fn tables() -> Vec<u8> {
    let mut image = vec![0u8; IMAGE];
    put(&mut image, PD, PT | P | RW | US);
    put(&mut image, PD + 4, 0x40_0000 | PS | P | RW | US);
    for page in 0..8u64 {
        put(&mut image, PT + 4 * page, (0x10 + page) << 12 | P | RW | US);
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

fn config(wp: bool, pse: bool, pse36: bool) -> Pg32ProbeConfig {
    let cr0 = 0x8000_0011 | if wp { CR0_WP } else { 0 };
    Pg32ProbeConfig::new(cr0, PD, if pse { CR4_PSE } else { 0 }, 0, pse36).unwrap()
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
    config: &Pg32ProbeConfig,
    linear: u32,
    privilege: PrivilegeLevel,
    access: AccessKind,
) -> Result<Pg32Translation, TrackedAccessError> {
    translate_pg32_ram(
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

// ---- configuration -------------------------------------------------------------------------------

#[test]
fn the_configuration_checks_the_32_bit_paging_register_state() {
    let ok = |cr0, cr3, cr4, efer| Pg32ProbeConfig::new(cr0, cr3, cr4, efer, false);
    let good = ok(0x8000_0011, PD, 0, 0).unwrap();
    assert_eq!(
        (
            good.cr0(),
            good.cr3(),
            good.cr4(),
            good.efer(),
            good.pse36()
        ),
        (0x8000_0011, PD, 0, 0, false)
    );
    // PSE, PGE, SCE and NXE (inert without PAE) are snapshot bits; CR3 low bits are kept as given.
    assert!(
        ok(
            0x8001_0011,
            PD | 0x18,
            CR4_PSE | CR4_PGE,
            EFER_SCE | EFER_NXE
        )
        .is_ok()
    );
    // PAE is PAE paging, not this one; paging off, no PE and no ET are refused.
    assert_eq!(
        ok(0x8000_0011, PD, CR4_PAE, 0),
        Err(ProbeConfigError::UnsupportedBits {
            register: "CR4",
            value: CR4_PAE,
            offending_bits: CR4_PAE
        })
    );
    assert!(matches!(
        ok(0x0000_0011, PD, 0, 0),
        Err(ProbeConfigError::RequiredBits {
            register: "CR0",
            ..
        })
    ));
    assert!(matches!(
        ok(0x8000_0010, PD, 0, 0),
        Err(ProbeConfigError::RequiredBits {
            register: "CR0",
            ..
        })
    ));
    assert_eq!(
        ok(0x8000_0001, PD, 0, 0),
        Err(ProbeConfigError::Cr0EtRequired { value: 0x8000_0001 })
    );
    // IA-32e mode bits are refused, CR3 is 32 bits, no bit outside the allowed masks.
    assert!(ok(0x8000_0011, PD, 0, EFER_LME).is_err());
    assert!(ok(0x8000_0011, PD, 0, EFER_LMA).is_err());
    assert!(matches!(
        ok(0x8000_0011, 1 << 32 | PD, 0, 0),
        Err(ProbeConfigError::InvalidCr3 { .. })
    ));
    assert!(
        ok(0x8000_0011, PD, 1 << 20, 0).is_err(),
        "SMEP is not allowed"
    );
}

#[test]
fn a_protected_mode_model_on_32_bit_paging_is_a_32_bit_snapshot() {
    let mut cpu = od_isa::protected_exec::ProtectedCpu {
        gpr: [0; 8],
        eip: 0,
        rip_high: 0,
        target_entered: false,
        eflags: 2,
        cr0: 0x8001_0011,
        cr3: PD as u32,
        cr4: CR4_PSE as u32,
        efer: 0,
        pdptes: [0; 4],
        pse36: true,
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
    assert!(cpu.paging32());
    let config = Pg32ProbeConfig::from_protected_cpu(&cpu).unwrap();
    assert_eq!(
        (config.cr0(), config.cr3(), config.cr4()),
        (0x8001_0011, PD, CR4_PSE)
    );
    assert!(config.pse36(), "the model switch is the model's own");
    // Without paging, or with PAE (PAE paging), it is not 32-bit paging.
    cpu.cr0 = 0x0000_0011;
    assert!(Pg32ProbeConfig::from_protected_cpu(&cpu).is_err());
    cpu.cr0 = 0x8000_0011;
    cpu.cr4 = 0x20;
    assert!(Pg32ProbeConfig::from_protected_cpu(&cpu).is_err());
}

// ---- 4 KiB and 4 MiB translations --------------------------------------------------------------

#[test]
fn a_4k_translation_sets_accessed_in_the_pde_and_pte_and_dirty_in_the_pte_of_a_write() {
    let memory = memory();
    load(&memory, &tables());
    let config = config(false, false, false);
    let mut log = Vec::new();
    // PT[1] lies in the high dword of its cell, PD[0] in the low one.
    let read = translate_pg32_ram(
        &memory,
        &config,
        0x1234,
        0x1abc,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Read,
        |written| log.push(written),
    )
    .unwrap();
    assert_eq!(read.physical, at(0x1_1abc));
    assert_eq!(read.page_size, Pg32PageSize::Size4K);
    assert_eq!(read.leaf_entry_address, at(PT + 4));
    assert_eq!(read.leaf_entry_value, 0x11000 | P | RW | US | ACCESSED);
    assert_eq!(read.table_pages, [None, None, Some(at(PD)), Some(at(PT))]);
    assert_eq!(entry(&memory, PD), PT | P | RW | US | ACCESSED);
    assert_eq!(entry(&memory, PT + 4), 0x11000 | P | RW | US | ACCESSED);
    assert_eq!(
        entry(&memory, PT),
        0x10000 | P | RW | US,
        "the other dword is untouched"
    );
    assert_eq!(read.ad_updates.len(), 2);
    assert_eq!(read.ad_updates[0].address, at(PD));
    assert_eq!(read.ad_updates[0].level, PageTableLevel::Pd);
    assert_eq!(read.ad_updates[1].address, at(PT + 4));
    assert_eq!(read.ad_updates[1].bit, AdBit::Accessed);
    // The commits report the 8-byte cell of the entry.
    assert_eq!(log, [range(PD, 8), range(PT, 8)]);
    // A write sets D in the PTE only; the PDE has nothing left to set, so one write.
    log.clear();
    let write = translate_pg32_ram(
        &memory,
        &config,
        0x1234,
        0x1abc,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Write,
        |written| log.push(written),
    )
    .unwrap();
    assert_eq!(
        entry(&memory, PT + 4),
        0x11000 | P | RW | US | ACCESSED | DIRTY
    );
    assert_eq!(
        entry(&memory, PD),
        PT | P | RW | US | ACCESSED,
        "no D in a PDE to a table"
    );
    assert_eq!(write.ad_updates.len(), 1);
    assert_eq!(write.ad_updates[0].bit, AdBit::Dirty);
    assert_eq!(log, [range(PT, 8)]);
    // Nothing is left to set: no write, no callback.
    translate_pg32_ram(
        &memory,
        &config,
        0x1234,
        0x1abc,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Write,
        |_| panic!("no write"),
    )
    .unwrap();
    // A fetch is a read for the bits.
    let fetch = walk(
        &memory,
        &config,
        0x2abc,
        PrivilegeLevel::Ring0,
        AccessKind::Fetch,
    )
    .unwrap();
    assert_eq!(fetch.leaf_entry_address, at(PT + 8));
    assert_eq!(entry(&memory, PT + 8), 0x12000 | P | RW | US | ACCESSED);
}

#[test]
fn a_4m_translation_ends_at_the_pde_with_the_22_bit_offset_when_pse_is_set() {
    let memory = memory();
    load(&memory, &tables());
    let config = config(false, true, false);
    // PD[1] (the high dword of its cell): the page at 0x40_0000.
    let read = walk(
        &memory,
        &config,
        0x7F_FFFF,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(read.physical, at(0x7F_FFFF));
    assert_eq!(read.page_size, Pg32PageSize::Size4M);
    assert_eq!(read.leaf_entry_address, at(PD + 4));
    assert_eq!(read.table_pages, [None, None, Some(at(PD)), None]);
    assert_eq!(read.ad_updates.len(), 1);
    assert_eq!(
        entry(&memory, PD + 4),
        0x40_0000 | PS | P | RW | US | ACCESSED
    );
    assert_eq!(
        entry(&memory, PD),
        PT | P | RW | US,
        "the PDE next to it is untouched"
    );
    // A write sets A and D of the same PDE: D lives in the PDE of a 4 MiB page.
    let write = walk(
        &memory,
        &config,
        0x40_0010,
        PrivilegeLevel::Ring0,
        AccessKind::Write,
    )
    .unwrap();
    assert_eq!(write.physical, at(0x40_0010));
    assert_eq!(
        entry(&memory, PD + 4),
        0x40_0000 | PS | P | RW | US | ACCESSED | DIRTY
    );
    assert_eq!(write.ad_updates.len(), 1);
    assert_eq!(write.ad_updates[0].bit, AdBit::Dirty);
    // A fresh table: the first write sets both bits at once (accessed in the walk, dirty after).
    load(&memory, &tables());
    let both = walk(
        &memory,
        &config,
        0x40_0010,
        PrivilegeLevel::Ring0,
        AccessKind::Write,
    )
    .unwrap();
    assert_eq!(both.ad_updates.len(), 2);
    assert_eq!(
        entry(&memory, PD + 4),
        0x40_0000 | PS | P | RW | US | ACCESSED | DIRTY
    );
}

#[test]
fn without_pse_the_ps_bit_is_ignored_and_no_bit_of_a_pde_is_reserved() {
    let memory = memory();
    let mut image = tables();
    // A PDE with PS and bits 16:13 set (reserved in a 4 MiB PDE): with PSE = 0 they are bits of the
    // page-table address, here 0x1_F000 (RAM, all zeros), so the walk goes on to a PTE that is not present.
    put(&mut image, PD, (PT | 0x1_E000) | PS | P | RW | US);
    load(&memory, &image);
    let walked = walk(
        &memory,
        &config(false, false, false),
        0x1abc,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    );
    let (reason, code, cr2) = fault(walked.unwrap_err());
    assert_eq!(
        reason,
        PageFaultReason::NotPresent {
            level: PageTableLevel::Pt
        }
    );
    assert_eq!((code, cr2), (0, 0x1abc));
    // The same PDE with a PT address and PS only: a 4 KiB walk.
    put(&mut image, PD, PT | PS | P | RW | US);
    load(&memory, &image);
    let ok = walk(
        &memory,
        &config(false, false, false),
        0x1abc,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(ok.page_size, Pg32PageSize::Size4K);
    assert_eq!(ok.physical, at(0x1_1abc));
    // With PSE the same entry is a 4 MiB PDE whose frame bits 21:13 (PT bits 13 and 12 are set in
    // 0x3000) are reserved or PAT: bit 13 is reserved.
    load(&memory, &image);
    let (reason, code, _) = fault(
        walk(
            &memory,
            &config(false, true, false),
            0x1abc,
            PrivilegeLevel::Ring0,
            AccessKind::Read,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        PageFaultReason::ReservedBits {
            level: PageTableLevel::Pd,
            offending_bits: 1 << 13
        }
    );
    assert_eq!(code, 0b1001, "P and RSVD, a read at CPL 0");
}

#[test]
fn bits_21_to_13_of_a_4m_pde_are_reserved_and_bit_12_is_the_pat_bit() {
    let memory = memory();
    let config = config(false, true, false);
    for bit in 13..=21u32 {
        let mut image = tables();
        put(&mut image, PD, 0x80_0000 | PS | P | RW | US | 1 << bit);
        load(&memory, &image);
        let (reason, code, cr2) = fault(
            walk(
                &memory,
                &config,
                0x1234,
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
        assert_eq!((code, cr2), (0b1001, 0x1234));
        // A reserved-bit fault happens before the accessed bit is set.
        assert_eq!(entry(&memory, PD) & ACCESSED, 0, "bit {bit}");
    }
    // Bit 12 (PAT) and bits 11:9 and G are ignored by the walk, not address bits of a 4 MiB page.
    let mut image = tables();
    put(
        &mut image,
        PD,
        0x80_0000 | PS | P | RW | US | 1 << 12 | 1 << 8 | 7 << 9,
    );
    load(&memory, &image);
    let ok = walk(
        &memory,
        &config,
        0x1234,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(ok.physical, at(0x80_1234));
    // Only the frame bits 31:22 give the address.
    put(&mut image, PD, 0xFFC0_0000 | PS | P | RW | US);
    load(&memory, &image);
    let top = walk(
        &memory,
        &config,
        0x3F_FFFF,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(top.physical, at(0xFFFF_FFFF));
}

#[test]
fn pse36_takes_the_frame_bits_35_to_32_from_pde_bits_16_to_13_and_reserves_21_to_17() {
    let memory = memory();
    let config36 = config(false, true, true);
    let plain = config(false, true, false);
    for bit in 17..=21u32 {
        let mut image = tables();
        put(&mut image, PD, 0x40_0000 | PS | P | RW | US | 1 << bit);
        load(&memory, &image);
        let (reason, _, _) = fault(
            walk(
                &memory,
                &config36,
                0x10,
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
    // Bits 16:13 are address bits 35:32 with the switch: 4 GiB + 0x40_0000 + the offset.
    let mut image = tables();
    put(&mut image, PD, 0x40_0000 | PS | P | RW | US | 0x1 << 13);
    put(&mut image, PD + 4, 0xC0_0000 | PS | P | RW | US | 0xF << 13);
    load(&memory, &image);
    let high = walk(
        &memory,
        &config36,
        0x12_3456,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(high.physical, at(0x1_0000_0000 + 0x40_0000 + 0x12_3456));
    let top = walk(
        &memory,
        &config36,
        0x40_0000 + 0x10,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(top.physical, at(0xF_0000_0000 + 0xC0_0000 + 0x10));
    // Without the switch the same bits are reserved.
    let (reason, _, _) = fault(
        walk(
            &memory,
            &plain,
            0x10,
            PrivilegeLevel::Ring0,
            AccessKind::Read,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        PageFaultReason::ReservedBits {
            level: PageTableLevel::Pd,
            offending_bits: 1 << 13
        }
    );
    // The switch changes nothing for a PDE to a table or with PSE = 0: bits 16:13 are address bits of
    // the table (here 0x1_F000, all zeros: the PTE is not present, no reserved-bit fault).
    let mut image = tables();
    put(&mut image, PD, (PT | 0x1_E000) | P | RW | US);
    load(&memory, &image);
    for config in [config(false, false, true), config(false, true, true)] {
        let (reason, _, _) = fault(
            walk(
                &memory,
                &config,
                0x1abc,
                PrivilegeLevel::Ring0,
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
    }
}

// ---- not present, reserved bits and rights -----------------------------------------------------

#[test]
fn not_present_pde_and_pte_fault_with_the_p_bit_clear_in_the_error_code() {
    let memory = memory();
    let config = config(false, false, false);
    let mut image = tables();
    put(&mut image, PD + 8, 0); // PD[2], linear 0x80_0000
    put(&mut image, PT + 8, 0x12000 | RW | US); // PTE 2: P clear, other bits set
    load(&memory, &image);
    for (access, privilege, code) in [
        (AccessKind::Read, PrivilegeLevel::Ring0, 0),
        (AccessKind::Write, PrivilegeLevel::Ring0, 2),
        (AccessKind::Read, PrivilegeLevel::Ring3, 4),
        (AccessKind::Write, PrivilegeLevel::Ring3, 6),
        // 32-bit paging has no I/D bit (no XD, SMEP not modelled): a fetch is a read.
        (AccessKind::Fetch, PrivilegeLevel::Ring0, 0),
        (AccessKind::Fetch, PrivilegeLevel::Ring3, 4),
    ] {
        let (reason, got, cr2) =
            fault(walk(&memory, &config, 0x80_0abc, privilege, access).unwrap_err());
        assert_eq!(
            reason,
            PageFaultReason::NotPresent {
                level: PageTableLevel::Pd
            }
        );
        assert_eq!((got, cr2), (code, 0x80_0abc), "{access:?} {privilege:?}");
        let (reason, got, cr2) =
            fault(walk(&memory, &config, 0x2abc, privilege, access).unwrap_err());
        assert_eq!(
            reason,
            PageFaultReason::NotPresent {
                level: PageTableLevel::Pt
            }
        );
        assert_eq!((got, cr2), (code, 0x2abc), "{access:?} {privilege:?}");
    }
    // The accessed bit of the PDE was set on the way to the PTE; the PTE (P = 0) is left alone.
    assert_eq!(entry(&memory, PD) & ACCESSED, ACCESSED);
    assert_eq!(entry(&memory, PT + 8), 0x12000 | RW | US);
}

#[test]
fn write_protection_and_the_user_bit_follow_cr0_wp_and_the_cpl() {
    let memory = memory();
    let mut image = tables();
    // PTE 0: supervisor, read-only; PTE 1: user, read-only; PTE 2: user, writable.
    put(&mut image, PT, 0x10000 | P);
    put(&mut image, PT + 4, 0x11000 | P | US);
    put(&mut image, PT + 8, 0x12000 | P | RW | US);
    // PD[2] -> PT2 is a read-only user PDE over a writable user PTE.
    put(&mut image, PD + 8, PT2 | P | US);
    put(&mut image, PT2, 0x13000 | P | RW | US);
    load(&memory, &image);
    let ring0 = PrivilegeLevel::Ring0;
    let ring3 = PrivilegeLevel::Ring3;
    for wp in [false, true] {
        let config = config(wp, false, false);
        // Supervisor write to a read-only page: allowed without WP, refused with it.
        load(&memory, &image);
        let r = walk(&memory, &config, 0x10, ring0, AccessKind::Write);
        if wp {
            let (reason, code, _) = fault(r.unwrap_err());
            assert_eq!(
                reason,
                PageFaultReason::Protection {
                    user_denied: false,
                    write_denied: true,
                    execute_denied: false
                }
            );
            assert_eq!(code, 3, "P and W/R: a supervisor write with WP");
        } else {
            r.unwrap();
        }
        // CPL 3: a supervisor-only page is a user fault whatever WP is: read 5, write 7.
        for (access, code) in [
            (AccessKind::Read, 5),
            (AccessKind::Write, 7),
            (AccessKind::Fetch, 5),
        ] {
            load(&memory, &image);
            let (reason, got, _) = fault(walk(&memory, &config, 0x10, ring3, access).unwrap_err());
            assert_eq!(
                reason,
                PageFaultReason::Protection {
                    user_denied: true,
                    write_denied: access == AccessKind::Write,
                    execute_denied: false
                }
            );
            assert_eq!(got, code, "{access:?} wp {wp}");
        }
        // A user read-only page: CPL 3 reads, never writes; the supervisor reads, and writes only
        // without WP.
        load(&memory, &image);
        walk(&memory, &config, 0x1010, ring3, AccessKind::Read).unwrap();
        let (reason, code, _) =
            fault(walk(&memory, &config, 0x1010, ring3, AccessKind::Write).unwrap_err());
        assert_eq!(
            reason,
            PageFaultReason::Protection {
                user_denied: false,
                write_denied: true,
                execute_denied: false
            }
        );
        assert_eq!(code, 7);
        assert_eq!(
            walk(&memory, &config, 0x1010, ring0, AccessKind::Write).is_ok(),
            !wp
        );
        // A writable user page: everything goes.
        load(&memory, &image);
        for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
            walk(&memory, &config, 0x2010, ring3, access).unwrap();
        }
        // The rights are the AND of the PDE and the PTE: PDE read-only, PTE writable.
        load(&memory, &image);
        walk(&memory, &config, 0x80_0010, ring3, AccessKind::Read).unwrap();
        let r = walk(&memory, &config, 0x80_0010, ring0, AccessKind::Write);
        assert_eq!(r.is_ok(), !wp, "wp {wp}");
        let (reason, code, _) =
            fault(walk(&memory, &config, 0x80_0010, ring3, AccessKind::Write).unwrap_err());
        assert_eq!(
            reason,
            PageFaultReason::Protection {
                user_denied: false,
                write_denied: true,
                execute_denied: false
            }
        );
        assert_eq!(code, 7);
    }
    // A 4 MiB page has the rights of its PDE.
    let mut image = tables();
    put(&mut image, PD + 4, 0x40_0000 | PS | P | US);
    load(&memory, &image);
    let config = config(true, true, false);
    walk(&memory, &config, 0x40_0000, ring3, AccessKind::Read).unwrap();
    assert!(walk(&memory, &config, 0x40_0000, ring0, AccessKind::Write).is_err());
}

#[test]
fn a_refused_write_has_set_the_accessed_bits_it_passed_and_never_the_dirty_bit() {
    let memory = memory();
    let mut image = tables();
    put(&mut image, PT, 0x10000 | P | US);
    load(&memory, &image);
    let config = config(true, false, false);
    let error = walk(
        &memory,
        &config,
        0x10,
        PrivilegeLevel::Ring0,
        AccessKind::Write,
    )
    .unwrap_err();
    assert!(matches!(
        fault(error).0,
        PageFaultReason::Protection {
            write_denied: true,
            ..
        }
    ));
    // As the PAE and the 4-level walks do: A of the PDE and PTE are set when the walk reaches them.
    assert_eq!(entry(&memory, PD) & ACCESSED, ACCESSED);
    assert_eq!(entry(&memory, PT), 0x10000 | P | US | ACCESSED);
}

// ---- table access errors, read-only forms, the entry writer ------------------------------------

#[test]
fn the_read_only_forms_commit_nothing_and_a_bad_entry_read_or_write_is_a_typed_error() {
    let memory = memory();
    load(&memory, &tables());
    let config = config(false, false, false);
    let ro = translate_pg32_ram_read_only(
        &memory,
        &config,
        0x1234,
        0x2abc,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Write,
    )
    .unwrap();
    assert_eq!(dump(&memory), tables(), "no write");
    assert_eq!(
        ro.leaf_entry_value,
        0x12000 | P | RW | US | ACCESSED,
        "A is in the value only"
    );
    assert!(ro.ad_updates.is_empty());
    let (traced, trace) = translate_pg32_ram_read_only_with_trace(
        &memory,
        &config,
        0x1234,
        0x2abc,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Read,
    );
    assert!(traced.is_ok());
    assert_eq!(trace.len(), 2);
    let pt_entry = trace.entries()[1].unwrap();
    assert_eq!(
        (
            pt_entry.level,
            pt_entry.index,
            pt_entry.entry_address,
            pt_entry.entry_value
        ),
        (PageTableLevel::Pt, 2, at(PT + 8), 0x12000 | P | RW | US)
    );
    assert_eq!(trace.entries()[0].unwrap().index, 0);
    // The PD at an address no region covers: a typed table read error, level Pd, no fault.
    let hole = Pg32ProbeConfig::new(0x8000_0011, HOLE, 0, 0, false).unwrap();
    assert!(matches!(
        walk(&memory, &hole, 0, PrivilegeLevel::Ring0, AccessKind::Read),
        Err(TrackedAccessError::TableRead { level: PageTableLevel::Pd, entry_address, .. })
            if entry_address == at(HOLE)
    ));
    // The page directory at the very last cell of a hole-free page: index 1023 is the last dword.
    let mut image = tables();
    put(&mut image, PD + 4 * 1023, PT | P | RW | US);
    load(&memory, &image);
    let last = walk(
        &memory,
        &config,
        0xFFC0_1ABC,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(last.physical, at(0x1_1abc));
    assert_eq!(last.leaf_entry_address, at(PT + 4));
}

#[test]
fn the_entry_writer_replaces_one_dword_of_the_cell_and_reports_a_changed_entry() {
    let memory = memory();
    let cell = 0x1000u64;
    memory
        .store_bytes(at(cell), &0xAAAA_BBBB_1111_2222u64.to_le_bytes(), |_| {})
        .unwrap();
    let mut log = Vec::new();
    // Low dword: replaced, the high one is carried over.
    assert_eq!(
        compare_replace_entry32(&memory, at(cell), 0x1111_2222, 0x1111_2223, &mut |w| log
            .push(w)),
        Ok(TableReplace::Replaced)
    );
    assert_eq!(entry(&memory, cell), 0x1111_2223);
    assert_eq!(entry(&memory, cell + 4), 0xAAAA_BBBB);
    // High dword.
    assert_eq!(
        compare_replace_entry32(&memory, at(cell + 4), 0xAAAA_BBBB, 0xAAAA_BBBF, &mut |w| {
            log.push(w)
        }),
        Ok(TableReplace::Replaced)
    );
    assert_eq!(entry(&memory, cell), 0x1111_2223);
    assert_eq!(entry(&memory, cell + 4), 0xAAAA_BBBF);
    assert_eq!(log, [range(cell, 8), range(cell, 8)]);
    // An entry that is not what the caller read: nothing is written and the value is reported.
    assert_eq!(
        compare_replace_entry32(&memory, at(cell + 4), 0x1, 0x2, &mut |_| panic!("no write")),
        Ok(TableReplace::Changed(0xAAAA_BBBF))
    );
    assert_eq!(
        compare_replace_entry32(&memory, at(cell), 0xAAAA_BBBF, 0x2, &mut |_| panic!(
            "no write"
        )),
        Ok(TableReplace::Changed(0x1111_2223))
    );
    // ROM: a typed error.
    assert!(compare_replace_entry32(&memory, at(RAM_END), 0, 1, &mut |_| {}).is_err());
}

// ---- differential against the od_isa model ----------------------------------------------------

#[derive(Debug, PartialEq)]
enum Outcome {
    Mapped {
        physical: u64,
        large: bool,
    },
    NotPresent(u8),
    Reserved(u8),
    WriteProtected,
    /// A table entry outside the memory map (a 4 MiB PDE read as a table pointer, with PSE = 0).
    Unreadable,
}

fn od_isa_outcome(
    ram: &BoardRam<'_>,
    params: Paging32Params,
    features: WalkFeatures,
    linear: u32,
    kind: AccessKind,
) -> (Outcome, Vec<isa::Touch>) {
    let isa_kind = match kind {
        AccessKind::Read => isa::AccessKind::Read,
        AccessKind::Write => isa::AccessKind::Write,
        AccessKind::Fetch => isa::AccessKind::Fetch,
    };
    match isa::translate32_with(ram, params, features, linear, isa_kind) {
        Ok(t) => (
            Outcome::Mapped {
                physical: u64::from(t.phys),
                large: t.size == PageSize::Size4M,
            },
            t.touches,
        ),
        // The model addresses memory with 32 bits; the machine walk has no such limit.
        Err(PageRefusal::PhysicalBeyond4GiB { phys }) => (
            Outcome::Mapped {
                physical: phys,
                large: true,
            },
            Vec::new(),
        ),
        Err(PageRefusal::NotPresent { level, .. }) => (Outcome::NotPresent(level), Vec::new()),
        Err(PageRefusal::ReservedBits { level, .. }) => (Outcome::Reserved(level), Vec::new()),
        Err(PageRefusal::WriteProtected { .. }) => (Outcome::WriteProtected, Vec::new()),
        Err(PageRefusal::EntryUnreadable { .. }) => (Outcome::Unreadable, Vec::new()),
        Err(other) => panic!("unexpected od_isa refusal {other:?}"),
    }
}

fn od_machine_outcome(
    memory: &PhysicalMemory,
    config: &Pg32ProbeConfig,
    linear: u32,
    kind: AccessKind,
) -> Outcome {
    match walk(memory, config, linear, PrivilegeLevel::Ring0, kind) {
        Ok(t) => Outcome::Mapped {
            physical: t.physical.get(),
            large: t.page_size == Pg32PageSize::Size4M,
        },
        Err(TrackedAccessError::TableRead { .. }) => Outcome::Unreadable,
        Err(error) => match fault(error).0 {
            PageFaultReason::NotPresent { level } => Outcome::NotPresent(match level {
                PageTableLevel::Pd => 2,
                PageTableLevel::Pt => 1,
                other => panic!("32-bit paging has no {other:?}"),
            }),
            PageFaultReason::ReservedBits { level, .. } => Outcome::Reserved(match level {
                PageTableLevel::Pd => 2,
                other => panic!("reserved bits at {other:?}"),
            }),
            PageFaultReason::Protection {
                write_denied: true,
                user_denied: false,
                execute_denied: false,
            } => Outcome::WriteProtected,
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

/// A random PDE/PTE: mostly present, random R/W, U/S, A, D, G and ignored bits, now and then, for a
/// 4 MiB PDE, a reserved bit (13..=21), the PAT bit or PSE-36 address bits 16:13.
fn random_entry(state: &mut u64, address: u64, large: bool) -> u64 {
    let r = next(state);
    let mut entry = address;
    if r & 7 != 0 {
        entry |= P;
    }
    for (shift, bit) in [(3, RW), (4, US), (5, ACCESSED), (6, DIRTY), (9, 1 << 8)] {
        if (r >> shift) & 1 != 0 {
            entry |= bit;
        }
    }
    entry |= ((r >> 11) & 7) << 9; // ignored bits 11:9
    if large {
        entry |= PS;
        let r = next(state);
        if r.is_multiple_of(9) {
            entry |= 1 << (13 + next(state) % 9);
        }
        if r % 3 == 1 {
            entry |= (next(state) & 0xF) << 13; // PSE-36 address bits 35:32
        }
        if r % 4 == 2 {
            entry |= 1 << 12; // PAT: not an address bit of a 4 MiB page
        }
    } else if next(state).is_multiple_of(4) {
        entry |= PS; // PS in a PDE to a table: ignored with PSE = 0
    }
    entry
}

#[test]
fn the_od_machine_walk_and_the_od_isa_walk_agree_on_random_tables() {
    let machine = memory();
    let isa_memory = memory();
    let mut state = 0x5EED_2032_u64;
    let mut counts = [0usize; 4];
    let mut unreadable = 0;
    let (mut mapped_large, mut beyond, mut mapped_pse36) = (0, 0, 0);
    for _ in 0..400 {
        let mut image = tables();
        let pde0_large = next(&mut state).is_multiple_of(3);
        let pde1_large = next(&mut state).is_multiple_of(2);
        let pde0_address = if pde0_large {
            (next(&mut state) & 0x3FF) << 22
        } else {
            PT
        };
        let pde0 = random_entry(&mut state, pde0_address, pde0_large);
        let pde1_address = if pde1_large {
            (next(&mut state) & 0x3FF) << 22
        } else {
            PT
        };
        let pde1 = random_entry(&mut state, pde1_address, pde1_large);
        put(&mut image, PD, pde0);
        put(&mut image, PD + 4, pde1);
        for page in 1..3u64 {
            let address = (next(&mut state) & 0xF_FFFF) << 12;
            // Bit 7 of a PTE is the PAT bit: no meaning for the walk.
            let pte = random_entry(&mut state, address, false);
            put(&mut image, PT + 4 * page, pte);
        }
        for (wp, pse, pse36) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, true, true),
            (true, true, true),
            (false, false, true),
            (true, false, true),
        ] {
            let config = config(wp, pse, pse36);
            let params = Paging32Params {
                cr3: PD as u32,
                wp,
                pse,
            };
            let features = WalkFeatures { smep: false, pse36 };
            for linear in [0x1abcu32, 0x2abc, 0x40_1234, 0x40_0000, 0x7F_FFFF] {
                for kind in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
                    load(&machine, &image);
                    load(&isa_memory, &image);
                    let mut ram = BoardRam::new(&isa_memory);
                    let (expected, touches) = od_isa_outcome(&ram, params, features, linear, kind);
                    let got = od_machine_outcome(&machine, &config, linear, kind);
                    assert_eq!(
                        got, expected,
                        "linear {linear:#x} {kind:?} wp {wp} pse {pse} pse36 {pse36} pde0 {pde0:#x} pde1 {pde1:#x}"
                    );
                    match &expected {
                        Outcome::Mapped { large, physical } => {
                            counts[0] += 1;
                            mapped_large += usize::from(*large);
                            mapped_pse36 += usize::from(*large && *physical >> 32 != 0);
                            if touches.is_empty() && *physical >> 32 != 0 {
                                // A frame above 4 GiB: od_isa refuses (a model limit) and sets nothing.
                                beyond += 1;
                            } else {
                                // The accessed/dirty updates leave the same table bytes. (On a refused
                                // walk od_machine has already set the accessed bits it passed, od_isa
                                // none: not compared.)
                                assert_eq!(isa::commit_touches(&mut ram, &touches), 0);
                                assert_eq!(
                                    dump(&machine),
                                    dump(&isa_memory),
                                    "tables after {linear:#x} {kind:?}"
                                );
                            }
                        }
                        Outcome::NotPresent(_) => counts[1] += 1,
                        Outcome::Reserved(_) => counts[2] += 1,
                        Outcome::WriteProtected => counts[3] += 1,
                        Outcome::Unreadable => unreadable += 1,
                    }
                }
            }
        }
    }
    // The sweep reaches every outcome, 4 MiB pages and frames above 4 GiB included.
    assert!(counts.iter().all(|&n| n > 100), "{counts:?}");
    assert!(
        mapped_large > 100 && unreadable > 0,
        "{mapped_large} {unreadable}"
    );
    assert!(
        beyond > 20 && mapped_pse36 >= beyond,
        "{beyond} {mapped_pse36}"
    );
}

#[test]
fn the_read_only_walk_is_the_committing_walk_without_the_writes() {
    let memory = memory();
    let mut state = 91u64;
    for _ in 0..200 {
        let mut image = tables();
        let large = next(&mut state).is_multiple_of(2);
        let pde = random_entry(&mut state, if large { 0x80_0000 } else { PT }, large);
        put(&mut image, PD, pde);
        put(
            &mut image,
            PT + 4,
            random_entry(&mut state, 0x1_2000, false),
        );
        let config = config(true, true, next(&mut state).is_multiple_of(2));
        for kind in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
            load(&memory, &image);
            let read_only = translate_pg32_ram_read_only(
                &memory,
                &config,
                0x1234,
                0x1abc,
                ctx(PrivilegeLevel::Ring0),
                kind,
            );
            assert_eq!(dump(&memory), image, "no write by the read-only walk");
            let committing = walk(&memory, &config, 0x1abc, PrivilegeLevel::Ring0, kind);
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
