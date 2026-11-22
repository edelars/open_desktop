//! Tests of the operand access through 32-bit paging (S2-25 remainder): the tracked access of
//! [access_pg32_ram] and [access_pg32_with_mmio]. No hardware oracle exists for it; the rights and the
//! page-fault error codes are the SDM text (Vol. 3A section 5.3 and 5.7, Rev. 093), the order of the
//! effects is that of the 4-level and PAE tracked accesses of this crate.

use super::*;
use crate::{MachineMemoryMap, MemoryRegionSpec, RamLimits};
use od_isa::memory::{AccessOrigin, InstructionMode};

const P: u64 = 1;
const RW: u64 = 1 << 1;
const US: u64 = 1 << 2;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
const PS: u64 = 1 << 7;

const PD: u64 = 0x2000;
const PT: u64 = 0x3000;
const DATA: u64 = 0x1_0000;
const RAM_END: u64 = 0x2_0000;
const ROM_AT: u64 = 0x2_0000;
const MMIO_AT: u64 = 0x3_0000;
/// An address no region covers.
const HOLE: u64 = 0x4_0000;
/// RAM above 4 GiB, reached by a PSE-36 page (physical-address bit 32 is PDE bit 13).
const HIGH_AT: u64 = 0x1_0000_0000;

fn at(address: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(address)
}

fn range(start: u64, len: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(at(start), len).unwrap()
}

/// RAM below 0x2_0000, a ROM page, an MMIO page, a hole, and a RAM page above 4 GiB.
fn memory() -> PhysicalMemory {
    let regions = vec![
        MemoryRegionSpec::new(range(0, RAM_END), MemoryRegionKind::Ram),
        MemoryRegionSpec::new(range(ROM_AT, 0x1000), MemoryRegionKind::Rom),
        MemoryRegionSpec::new(range(MMIO_AT, 0x1000), MemoryRegionKind::Mmio),
        MemoryRegionSpec::new(range(HIGH_AT, 0x1000), MemoryRegionKind::Ram),
    ];
    PhysicalMemory::new(
        &MachineMemoryMap::new(36, regions).unwrap(),
        RamLimits::new(1 << 20),
    )
    .unwrap()
}

/// A 4-byte entry.
fn put(memory: &PhysicalMemory, address: u64, value: u64) {
    memory
        .store_bytes(at(address), &(value as u32).to_le_bytes(), |_| {})
        .unwrap();
}

fn get(memory: &PhysicalMemory, address: u64) -> u64 {
    let mut bytes = [0u8; 4];
    memory.load_bytes(at(address), &mut bytes).unwrap();
    u64::from(u32::from_le_bytes(bytes))
}

/// Linear page n (n < 8) -> PT[n]; PD[1] (0x40_0000) is a 4 MiB page at 0x40_0000 (outside the memory
/// map), PD[2] (0x80_0000) a PSE-36 4 MiB page at HIGH_AT, PD[3] (0xC0_0000) a 4 MiB page at 0.
fn tables(memory: &PhysicalMemory) {
    put(memory, PD, PT | P | RW | US);
    put(memory, PD + 4, 0x40_0000 | PS | P | RW | US);
    put(memory, PD + 8, PS | P | RW | US | 1 << 13);
    put(memory, PD + 12, PS | P | RW | US);
    let entries = [
        DATA | P | RW | US,
        MMIO_AT | P | RW | US,
        ROM_AT | P | RW | US,
        HOLE | P | RW | US,
        (DATA + 0x1000) | P | RW,
        (DATA + 0x2000) | P | US,
        (DATA + 0x3000) | P | RW | US,
        0,
    ];
    for (i, entry) in entries.into_iter().enumerate() {
        put(memory, PT + 4 * i as u64, entry);
    }
}

fn setup() -> (PhysicalMemory, Pg32ProbeConfig) {
    let memory = memory();
    tables(&memory);
    let config = Pg32ProbeConfig::new(0x8001_0011, PD, CR4_PSE, 0, true).unwrap();
    (memory, config)
}

