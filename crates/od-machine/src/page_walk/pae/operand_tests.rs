//! Tests of the operand access through PAE paging (S2-25k): the tracked access of
//! [`access_pae_ram`] and [`access_pae_with_mmio`]. No hardware oracle exists for PAE paging; the rights
//! and the page-fault error codes are the SDM text (Vol. 3A section 5.4 and Figure 5-12, Rev. 093), the
//! order of the effects is that of the 4-level tracked access of this crate.

use super::*;
use crate::{MachineMemoryMap, MemoryRegionSpec, RamLimits};
use od_isa::memory::{AccessOrigin, InstructionMode};

const P: u64 = 1;
const RW: u64 = 1 << 1;
const US: u64 = 1 << 2;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
const PS: u64 = 1 << 7;

const PDPT: u64 = 0x1000;
const PD: u64 = 0x2000;
const PT: u64 = 0x3000;
const DATA: u64 = 0x1_0000;
const RAM_END: u64 = 0x2_0000;
const ROM_AT: u64 = 0x2_0000;
const MMIO_AT: u64 = 0x3_0000;
/// An address no region covers.
const HOLE: u64 = 0x4_0000;
/// RAM above 4 GiB: bit 35 is the highest address bit of a PAE entry of the profile (36 bits).
const HIGH_AT: u64 = 0x8_0000_0000;

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

fn put(memory: &PhysicalMemory, address: u64, value: u64) {
    memory
        .store_bytes(at(address), &value.to_le_bytes(), |_| {})
        .unwrap();
}

