//! S4-10d: permission, execute-disable and reserved-bit matrix of the IA-32e 2 MiB leaf
//! (PDE.PS=1), checked against an independent reference model of Intel SDM vol. 3A
//! (section, table and figure numbers follow the text, not checked against revision 325462-093US),
//! access rights and the PDE that maps a 2-MByte page, and against the 4 KiB leaf with the same rights. Every walk entry point that accepts 2 MiB leaves is compared with the
//! tracked access, so a single walker owns the result.

use crate::page_walk::{translate_long64_2m_ram_read_only, translate_long64_2m_ram_write_check};
use crate::{
    AccessWidth, GuestLinearAddress, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PageFaultReason, PageTableLevel,
    PhysicalMemory, RamAccessOperation, RamLimits, TrackedAccessError, TrackedAccessOutcome,
    TrackedAccessReport, TrackedAccessRequest, access_long64_2m_ram,
};
use od_isa::memory::{AccessContext, AccessKind, AccessOrigin, InstructionMode, PrivilegeLevel};

const P: u64 = 1;
const RW: u64 = 1 << 1;
const US: u64 = 1 << 2;
const A: u64 = 1 << 5;
const D: u64 = 1 << 6;
const PS: u64 = 1 << 7;
const XD: u64 = 1 << 63;

const CR0_PG_PE_NE_ET_MP: u64 = 0x8000_0033;
const CR0_WP: u64 = 1 << 16;
const CR4_PAE: u64 = 1 << 5;
const EFER_LME_LMA: u64 = (1 << 8) | (1 << 10);
const EFER_NXE: u64 = 1 << 11;

const RAM: u64 = 0x40_0000;
const LINEAR: u64 = 0x0000_0080_8060_4abc;
const RIP: u64 = 0x1234;
const PML4E: u64 = 0x1008;
const PDPTE: u64 = 0x2010;
const PDE: u64 = 0x3018;
const PTE: u64 = 0x4020;
/// 2 MiB page base; the 4 KiB page sits at the same physical address as LINEAR inside it.
const LARGE_BASE: u64 = 0x20_0000;
const SMALL_PAGE: u64 = 0x20_4000;
const PHYSICAL: u64 = 0x20_4abc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ancestors {
    Permissive,
    Pml4ReadOnly,
    PdptSupervisor,
    Pml4Xd,
    PdptXd,
}

impl Ancestors {
    const ALL: [Self; 5] = [
        Self::Permissive,
        Self::Pml4ReadOnly,
        Self::PdptSupervisor,
        Self::Pml4Xd,
        Self::PdptXd,
    ];

    fn entries(self) -> (u64, u64) {
        let full = P | RW | US;
        match self {
            Self::Permissive => (0x2000 | full, 0x3000 | full),
            Self::Pml4ReadOnly => (0x2000 | P | US, 0x3000 | full),
            Self::PdptSupervisor => (0x2000 | full, 0x3000 | P | RW),
            Self::Pml4Xd => (0x2000 | full | XD, 0x3000 | full),
            Self::PdptXd => (0x2000 | full, 0x3000 | full | XD),
        }
    }