fn request(
    operation: PaeOperandOperation,
    linear: u32,
    privilege: PrivilegeLevel,
) -> Pg32AccessRequest {
    Pg32AccessRequest {
        instruction_rip: 0x1234,
        linear,
        context: AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        },
        operation,
    }
}

const READ4: PaeOperandOperation = PaeOperandOperation::Read {
    width: AccessWidth::Dword,
};

fn write(width: AccessWidth, value: u64) -> PaeOperandOperation {
    PaeOperandOperation::Write { width, value }
}

fn page(n: u32) -> u32 {
    n * 0x1000
}

fn fault(report: &TrackedAccessReport) -> (PageFaultReason, u32, u64) {
    match report.outcome.clone() {
        Err(TrackedAccessError::PageFault { record, reason }) => (
            reason,
            record.fault().error_code.unwrap(),
            record.cr2_value(),
        ),
        other => panic!("expected a page fault, got {other:?}"),
    }
}

#[test]
fn a_read_sets_accessed_bits_of_the_pde_and_pte_never_dirty_and_returns_the_bytes() {
    let (memory, config) = setup();
    memory
        .store_bytes(at(DATA + 0x10), &0x1122_3344u32.to_le_bytes(), |_| {})
        .unwrap();
    let mut log = Vec::new();
    let report = access_pg32_ram(
        &memory,
        &config,
        request(READ4, page(0) + 0x10, PrivilegeLevel::Ring0),
        |written| log.push(written),
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Read(0x1122_3344)));
    assert_eq!(get(&memory, PD), PT | P | RW | US | ACCESSED);
    assert_eq!(get(&memory, PT), DATA | P | RW | US | ACCESSED);
    assert_eq!(report.ad_update_count, 2);
    assert_eq!(report.ad_updates[0].unwrap().address, at(PD));
    assert_eq!(report.ad_updates[1].unwrap().address, at(PT));
    // The two entries are the only writes: a read writes no operand byte.
    assert_eq!(log, [range(PD, 8), range(PT, 8)]);
    // A second read changes nothing.
    let again = access_pg32_ram(
        &memory,
        &config,
        request(READ4, page(0) + 0x10, PrivilegeLevel::Ring0),
        |_| panic!("no write"),
    );
    assert_eq!(again.ad_update_count, 0);
    // The PTE of page 1 lies in the high dword of the PT's first cell: only that dword changes.
    let high = access_pg32_ram(
        &memory,
        &config,
        request(READ4, page(1), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        high.outcome,
        Err(TrackedAccessError::OperandPhysical {
            address: at(MMIO_AT),
            source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Mmio)
        })
    );
    assert_eq!(get(&memory, PT + 4), MMIO_AT | P | RW | US | ACCESSED);
    assert_eq!(get(&memory, PT), DATA | P | RW | US | ACCESSED);
}