fn get(memory: &PhysicalMemory, address: u64) -> u64 {
    let mut bytes = [0u8; 8];
    memory.load_bytes(at(address), &mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}

/// Linear page n (n < 9) -> PT[n]; linear 0x20_0000 is a 2 MiB page at physical 0.
fn tables(memory: &PhysicalMemory) {
    put(memory, PDPT, PD | P);
    put(memory, PD, PT | P | RW | US);
    put(memory, PD + 8, PS | P | RW | US);
    let entries = [
        DATA | P | RW | US,
        MMIO_AT | P | RW | US,
        ROM_AT | P | RW | US,
        HOLE | P | RW | US,
        HIGH_AT | P | RW | US,
        (DATA + 0x1000) | P | RW,
        (DATA + 0x2000) | P | US,
        (DATA + 0x3000) | P | RW | US | 1 << 36,
        0,
    ];
    for (i, entry) in entries.into_iter().enumerate() {
        put(memory, PT + 8 * i as u64, entry);
    }
}

fn setup() -> (PhysicalMemory, PaeProbeConfig) {
    let memory = memory();
    tables(&memory);
    let config = PaeProbeConfig::new(0x8001_0011, PDPT, 0x20, 0, [PD_PDPTE, 0, 0, 0]).unwrap();
    (memory, config)
}
const PD_PDPTE: u64 = PD | P;

fn request(
    operation: PaeOperandOperation,
    linear: u32,
    privilege: PrivilegeLevel,
) -> PaeAccessRequest {
    PaeAccessRequest {
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
    let report = access_pae_ram(
        &memory,
        &config,
        request(READ4, page(0) + 0x10, PrivilegeLevel::Ring0),
        |written| log.push(written),
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Read(0x1122_3344)));
    assert_eq!(get(&memory, PDPT), PD | P, "a PDPTE has no accessed bit");
    assert_eq!(get(&memory, PD), PT | P | RW | US | ACCESSED);
    assert_eq!(get(&memory, PT), DATA | P | RW | US | ACCESSED);
    assert_eq!(report.ad_update_count, 2);
    assert_eq!(report.ad_updates[0].unwrap().address, at(PD));
    assert_eq!(report.ad_updates[1].unwrap().address, at(PT));
    // The two entries are the only writes: a read writes no operand byte.
    assert_eq!(log, [range(PD, 8), range(PT, 8)]);
    // A second read changes nothing.
    let again = access_pae_ram(
        &memory,
        &config,
        request(READ4, page(0) + 0x10, PrivilegeLevel::Ring0),
        |_| panic!("no write"),
    );
    assert_eq!(again.ad_update_count, 0);
}

#[test]
fn a_write_commits_accessed_then_dirty_then_the_bytes_and_every_width_round_trips() {
    let (memory, config) = setup();
    let mut log = Vec::new();
    let report = access_pae_ram(
        &memory,
        &config,
        request(
            write(AccessWidth::Dword, 0xCAFE_BABE),
            page(0) + 0x20,
            PrivilegeLevel::Ring0,
        ),
        |written| log.push(written),
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(
        log,
        [
            range(PD, 8),
            range(PT, 8),
            range(PT, 8),
            range(DATA + 0x20, 4)
        ],
        "A of the PDE, A of the PTE, D of the PTE, then the operand"
    );
    assert_eq!(
        get(&memory, PD),
        PT | P | RW | US | ACCESSED,
        "no D on a PDE of a 4 KiB page"
    );
    assert_eq!(get(&memory, PT), DATA | P | RW | US | ACCESSED | DIRTY);
    assert_eq!(report.ad_update_count, 3);
    assert_eq!(report.ad_updates[2].unwrap().bit, AdBit::Dirty);
    for (width, value, len) in [
        (AccessWidth::Byte, 0xA5u64, 1u8),
        (AccessWidth::Word, 0xBEEF, 2),
        (AccessWidth::Dword, 0x0102_0304, 4),
        (AccessWidth::Qword, 0x0102_0304_0506_0708, 8),
    ] {
        let linear = page(0) + 0x40;
        let stored = access_pae_ram(
            &memory,
            &config,
            request(write(width, value), linear, PrivilegeLevel::Ring0),
            |_| {},
        );
        assert_eq!(stored.outcome, Ok(TrackedAccessOutcome::Written), "{len}");
        let read = access_pae_ram(
            &memory,
            &config,
            request(
                PaeOperandOperation::Read { width },
                linear,
                PrivilegeLevel::Ring0,
            ),
            |_| {},
        );
        assert_eq!(read.outcome, Ok(TrackedAccessOutcome::Read(value)), "{len}");
    }
}

#[test]
fn a_write_through_a_two_mib_page_sets_dirty_in_the_pde() {
    let (memory, config) = setup();
    let report = access_pae_ram(
        &memory,
        &config,
        request(
            write(AccessWidth::Dword, 0x0A0B_0C0D),
            0x20_8000,
            PrivilegeLevel::Ring0,
        ),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(get(&memory, PD + 8), PS | P | RW | US | ACCESSED | DIRTY);
    assert_eq!(report.ad_update_count, 2);
    assert_eq!(get(&memory, 0x8000) & 0xFFFF_FFFF, 0x0A0B_0C0D);
}

#[test]
fn physical_addresses_above_4_gib_are_served_by_the_memory_map_not_truncated() {
    let (memory, config) = setup();
    let report = access_pae_ram(
        &memory,
        &config,
        request(
            write(AccessWidth::Dword, 0x5566_7788),
            page(4) + 0x30,
            PrivilegeLevel::Ring0,
        ),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(get(&memory, HIGH_AT + 0x30) & 0xFFFF_FFFF, 0x5566_7788);
    assert_eq!(get(&memory, 0x30), 0, "not the truncated low alias");
    // A hole is an operand error with the 36-bit address, not a fault.
    put(&memory, PT + 32, (HIGH_AT + 0x1000) | P | RW | US);
    let hole = access_pae_ram(
        &memory,
        &config,
        request(READ4, page(4), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert!(matches!(
        hole.outcome,
        Err(TrackedAccessError::OperandPhysical { address, .. }) if address == at(HIGH_AT + 0x1000)
    ));
}

#[test]
fn an_mmio_operand_reaches_the_dispatch_with_the_dirty_bit_set_first_and_a_read_does_not_dirty() {
    let (memory, config) = setup();
    let mut calls = Vec::new();
    let report = access_pae_with_mmio(
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
    assert_eq!(get(&memory, PT + 8), MMIO_AT | P | RW | US | ACCESSED);
    // A write: the dirty bit is already in the PTE when the device sees the access.
    let mut dirty_when_dispatched = None;
    let report = access_pae_with_mmio(
        &memory,
        &config,
        request(
            write(AccessWidth::Word, 0x1FFFF),
            page(1),
            PrivilegeLevel::Ring0,
        ),
        |_| {},
        |address, width, operation| {
            dirty_when_dispatched = Some(get(&memory, PT + 8) & DIRTY != 0);
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
    // A device error is a typed backend error; the operation is a read, so no dirty bit is set.
    let report = access_pae_with_mmio(
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
    let write_report = access_pae_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 1), page(1), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        write_report.outcome,
        Err(TrackedAccessError::DeviceUnavailable {
            address: at(MMIO_AT)
        })
    );
    let read_report = access_pae_ram(
        &memory,
        &config,
        request(READ4, page(1), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        read_report.outcome,
        Err(TrackedAccessError::OperandPhysical {
            address: at(MMIO_AT),
            source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Mmio)
        })
    );
    // The walks set A; neither refused access set D.
    assert_eq!(get(&memory, PT + 8), MMIO_AT | P | RW | US | ACCESSED);
    // The write-intent read never reaches a device, dispatch or not.
    let rfw = access_pae_with_mmio(
        &memory,
        &config,
        request(
            PaeOperandOperation::ReadForWrite {
                width: AccessWidth::Dword,
            },
            page(1),
            PrivilegeLevel::Ring0,
        ),
        |_| {},
        |_, _, _| panic!("no device"),
    );
    assert!(matches!(
        rfw.outcome,
        Err(TrackedAccessError::DeviceUnavailable { .. })
    ));
    assert_eq!(get(&memory, PT + 8) & DIRTY, 0);
}

#[test]
fn rom_and_holes_are_operand_errors_and_never_dirty_the_page() {
    let (memory, config) = setup();
    let rom_write = access_pae_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 1), page(2), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        rom_write.outcome,
        Err(TrackedAccessError::OperandPhysical {
            address: at(ROM_AT),
            source: MemoryAccessError::ReadOnlyRegion(MemoryRegionKind::Rom)
        })
    );
    assert_eq!(get(&memory, PT + 16) & DIRTY, 0);
    let hole = access_pae_ram(
        &memory,
        &config,
        request(READ4, page(3), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert!(matches!(
        hole.outcome,
        Err(TrackedAccessError::OperandPhysical { address, .. }) if address == at(HOLE)
    ));
}

#[test]
fn page_faults_carry_the_pae_error_codes_and_the_linear_address() {
    // (linear, access, privilege, expected reason, error code): P = bit 0, W/R = 1, U/S = 2, RSVD = 3.
    let user = PrivilegeLevel::Ring3;
    let sup = PrivilegeLevel::Ring0;
    let cases = [
        // PDPTE 1 is not present.
        (0x4000_0000u32, false, sup, 0u32),
        (0x4000_0000, true, sup, 2),
        (0x4000_0000, false, user, 4),
        (0x4000_0000, true, user, 6),
        // PTE 8 is not present.
        (page(8), false, sup, 0),
        (page(8), true, user, 6),
        // A supervisor-only page (PT[5]): U/S denies CPL 3, present.
        (page(5), false, user, 5),
        (page(5), true, user, 7),
        // A read-only page (PT[6]) with CR0.WP = 1 is write-denied also for CPL 0.
        (page(6), true, sup, 3),
        (page(6), true, user, 7),
        // Bit 36 of PT[7] is reserved with PAE paging.
        (page(7), false, sup, 9),
        (page(7), true, user, 15),
    ];
    for (linear, write_access, privilege, code) in cases {
        let (memory, config) = setup();
        let operation = if write_access {
            write(AccessWidth::Dword, 7)
        } else {
            READ4
        };
        let report = access_pae_ram(
            &memory,
            &config,
            request(operation, linear, privilege),
            |_| {},
        );
        let (_, error_code, cr2) = fault(&report);
        assert_eq!(
            (error_code, cr2),
            (code, u64::from(linear)),
            "{linear:#x} write={write_access} {privilege:?}"
        );
        // No fault sets a dirty bit or writes an operand byte.
        for entry in 0..9 {
            assert_eq!(get(&memory, PT + 8 * entry) & DIRTY, 0, "{linear:#x}");
        }
        assert_eq!(get(&memory, DATA + 0x2000), 0);
    }
    // The reason names the check.
    let (memory, config) = setup();
    let report = access_pae_ram(
        &memory,
        &config,
        request(READ4, 0x4000_0000, PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        fault(&report).0,
        PageFaultReason::NotPresent {
            level: PageTableLevel::Pdpt
        }
    );
    let report = access_pae_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 0), page(6), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(
        fault(&report).0,
        PageFaultReason::Protection {
            user_denied: false,
            write_denied: true,
            execute_denied: false
        }
    );
}

#[test]
fn write_protect_clear_lets_a_supervisor_write_a_read_only_page() {
    let (memory, _) = setup();
    let config = PaeProbeConfig::new(0x8000_0011, PDPT, 0x20, 0, [PD | P, 0, 0, 0]).unwrap();
    let report = access_pae_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 9), page(6), PrivilegeLevel::Ring0),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Written));
    assert_eq!(
        get(&memory, PT + 48),
        (DATA + 0x2000) | P | US | ACCESSED | DIRTY
    );
    // CPL 3 is denied by R/W = 0 whatever CR0.WP.
    let (_, error_code, _) = fault(&access_pae_ram(
        &memory,
        &config,
        request(write(AccessWidth::Dword, 9), page(6), PrivilegeLevel::Ring3),
        |_| {},
    ));
    assert_eq!(error_code, 7);
}

#[test]
fn a_write_intent_read_walks_as_a_write_without_the_dirty_bit() {
    let (memory, config) = setup();
    memory
        .store_bytes(at(DATA + 8), &0x99u32.to_le_bytes(), |_| {})
        .unwrap();
    let rfw = |linear, privilege| {
        access_pae_ram(
            &memory,
            &config,
            request(
                PaeOperandOperation::ReadForWrite {
                    width: AccessWidth::Dword,
                },
                linear,
                privilege,
            ),
            |_| {},
        )
    };
    let report = rfw(page(0) + 8, PrivilegeLevel::Ring0);
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Read(0x99)));
    assert_eq!(
        get(&memory, PT) & DIRTY,
        0,
        "the dirty bit waits for the write"
    );
    // A read-only destination is a write fault before any byte is read.
    let (reason, error_code, _) = fault(&rfw(page(6), PrivilegeLevel::Ring0));
    assert_eq!(error_code, 3);
    assert!(matches!(
        reason,
        PageFaultReason::Protection {
            write_denied: true,
            ..
        }
    ));
}

#[test]
fn an_operand_that_crosses_a_page_is_refused_before_any_walk() {
    let (memory, config) = setup();
    let before = [get(&memory, PD), get(&memory, PT)];
    for (linear, width) in [(0xFFEu32, AccessWidth::Dword), (0xFF9, AccessWidth::Qword)] {
        let report = access_pae_ram(
            &memory,
            &config,
            request(
                PaeOperandOperation::Read { width },
                linear,
                PrivilegeLevel::Ring0,
            ),
            |_| panic!("no write"),
        );
        assert_eq!(
            report.outcome,
            Err(TrackedAccessError::CrossesPage {
                address: GuestLinearAddress::new(u64::from(linear)),
                width: width.bytes()
            })
        );
        assert_eq!(report.ad_update_count, 0);
    }
    assert_eq!([get(&memory, PD), get(&memory, PT)], before);
}

#[test]
fn a_fetch_walks_as_a_fetch_and_is_served_from_ram_only() {
    let (memory, config) = setup();
    put(&memory, DATA, 0x1122_3344_5566_7788);
    // RAM: the byte, A on the PDE and the PTE, no D.
    let report = access_pae_ram(
        &memory,
        &config,
        request(
            PaeOperandOperation::Fetch,
            page(0) + 1,
            PrivilegeLevel::Ring0,
        ),
        |_| {},
    );
    assert_eq!(report.outcome, Ok(TrackedAccessOutcome::Fetched(0x77)));
    assert_eq!(get(&memory, PT) & (ACCESSED | DIRTY), ACCESSED);
    // MMIO and ROM: an instruction fetch reaches no device, even with a dispatch.
    for (linear, kind) in [
        (page(1), MemoryRegionKind::Mmio),
        (page(2), MemoryRegionKind::Rom),
    ] {
        let report = access_pae_with_mmio(
            &memory,
            &config,
            request(PaeOperandOperation::Fetch, linear, PrivilegeLevel::Ring0),
            |_| {},
            |_, _, _| panic!("a fetch must not reach a device"),
        );
        assert!(
            matches!(
                report.outcome,
                Err(TrackedAccessError::OperandPhysical {
                    source: MemoryAccessError::UnreadableRegion(found),
                    ..
                }) if found == kind
            ),
            "{kind:?}: {:?}",
            report.outcome
        );
    }
}

#[test]
fn the_windows_of_a_set_loop_limit_what_the_dispatch_serves() {
    let (memory, config) = setup();
    let calls = std::cell::Cell::new(0);
    let access = |windows: &[MmioWindow], linear, operation| {
        access_pae_with_mmio_windows(
            &memory,
            &config,
            request(operation, linear, PrivilegeLevel::Ring0),
            |_| {},
            windows,
            |_, _, _| {
                calls.set(calls.get() + 1);
                Ok(7)
            },
        )
    };
    // The operand is wholly inside a window: served. Partly outside, or outside: the typed error of
    // an access without a dispatch (a read: UnreadableRegion; a write: DeviceUnavailable).
    let inside = [(MMIO_AT, 0x1000)];
    let tail = [(MMIO_AT + 0x10, 0x1000)];
    let around = [(MMIO_AT + 4, 4)];
    assert_eq!(
        access(&inside, page(1), READ4).outcome,
        Ok(TrackedAccessOutcome::Read(7))
    );
    assert!(matches!(
        access(&tail, page(1), READ4).outcome,
        Err(TrackedAccessError::OperandPhysical { .. })
    ));
    assert!(matches!(
        access(&around, page(1) + 2, READ4).outcome,
        Err(TrackedAccessError::OperandPhysical { .. })
    ));
    assert!(matches!(
        access(&tail, page(1), write(AccessWidth::Dword, 1)).outcome,
        Err(TrackedAccessError::DeviceUnavailable { .. })
    ));
    assert_eq!(
        access(&inside, page(1), write(AccessWidth::Dword, 1)).outcome,
        Ok(TrackedAccessOutcome::Written)
    );
    assert_eq!(calls.get(), 2);
}