    /// (writable, user, execute-disabled, level of the XD bit).
    fn rights(self) -> (bool, bool, bool, Option<PageTableLevel>) {
        match self {
            Self::Permissive => (true, true, false, None),
            Self::Pml4ReadOnly => (false, true, false, None),
            Self::PdptSupervisor => (true, false, false, None),
            Self::Pml4Xd => (true, true, true, Some(PageTableLevel::Pml4)),
            Self::PdptXd => (true, true, true, Some(PageTableLevel::Pdpt)),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Case {
    large: bool,
    ancestors: Ancestors,
    rw: bool,
    us: bool,
    xd: bool,
    ring3: bool,
    access: AccessKind,
    wp: bool,
    nxe: bool,
}

#[derive(Debug, Eq, PartialEq)]
enum Expect {
    Allowed,
    Protection(u32),
    Reserved(u32, PageTableLevel),
}

fn leaf_flags(case: &Case) -> u64 {
    P | if case.rw { RW } else { 0 } | if case.us { US } else { 0 } | if case.xd { XD } else { 0 }
}

fn leaf_level(case: &Case) -> PageTableLevel {
    if case.large {
        PageTableLevel::Pd
    } else {
        PageTableLevel::Pt
    }
}

/// Literal PFEC layout (SDM vol. 3A, page-fault error code figure; number not checked against revision 325462-093US): P, W/R, U/S, RSVD, I/D (only with NXE).
fn pfec(case: &Case, rsvd: bool) -> u32 {
    1 | ((case.access == AccessKind::Write) as u32) << 1
        | (case.ring3 as u32) << 2
        | (rsvd as u32) << 3
        | ((case.access == AccessKind::Fetch && case.nxe) as u32) << 4
}

fn expect(case: &Case) -> Expect {
    let (anc_rw, anc_us, anc_xd, anc_xd_level) = case.ancestors.rights();
    let any_xd = anc_xd || case.xd;
    if !case.nxe && any_xd {
        // XD with EFER.NXE=0 is a reserved bit; the walk reports the first level holding it.
        return Expect::Reserved(pfec(case, true), anc_xd_level.unwrap_or(leaf_level(case)));
    }
    let writable = anc_rw && case.rw;
    let user = anc_us && case.us;
    let denied = (case.ring3 && !user)
        || (case.access == AccessKind::Write && !writable && (case.ring3 || case.wp))
        || (case.access == AccessKind::Fetch && case.nxe && any_xd);
    if denied {
        Expect::Protection(pfec(case, false))
    } else {
        Expect::Allowed
    }
}

fn new_memory() -> PhysicalMemory {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    PhysicalMemory::new(&map, RamLimits::new(RAM)).unwrap()
}

fn store(memory: &PhysicalMemory, entries: [u64; 4]) {
    for (address, value) in [PML4E, PDPTE, PDE, PTE].into_iter().zip(entries) {
        memory
            .store_u64(GuestPhysicalAddress::new(address), value, |_| {})
            .unwrap();
    }
    memory
        .store_u8(GuestPhysicalAddress::new(PHYSICAL), 0x5a, |_| {})
        .unwrap();
}

fn entries_of(case: &Case) -> [u64; 4] {
    let (pml4, pdpt) = case.ancestors.entries();
    if case.large {
        [pml4, pdpt, leaf_flags(case) | PS | LARGE_BASE, 0]
    } else {
        [
            pml4,
            pdpt,
            0x4000 | P | RW | US,
            SMALL_PAGE | leaf_flags(case),
        ]
    }
}

fn build(case: &Case) -> PhysicalMemory {
    let memory = new_memory();
    store(&memory, entries_of(case));
    memory
}

fn read_entries(memory: &PhysicalMemory) -> [u64; 4] {
    [PML4E, PDPTE, PDE, PTE]
        .map(|address| memory.load_u64(GuestPhysicalAddress::new(address)).unwrap())
}

fn paging(wp: bool, nxe: bool) -> Long64ProbeConfig {
    Long64ProbeConfig::new(
        CR0_PG_PE_NE_ET_MP | if wp { CR0_WP } else { 0 },
        0x1000,
        CR4_PAE,
        EFER_LME_LMA | if nxe { EFER_NXE } else { 0 },
    )
    .unwrap()
}

fn context(ring3: bool) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: if ring3 {
            PrivilegeLevel::Ring3
        } else {
            PrivilegeLevel::Ring0
        },
        origin: AccessOrigin::Explicit,
    }
}

fn operation(access: AccessKind) -> RamAccessOperation {
    match access {
        AccessKind::Read => RamAccessOperation::Read {
            width: AccessWidth::Byte,
        },
        AccessKind::Write => RamAccessOperation::Write {
            width: AccessWidth::Byte,
            value: 0xa5,
        },
        AccessKind::Fetch => RamAccessOperation::Fetch,
    }
}

fn request(case: &Case) -> TrackedAccessRequest {
    TrackedAccessRequest {
        instruction_rip: RIP,
        address: GuestLinearAddress::new(LINEAR),
        context: context(case.ring3),
        operation: operation(case.access),
    }
}

fn all_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for large in [true, false] {
        for ancestors in Ancestors::ALL {
            for bits in 0..8u8 {
                for ring3 in [false, true] {
                    for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
                        for wp in [false, true] {
                            for nxe in [false, true] {
                                cases.push(Case {
                                    large,
                                    ancestors,
                                    rw: bits & 1 != 0,
                                    us: bits & 2 != 0,
                                    xd: bits & 4 != 0,
                                    ring3,
                                    access,
                                    wp,
                                    nxe,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    cases
}

fn fault(error: &TrackedAccessError) -> (u32, u64, &PageFaultReason) {
    match error {
        TrackedAccessError::PageFault { record, reason } => (
            record
                .fault()
                .error_code
                .expect("page fault has an error code"),
            record.fault().linear_address.expect("page fault has CR2"),
            reason,
        ),
        other => panic!("not a guest page fault: {other:?}"),
    }
}

fn check_case(case: &Case) {
    let memory = build(case);
    let before = read_entries(&memory);
    let report: TrackedAccessReport =
        access_long64_2m_ram(&memory, &paging(case.wp, case.nxe), request(case), |_| {});
    let after = read_entries(&memory);
    let expected = expect(case);
    let leaf_index = if case.large { 2 } else { 3 };
    match &expected {
        Expect::Allowed => {
            let outcome = match case.access {
                AccessKind::Read => TrackedAccessOutcome::Read(0x5a),
                AccessKind::Fetch => TrackedAccessOutcome::Fetched(0x5a),
                AccessKind::Write => TrackedAccessOutcome::Written,
            };
            assert_eq!(report.outcome, Ok(outcome), "{case:?}");
            let dirty = if case.access == AccessKind::Write {
                D
            } else {
                0
            };
            for index in 0..4 {
                let walked = index <= leaf_index;
                let want = match index {
                    _ if !walked => before[index],
                    i if i == leaf_index => before[i] | A | dirty,
                    i => before[i] | A,
                };
                assert_eq!(after[index], want, "entry {index} of {case:?}");
            }
            let byte = memory.load_u8(GuestPhysicalAddress::new(PHYSICAL)).unwrap();
            assert_eq!(
                byte,
                if case.access == AccessKind::Write {
                    0xa5
                } else {
                    0x5a
                },
                "{case:?}"
            );
        }
        Expect::Protection(code) | Expect::Reserved(code, _) => {
            let error = report.outcome.as_ref().expect_err("a fault is expected");
            let (error_code, cr2, reason) = fault(error);
            assert_eq!(error_code, *code, "{case:?}");
            assert_eq!(cr2, LINEAR, "{case:?}");
            match (&expected, reason) {
                (
                    Expect::Protection(_),
                    PageFaultReason::Protection {
                        user_denied,
                        write_denied,
                        execute_denied,
                    },
                ) => {
                    assert!(user_denied | write_denied | execute_denied, "{case:?}");
                    assert_eq!(*user_denied, case.ring3 && !case_user(case), "{case:?}");
                }
                (
                    Expect::Reserved(_, level),
                    PageFaultReason::ReservedBits {
                        level: got_level,
                        offending_bits,
                    },
                ) => {
                    assert_eq!(got_level, level, "{case:?}");
                    assert_eq!(*offending_bits, XD, "{case:?}");
                }
                other => panic!("reason class differs for {case:?}: {other:?}"),
            }
            // A fault never sets D on any entry and never stores the operand.
            for (b, a) in before.iter().zip(after.iter()) {
                assert_eq!(a & D, b & D, "{case:?}");
            }
            let byte = memory.load_u8(GuestPhysicalAddress::new(PHYSICAL)).unwrap();
            assert_eq!(byte, 0x5a, "{case:?}");
        }
    }
}

fn case_user(case: &Case) -> bool {
    case.ancestors.rights().1 && case.us
}

#[test]
fn permission_xd_matrix_of_the_two_mib_leaf_matches_the_reference_model() {
    let cases = all_cases();
    assert_eq!(cases.len(), 2 * 5 * 8 * 2 * 3 * 2 * 2);
    let mut allowed = 0;
    let mut protection = 0;
    let mut reserved = 0;
    for case in &cases {
        match expect(case) {
            Expect::Allowed => allowed += 1,
            Expect::Protection(_) => protection += 1,
            Expect::Reserved(..) => reserved += 1,
        }
        check_case(case);
    }
    // Totals over both leaf sizes together reach every outcome class (not checked per leaf size).
    assert!(allowed > 0 && protection > 0 && reserved > 0);
}

#[test]
fn two_mib_and_four_kib_leaves_with_the_same_rights_fault_identically() {
    for case in all_cases().into_iter().filter(|case| case.large) {
        let small = Case {
            large: false,
            ..case
        };
        let run = |case: &Case| {
            let memory = build(case);
            access_long64_2m_ram(&memory, &paging(case.wp, case.nxe), request(case), |_| {}).outcome
        };
        let (large_outcome, small_outcome) = (run(&case), run(&small));
        match (&large_outcome, &small_outcome) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{case:?}"),
            (Err(a), Err(b)) => {
                let (code_a, cr2_a, reason_a) = fault(a);
                let (code_b, cr2_b, reason_b) = fault(b);
                assert_eq!((code_a, cr2_a), (code_b, cr2_b), "{case:?}");
                // Same class; the level of a leaf-level reserved XD differs (PD versus PT).
                assert_eq!(
                    std::mem::discriminant(reason_a),
                    std::mem::discriminant(reason_b),
                    "{case:?}"
                );
            }
            other => panic!("outcome class differs for {case:?}: {other:?}"),
        }
    }
}

#[test]
fn every_two_mib_walk_entry_point_agrees_with_the_tracked_access() {
    for case in all_cases().into_iter().filter(|case| case.large) {
        let config = paging(case.wp, case.nxe);
        let address = GuestLinearAddress::new(LINEAR);
        let ctx = context(case.ring3);
        let tracked = {
            let memory = build(&case);
            access_long64_2m_ram(&memory, &config, request(&case), |_| {}).outcome
        };
        // The read-only translation never commits anything and reports the same fault.
        let memory = build(&case);
        let before = read_entries(&memory);
        let read_only =
            translate_long64_2m_ram_read_only(&memory, &config, RIP, address, ctx, case.access);
        assert_eq!(read_entries(&memory), before, "{case:?}");
        match (&tracked, &read_only) {
            (Ok(_), Ok(physical)) => assert_eq!(physical.get(), PHYSICAL, "{case:?}"),
            (Err(a), Err(b)) => assert_eq!(a, b, "{case:?}"),
            other => panic!("read-only walk differs for {case:?}: {other:?}"),
        }
        if case.access == AccessKind::Write {
            let memory = build(&case);
            let check =
                translate_long64_2m_ram_write_check(&memory, &config, RIP, address, ctx, |_| {});
            match (&tracked, &check) {
                (Ok(_), Ok(())) => {
                    // The write check sets A like the walk of the store, but never D (a split store
                    // writes only after both pages passed), and stores no operand byte.
                    let entries = read_entries(&memory);
                    assert_eq!(entries[2] & (A | D), A, "{case:?}");
                    let byte = memory.load_u8(GuestPhysicalAddress::new(PHYSICAL)).unwrap();
                    assert_eq!(byte, 0x5a, "{case:?}");
                }
                (Err(a), Err(b)) => assert_eq!(a, b, "{case:?}"),
                other => panic!("write check differs for {case:?}: {other:?}"),
            }
        }
    }
}

fn large_case_base() -> Case {
    Case {
        large: true,
        ancestors: Ancestors::Permissive,
        rw: true,
        us: true,
        xd: false,
        ring3: false,
        access: AccessKind::Read,
        wp: true,
        nxe: true,
    }
}

fn read_with(case: &Case, pde: u64) -> TrackedAccessReport {
    let memory = new_memory();
    let mut entries = entries_of(case);
    entries[2] = pde;
    store(&memory, entries);
    access_long64_2m_ram(&memory, &paging(case.wp, case.nxe), request(case), |_| {})
}

#[test]
fn each_reserved_bit_of_a_two_mib_pde_faults_with_rsvd_for_every_access_kind() {
    let base = P | RW | US | PS | LARGE_BASE;
    // Bits 20:13 (below the 2 MiB address) and 51:36 (above MAXPHYADDR=36) are reserved.
    for bit in (13..=20).chain(36..=51) {
        for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
            for ring3 in [false, true] {
                let case = Case {
                    access,
                    ring3,
                    ..large_case_base()
                };
                let pde = base | (1 << bit);
                let report = read_with(&case, pde);
                let error = report.outcome.as_ref().expect_err("reserved bit faults");
                let (code, cr2, reason) = fault(error);
                assert_eq!(code, pfec(&case, true), "bit {bit} {case:?}");
                assert_eq!(cr2, LINEAR);
                assert_eq!(
                    *reason,
                    PageFaultReason::ReservedBits {
                        level: PageTableLevel::Pd,
                        offending_bits: 1 << bit,
                    }
                );
                // The ancestors keep A; the faulting PDE is neither accessed nor dirty.
                assert_eq!(report.ad_update_count, 2, "bit {bit}");
            }
        }
    }
}

#[test]
fn ignored_and_memory_type_bits_of_a_two_mib_pde_do_not_change_the_translation() {
    let case = large_case_base();
    // G (8), ignored (9, 10), R (11), PAT (12), ignored (58:52), protection key (62:59), PWT/PCD.
    let mut bits = vec![3, 4, 8, 9, 10, 11, 12];
    bits.extend(52..=62);
    for bit in bits {
        let pde = P | RW | US | PS | LARGE_BASE | (1 << bit);
        let report = read_with(&case, pde);
        assert_eq!(
            report.outcome,
            Ok(TrackedAccessOutcome::Read(0x5a)),
            "bit {bit}"
        );
    }
    // PS=1 without P is not present: the walk faults as not-present at the PD, not as a leaf.
    let report = read_with(&case, RW | US | PS | LARGE_BASE);
    let (code, _, reason) = fault(report.outcome.as_ref().unwrap_err());
    assert_eq!(code, 0);
    assert_eq!(
        *reason,
        PageFaultReason::NotPresent {
            level: PageTableLevel::Pd
        }
    );
}

#[test]
fn xd_on_the_two_mib_leaf_blocks_only_fetches_and_needs_nxe() {
    let pde = P | RW | US | PS | LARGE_BASE | XD;
    for access in [AccessKind::Read, AccessKind::Write] {
        let case = Case {
            access,
            ..large_case_base()
        };
        assert!(read_with(&case, pde).outcome.is_ok(), "{access:?}");
    }
    let fetch = Case {
        access: AccessKind::Fetch,
        ..large_case_base()
    };
    let report = read_with(&fetch, pde);
    let (code, _, reason) = fault(report.outcome.as_ref().unwrap_err());
    assert_eq!(code, 0x11);
    assert_eq!(
        *reason,
        PageFaultReason::Protection {
            user_denied: false,
            write_denied: false,
            execute_denied: true
        }
    );
    // NXE=0: bit 63 is reserved for every access kind, and the I/D flag stays clear.
    for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
        let case = Case {
            access,
            nxe: false,
            ..large_case_base()
        };
        let report = read_with(&case, pde);
        let (code, _, reason) = fault(report.outcome.as_ref().unwrap_err());
        assert_eq!(code, pfec(&case, true));
        assert_eq!(code & 0x10, 0);
        assert_eq!(
            *reason,
            PageFaultReason::ReservedBits {
                level: PageTableLevel::Pd,
                offending_bits: XD
            }
        );
    }
}

#[test]
fn two_mib_leaf_maps_every_4k_page_of_its_range_to_the_same_offset() {
    for base in [0u64, LARGE_BASE] {
        let memory = new_memory();
        store(
            &memory,
            [
                0x2000 | P | RW | US,
                0x3000 | P | RW | US,
                P | RW | US | PS | base,
                0,
            ],
        );
        for offset in [0u64, 0xfff, 0x1000, 0x1234, 0x10_0000, 0x1f_f000, 0x1f_ffff] {
            let physical = GuestPhysicalAddress::new(base + offset);
            // The first bytes of the low 2 MiB hold the page tables; probe a free byte instead.
            if base == 0 && offset < 0x5000 {
                continue;
            }
            memory.store_u8(physical, 0xc3, |_| {}).unwrap();
            let request = TrackedAccessRequest {
                instruction_rip: RIP,
                address: GuestLinearAddress::new((LINEAR & !0x1f_ffff) + offset),
                context: context(false),
                operation: operation(AccessKind::Read),
            };
            let report = access_long64_2m_ram(&memory, &paging(true, true), request, |_| {});
            assert_eq!(
                report.outcome,
                Ok(TrackedAccessOutcome::Read(0xc3)),
                "base {base:#x} offset {offset:#x}"
            );
        }
    }
    // A leaf whose 2 MiB page lies beyond RAM is a bounded physical error, not a guest fault.
    let case = large_case_base();
    let report = read_with(&case, P | RW | US | PS | 0x40_0000);
    assert!(matches!(
        report.outcome,
        Err(TrackedAccessError::OperandPhysical { .. })
    ));
}