#[test]
fn a_write_commits_accessed_then_dirty_then_the_bytes_and_every_width_round_trips() {
    let (memory, config) = setup();
    let mut log = Vec::new();
    let report = access_pg32_ram(
        &memory,
        &config,
        request(
            write(AccessWidth::Dword, 0xDEAD_BEEF),
            page(0) + 0x20,
            PrivilegeLevel::Ring0,
        ),
        |written| log.push(written),
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(get(&memory, PD), PT | P | RW | US | ACCESSED);
    assert_eq!(get(&memory, PT), DATA | P | RW | US | ACCESSED | DIRTY);
    assert_eq!(get(&memory, DATA + 0x20), 0xDEAD_BEEF);
    assert_eq!(report.ad_update_count, 3);
    assert_eq!(report.ad_updates[2].unwrap().bit, AdBit::Dirty);
    assert_eq!(
        log,
        [
            range(PD, 8),
            range(PT, 8),
            range(PT, 8),
            range(DATA + 0x20, 4)
        ]
    );
    for (width, value, size) in [
        (AccessWidth::Byte, 0x5A_u64, 1),
        (AccessWidth::Word, 0xBEEF, 2),
        (AccessWidth::Dword, 0x1234_5678, 4),
        (AccessWidth::Qword, 0x0123_4567_89AB_CDEF, 8),
    ] {
        let linear = page(0) + 0x100;
        let stored = access_pg32_ram(
            &memory,
            &config,
            request(write(width, value), linear, PrivilegeLevel::Ring0),
            |_| {},
        );
        assert_eq!(stored.outcome, Ok(TrackedAccessOutcome::Written));
        let loaded = access_pg32_ram(
            &memory,
            &config,
            request(
                PaeOperandOperation::Read { width },
                linear,
                PrivilegeLevel::Ring0,
            ),
            |_| {},
        );
        assert_eq!(
            loaded.outcome,
            Ok(TrackedAccessOutcome::Read(value)),
            "{size}"
        );
    }
}

#[test]
fn a_write_through_a_4m_page_sets_dirty_in_the_pde() {
    let (memory, config) = setup();
    // PD[3]: 4 MiB page at 0; linear 0xC1_0010 is physical 0x1_0010 (RAM).
    let report = access_pg32_ram(
        &memory,
        &config,
        request(
            write(AccessWidth::Dword, 0x600D_F00D),
            0xC1_0010,
            PrivilegeLevel::Ring0,
        ),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(get(&memory, DATA + 0x10), 0x600D_F00D);
    assert_eq!(get(&memory, PD + 12), PS | P | RW | US | ACCESSED | DIRTY);
    assert_eq!(report.ad_update_count, 2);
    assert_eq!(report.ad_updates[0].unwrap().level, PageTableLevel::Pd);
    assert_eq!(report.ad_updates[1].unwrap().level, PageTableLevel::Pd);
    assert_eq!(
        get(&memory, PD),
        PT | P | RW | US,
        "PD[0] is not on the path"
    );
}

#[test]
fn a_pse36_page_above_4_gib_is_served_by_the_memory_map_not_truncated() {
    let (memory, config) = setup();
    let report = access_pg32_ram(
        &memory,
        &config,
        request(
            write(AccessWidth::Dword, 0x5566_7788),
            0x80_0030,
            PrivilegeLevel::Ring0,
        ),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(get(&memory, HIGH_AT + 0x30), 0x5566_7788);
    assert_eq!(get(&memory, 0x30), 0, "not the truncated low alias");
    // Past the one RAM page of the high region: an operand error with the 36-bit address, not a fault.
    let hole = access_pg32_ram(
        &memory,
        &config,
        request(READ4, 0x80_1000, PrivilegeLevel::Ring0),
        |_| {},
    );
    assert!(matches!(
        hole.outcome,
        Err(TrackedAccessError::OperandPhysical { address, .. }) if address == at(HIGH_AT + 0x1000)
    ));
    // Without the switch the same PDE has a reserved bit (13): #PF with P and RSVD.
    let plain = Pg32ProbeConfig::new(0x8001_0011, PD, CR4_PSE, 0, false).unwrap();
    let refused = access_pg32_ram(
        &memory,
        &plain,
        request(READ4, 0x80_0030, PrivilegeLevel::Ring0),
        |_| {},
    );
    let (reason, code, cr2) = fault(&refused);
    assert!(matches!(
        reason,
        PageFaultReason::ReservedBits { level: PageTableLevel::Pd, offending_bits } if offending_bits == 1 << 13
    ));
    assert_eq!((code, cr2), (9, 0x80_0030));
}

#[test]
fn an_mmio_operand_reaches_the_dispatch_with_the_dirty_bit_set_first_and_a_read_does_not_dirty() {
    let (memory, config) = setup();
    let mut calls = Vec::new();
    let report = access_pg32_with_mmio(
        &memory,
        &config,
        request(READ4, page(1) + 4, PrivilegeLevel::Ring0),
        |_| {},
        |address, width, operation| {
            calls.push((address, width, operation));
            Ok(0xABCD)
        },
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Read(0xABCD)));
    assert_eq!(
        calls,
        [(
            at(MMIO_AT + 4),
            4,
            RamAccessOperation::Read {
                width: AccessWidth::Dword
            }
        )]
    );
    assert_eq!(get(&memory, PT + 4), MMIO_AT | P | RW | US | ACCESSED);
    // A write: the dirty bit is already in the PTE when the device sees the access.
    let mut dirty_when_dispatched = None;
    let report = access_pg32_with_mmio(
        &memory,
        &config,
        request(
            write(AccessWidth::Word, 0x1FFFF),
            page(1),
            PrivilegeLevel::Ring0,
        ),
        |_| {},
        |address, width, operation| {
            dirty_when_dispatched = Some(get(&memory, PT + 4) & DIRTY != 0);
            calls.push((address, width, operation));
            Ok(0)
        },
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(dirty_when_dispatched, Some(true));
    assert_eq!(
        calls[1],
        (
            at(MMIO_AT),
            2,
            RamAccessOperation::Write {
                width: AccessWidth::Word,
                value: 0x1FFFF
            }
        )
    );
    // A device error is a typed backend error.
    let report = access_pg32_with_mmio(
        &memory,
        &config,
        request(READ4, page(1), PrivilegeLevel::Ring0),
        |_| {},
        |_, _, _| Err(MmioDispatchError::Backend("refused".to_owned())),
    );
    assert_eq!(
        report.outcome,
        Err(TrackedAccessError::MmioBackend {
            address: at(MMIO_AT),
            message: "refused".to_owned()
        })
    );
}

#[test]
fn without_a_dispatch_an_mmio_operand_is_refused_before_the_dirty_bit() {
    let (memory, config) = setup();
    let written = access_pg32_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 1), page(1), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        written.outcome,
        Err(TrackedAccessError::DeviceUnavailable {
            address: at(MMIO_AT)
        })
    );
    assert_eq!(get(&memory, PT + 4) & DIRTY, 0, "no dirty bit for it");
    assert_eq!(get(&memory, PT + 4) & ACCESSED, ACCESSED, "the walk set A");
}

#[test]
fn rom_and_holes_are_operand_errors_and_never_dirty_the_page() {
    let (memory, config) = setup();
    let rom = access_pg32_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 1), page(2), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        rom.outcome,
        Err(TrackedAccessError::OperandPhysical {
            address: at(ROM_AT),
            source: MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom)
        })
    );
    assert_eq!(get(&memory, PT + 8) & DIRTY, 0);
    let hole = access_pg32_ram(
        &memory,
        &config,
        request(READ4, page(3), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert!(matches!(
        hole.outcome,
        Err(TrackedAccessError::OperandPhysical { address, .. }) if address == at(HOLE)
    ));
    // The 4 MiB page at 0x40_0000 is outside the memory map too.
    let outside = access_pg32_ram(
        &memory,
        &config,
        request(READ4, 0x40_0010, PrivilegeLevel::Ring0),
        |_| {},
    );
    assert!(matches!(
        outside.outcome,
        Err(TrackedAccessError::OperandPhysical { address, .. }) if address == at(0x40_0010)
    ));
}

#[test]
fn page_faults_carry_the_32_bit_error_codes_and_the_linear_address() {
    let (memory, config) = setup();
    // PDE 4 is not present (linear 0x100_0000): read 0, write 2, user read 4, user write 6.
    for (operation, privilege, code) in [
        (READ4, PrivilegeLevel::Ring0, 0),
        (write(AccessWidth::Dword, 1), PrivilegeLevel::Ring0, 2),
        (READ4, PrivilegeLevel::Ring3, 4),
        (write(AccessWidth::Dword, 1), PrivilegeLevel::Ring3, 6),
    ] {
        let report = access_pg32_ram(
            &memory,
            &config,
            request(operation, 0x100_0abc, privilege),
            |_| {},
        );
        let (reason, got, cr2) = fault(&report);
        assert_eq!(
            reason,
            PageFaultReason::NotPresent {
                level: PageTableLevel::Pd
            }
        );
        assert_eq!(
            (got, cr2),
            (code, 0x100_0abc),
            "{operation:?} {privilege:?}"
        );
        // The record names the EIP the request gave.
        match report.outcome {
            Err(TrackedAccessError::PageFault { record, .. }) => {
                assert_eq!(record.saved_rip(), 0x1234);
            }
            other => panic!("{other:?}"),
        }
    }
    // Page 4 is supervisor-only: a user read is 5 (present, user), a user write 7.
    for (operation, code) in [(READ4, 5), (write(AccessWidth::Dword, 1), 7)] {
        let report = access_pg32_ram(
            &memory,
            &config,
            request(operation, page(4) + 8, PrivilegeLevel::Ring3),
            |_| {},
        );
        let (reason, got, cr2) = fault(&report);
        assert!(matches!(
            reason,
            PageFaultReason::Protection {
                user_denied: true,
                ..
            }
        ));
        assert_eq!((got, cr2), (code, u64::from(page(4) + 8)));
    }
    // Page 5 is read-only: a supervisor write with CR0.WP = 1 is 3, a user write 7; reads work.
    let supervisor = access_pg32_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 1), page(5), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(fault(&supervisor).1, 3);
    let user = access_pg32_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 1), page(5), PrivilegeLevel::Ring3),
        |_| {},
    );
    assert_eq!(fault(&user).1, 7);
    let read = access_pg32_ram(
        &memory,
        &config,
        request(READ4, page(5), PrivilegeLevel::Ring3),
        |_| {},
    );
    assert_eq!(read.outcome, Ok(TrackedAccessOutcome::Read(0)));
    assert_eq!(
        get(&memory, PT + 20) & DIRTY,
        0,
        "a refused write is not dirty"
    );
}

#[test]
fn write_protect_clear_lets_a_supervisor_write_a_read_only_page() {
    let (memory, _) = setup();
    let config = Pg32ProbeConfig::new(0x8000_0011, PD, CR4_PSE, 0, false).unwrap();
    let report = access_pg32_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 7), page(5), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(get(&memory, DATA + 0x2000), 7);
    let user = access_pg32_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 7), page(5), PrivilegeLevel::Ring3),
        |_| {},
    );
    assert_eq!(fault(&user).1, 7, "CPL 3 is refused whatever WP is");
}

#[test]
fn a_write_intent_read_walks_as_a_write_without_the_dirty_bit() {
    let (memory, config) = setup();
    memory
        .store_bytes(at(DATA + 0x40), &0x77u32.to_le_bytes(), |_| {})
        .unwrap();
    let intent = PaeOperandOperation::ReadForWrite {
        width: AccessWidth::Dword,
    };
    let report = access_pg32_ram(
        &memory,
        &config,
        request(intent, page(0) + 0x40, PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Read(0x77)));
    assert_eq!(get(&memory, PT) & DIRTY, 0);
    // It faults as a write does: the read-only page with WP is #PF(P, W) = 3.
    let refused = access_pg32_ram(
        &memory,
        &config,
        request(intent, page(5), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(fault(&refused).1, 3);
    // And an MMIO destination is refused before any device is reached.
    let mmio = access_pg32_with_mmio(
        &memory,
        &config,
        request(intent, page(1), PrivilegeLevel::Ring0),
        |_| {},
        |_, _, _| panic!("no device for a write-intent read"),
    );
    assert!(mmio.outcome.is_err());
}

#[test]
fn an_operand_that_crosses_a_page_is_refused_before_any_walk() {
    let (memory, config) = setup();
    let report = access_pg32_ram(
        &memory,
        &config,
        request(READ4, page(0) + 0xFFD, PrivilegeLevel::Ring0),
        |_| panic!("no write"),
    );
    assert_eq!(
        report.outcome,
        Err(TrackedAccessError::CrossesPage {
            address: GuestLinearAddress::new(0xFFD),
            width: 4
        })
    );
    assert!(report.trace.is_empty());
    assert_eq!(get(&memory, PD) & ACCESSED, 0, "no walk, no accessed bit");
}
