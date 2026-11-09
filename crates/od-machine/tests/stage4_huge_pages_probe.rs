use od_isa::Register;
use od_machine::{
    LoadedStage4HugePagesProbe, STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_IMAGE_GPA,
    STAGE4_HUGE_PAGES_ENTRIES, STAGE4_HUGE_PAGES_FINAL_RIP, STAGE4_HUGE_PAGES_GIB_STEPS,
    STAGE4_HUGE_PAGES_IMAGE_BYTES, STAGE4_HUGE_PAGES_INITIAL_RAX, STAGE4_HUGE_PAGES_INSTRUCTIONS,
    STAGE4_HUGE_PAGES_STEP_RIPS, STAGE4_HUGE_PAGES_STEPS, STAGE4_HUGE_PAGES_STORE_VALUE,
    Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError, Stage4HugePagesCase,
    Stage4HugePagesEvent, Stage4HugePagesInjection, Stage4HugePagesStep::*, Stage4HugePagesStop,
    load_stage4_huge_pages_probe,
};

const NX: u64 = 1 << 63;
const A: u64 = 1 << 5;
const D: u64 = 1 << 6;
const START: u64 = 0x1122_3344_5566_7788;
const MIDDLE: u64 = 0x99aa_bbcc_ddee_ff01;
const END: u64 = 0x0f1e_2d3c_4b5a_6978;
const SENTINEL: u64 = 0xfeed_face_0bad_f00d;
const DECOY: u64 = 0xa5a5_a5a5_a5a5_a5a5;
const RAX0: u64 = 0xdead_beef_cafe_babe;
/// RCX of the handoff: the value the store writes.
const W: u64 = 0xc0de_c0de_1234_56ab;

fn put(b: &mut [u8], gpa: u64, v: &[u8]) {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    b[o..o + v.len()].copy_from_slice(v)
}

/// The fixed image written from the architecture (SDM Vol. 3A 4.5, entry formats:
/// P=bit0, R/W=bit1, PS=bit7, address bits 51:21 for a 2 MiB PDE, XD=bit63), not from the module
/// under test.
fn image(values: [u64; 3]) -> Vec<u8> {
    let mut b = vec![0; STAGE4_HUGE_PAGES_IMAGE_BYTES];
    for (g, v) in [
        (0x1000, 0x2003),         // PML4E[0] -> PDPT 0
        (0x1008, 0x5003),         // PML4E[1] -> PDPT 1
        (0x2000, 0x3003),         // PDPT0[0] -> PD
        (0x3010, 0x4003),         // PD[2] -> code table
        (0x3020, 0x20_0083 | NX), // PD[4]: 2 MiB leaf, frame 0x200000
        (0x3028, 0x40_0083 | NX), // PD[5]: 2 MiB leaf, frame 0x400000
        (0x4000, 0x7001),         // code frame 0x7000
        (0x5000, 0x4000_0083),    // PDPT1[0]: PS=1, a 1 GiB leaf
    ] {
        put(&mut b, g, &u64::to_le_bytes(v));
    }
    put(
        &mut b,
        0x7000,
        &[
            0x48, 0x8b, 0x03, 0x48, 0x8b, 0x03, 0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b, 0x48, 0x8b,
            0x03, 0xee, 0xf4,
        ],
    );
    put(&mut b, 0x8000, &DECOY.to_le_bytes());
    put(&mut b, 0x20_0000, &values[0].to_le_bytes());
    put(&mut b, 0x30_0000, &values[1].to_le_bytes());
    put(&mut b, 0x3f_ff00, &[0x5a; 8]);
    put(&mut b, 0x3f_fff8, &values[2].to_le_bytes());
    put(&mut b, 0x40_0000, &SENTINEL.to_le_bytes());
    b
}

fn load(values: [u64; 3]) -> LoadedStage4HugePagesProbe {
    load_stage4_huge_pages_probe(&image(values)).unwrap()
}

fn writes(e: &Stage4HugePagesEvent) -> Vec<(u64, u64)> {
    e.committed_writes
        .iter()
        .map(|r| (r.start().get(), r.len()))
        .collect()
}

/// Entry order of the evidence: PML4E[0], PDPT0[0], PD[2], code PTE, PD[4], PD[5], PML4E[1], PDPT1[0].
const INITIAL: [u64; 8] = [
    0x2003,
    0x3003,
    0x4003,
    0x7001,
    0x20_0083 | NX,
    0x40_0083 | NX,
    0x5003,
    0x4000_0083,
];

#[test]
fn entry_state_is_the_fixed_loader_handoff_with_cpl0_ac_clear_and_nxe_set() {
    let mut m = load([START, MIDDLE, END]);
    let s = m.state();
    assert_eq!(s.rip, 0x40_0000);
    assert_eq!(s.rflags, 2);
    assert_eq!(s.cr0, Some(0x8001_0033));
    assert_eq!(s.cr3, None);
    assert_eq!(s.cr4, Some(0x20));
    assert_eq!(s.gpr[Register::Rax as usize], RAX0);
    assert_eq!(s.gpr[Register::Rbx as usize], 0x80_0000);
    assert_eq!(s.gpr[Register::Rcx as usize], W);
    assert_eq!(s.gpr[Register::Rdx as usize], 0x3f8);
    assert_eq!(m.cpl(), 0);
    assert!(!m.ac());
    assert!(m.nxe());
    assert_eq!(m.efer(), STAGE4_FLAT_PROBE_EFER);
    assert_eq!(m.efer() & (1 << 11), 1 << 11);
    assert_eq!(m.values(), [START, MIDDLE, END]);
    assert_eq!(m.case(), Stage4HugePagesCase::TwoMiB);
    assert_eq!(m.entries(), INITIAL);
    assert_eq!(STAGE4_HUGE_PAGES_ENTRIES, INITIAL);
    assert_eq!(STAGE4_HUGE_PAGES_STORE_VALUE, W);
    assert_eq!(STAGE4_HUGE_PAGES_INITIAL_RAX, RAX0);
    // The PDE is a 2 MiB leaf: P, R/W, PS set; A and D clear; bits 20:13 and PAT clear.
    let pde = INITIAL[4];
    assert_eq!(pde & 0x1fe3ff, 0x83);
    assert_eq!(pde & 0x1fe000, 0);
    assert_eq!(pde & ((1 << 12) | A | D), 0);
}

#[test]
fn two_mib_case_reads_three_offsets_stores_and_reads_back_through_the_pde_leaf() {
    let mut m = load([START, MIDDLE, END]);
    let mut callbacks = Vec::new();
    let report = m.run(16, |r| callbacks.push(r.start().get()));
    assert_eq!(report.stop, Stage4HugePagesStop::Completed);
    assert_eq!(
        (report.steps, report.retired),
        (STAGE4_HUGE_PAGES_STEPS, STAGE4_HUGE_PAGES_INSTRUCTIONS)
    );
    assert_eq!(report.serial, [0xab], "low byte of the stored value");
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(
        steps,
        [ReadStart, ReadMiddle, ReadEnd, Store, ReadBack, Out, Halt]
    );

    // RIP after each retired instruction: 3 loads (3 bytes), store (3), load (3), EE (1), HLT (1).
    let rips_after = [
        0x40_0003, 0x40_0006, 0x40_0009, 0x40_000c, 0x40_000f, 0x40_0010, 0x40_0011,
    ];
    // RAX: the little-endian qword at offset 0, 0x100000 and 0x1ffff8; the store leaves RAX
    // alone; the read back returns the stored value; OUT and HLT leave RAX alone.
    let rax = [START, MIDDLE, END, END, W, W, W];
    // The sequencer supplies RBX: GVA 0x800000 + offset.
    let rbx = [
        0x80_0000, 0x90_0000, 0x9f_fff8, 0x9f_ff00, 0x9f_ff00, 0x9f_ff00, 0x9f_ff00,
    ];
    // PML4E, PDPTE, PD[2] and the code PTE get A from the first fetch (SDM 4.8); the 2 MiB PDE
    // gets A on the first data access and D only on the store (D is set in the entry that maps
    // the page, which for a 2 MiB page is the PDE). Nothing else changes.
    let after_reads = [
        0x2023,
        0x3023,
        0x4023,
        0x7021,
        0x20_0083 | NX | A,
        0x40_0083 | NX,
        0x5003,
        0x4000_0083,
    ];
    let mut after_store = after_reads;
    after_store[4] |= D;
    for (i, e) in report.events.iter().enumerate() {
        assert!(e.retired, "step {i} retired");
        assert_eq!(e.rip, rips_after[i], "RIP after step {i}");
        assert_eq!(e.rax, rax[i], "RAX after step {i}");
        if i < 5 {
            assert_eq!(e.rbx, rbx[i], "RBX of step {i}");
        }
        assert_eq!(e.cpl, 0);
        assert_eq!(e.cr2, None, "no #PF, so no CR2");
        assert!(e.page_fault.is_none() && e.memory_error.is_none());
        let expected = if i >= 3 { after_store } else { after_reads };
        assert_eq!(e.entries, expected, "paging entries after step {i}");
    }
    // Committed writes: step 1 sets A on PML4E, PDPTE, PD[2] and the code PTE (fetch), then on the
    // 2 MiB PDE. Steps 2 and 3 walk nothing new. The store sets D on the PDE and writes the qword
    // at frame base + 0x1fff00. Nothing else commits.
    assert_eq!(
        writes(&report.events[0]),
        [
            (0x1000, 8),
            (0x2000, 8),
            (0x3010, 8),
            (0x4000, 8),
            (0x3020, 8)
        ]
    );
    assert!(report.events[1].committed_writes.is_empty());
    assert!(report.events[2].committed_writes.is_empty());
    assert_eq!(writes(&report.events[3]), [(0x3020, 8), (0x3f_ff00, 8)]);
    for e in &report.events[4..] {
        assert!(e.committed_writes.is_empty());
    }
    assert_eq!(
        callbacks,
        [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x3020, 0x3f_ff00]
    );
    let s = m.state();
    assert_eq!(s.rip, STAGE4_HUGE_PAGES_FINAL_RIP);
    assert_eq!(s.gpr[Register::Rax as usize], W);
    assert_eq!(s.activity, Some(od_isa::CpuActivity::Halted));
    assert_eq!(
        STAGE4_HUGE_PAGES_STEP_RIPS,
        [
            0x40_0000, 0x40_0003, 0x40_0006, 0x40_0009, 0x40_000c, 0x40_000f, 0x40_0010
        ]
    );
    for i in 0..7 {
        let next = STAGE4_HUGE_PAGES_STEP_RIPS
            .get(i + 1)
            .copied()
            .unwrap_or(STAGE4_HUGE_PAGES_FINAL_RIP);
        assert_eq!(
            report.events[i].rip, next,
            "step {i} ends where the next starts"
        );
    }
    // The store landed in the 2 MiB frame at base + 0x1fff00; the values were not modified.
    assert_eq!(m.physical_qword(0x3f_ff00), W);
    assert_eq!(m.physical_qword(0x20_0000), START);
    assert_eq!(m.physical_qword(0x30_0000), MIDDLE);
    assert_eq!(m.physical_qword(0x3f_fff8), END);
    assert_eq!(m.physical_qword(0x40_0000), SENTINEL);
}

#[test]
fn each_offset_returns_its_own_value_and_other_values_change_only_that_step() {
    let other = [
        0x0102_0304_0506_0708,
        0x1112_1314_1516_1718,
        0x2122_2324_2526_2728,
    ];
    let a = load([START, MIDDLE, END]).run(16, |_| {});
    let b = load(other).run(16, |_| {});
    assert_eq!(a.stop, Stage4HugePagesStop::Completed);
    assert_eq!(b.stop, Stage4HugePagesStop::Completed);
    assert_eq!(
        [a.events[0].rax, a.events[1].rax, a.events[2].rax],
        [START, MIDDLE, END]
    );
    assert_eq!([b.events[0].rax, b.events[1].rax, b.events[2].rax], other);
    // The values at the three offsets are pairwise different and none equals the store value.
    for report in [&a, &b] {
        let v = [
            report.events[0].rax,
            report.events[1].rax,
            report.events[2].rax,
        ];
        assert!(v[0] != v[1] && v[0] != v[2] && v[1] != v[2]);
        assert!(!v.contains(&W));
        assert_eq!(report.events[4].rax, W);
    }
}

#[test]
fn negative_1gib_pdpte_ps_is_a_guest_page_fault_with_p_and_rsvd() {
    let mut m = load([START, MIDDLE, END]);
    m.select_case(Stage4HugePagesCase::GibNegative);
    let mut callbacks = Vec::new();
    let report = m.run(16, |r| callbacks.push(r.start().get()));
    assert_eq!(report.stop, Stage4HugePagesStop::PageFault(GibRead));
    assert_eq!(
        (report.steps, report.retired),
        (STAGE4_HUGE_PAGES_GIB_STEPS, 0)
    );
    assert!(report.serial.is_empty());
    let e = &report.events[0];
    assert!(!e.retired);
    let pf = e.page_fault.as_ref().unwrap();
    // SDM Vol. 3A 4.7: vector 14; error code bit 0 P=1 (the entry is present), bit 1 W/R=0 (read),
    // bit 2 U/S=0 (CPL0), bit 3 RSVD=1 (reserved bit set), bit 4 I/D=0 (data): 0b01001 = 0x09.
    // CR2 is the faulting linear address.
    assert_eq!(pf.vector, 14);
    assert_eq!(pf.error_code, Some(0x09));
    assert_eq!(pf.cr2, 0x80_0000_0000);
    assert_eq!(
        pf.saved_rip, 0x40_0000,
        "the faulting instruction, not its successor"
    );
    assert_eq!(pf.access, "Read");
    assert!(e.memory_error.is_none());
    // No retirement: RIP and RAX are unchanged. Delivery is not part of the case: CR2 uncommitted.
    assert_eq!(e.rip, 0x40_0000);
    assert_eq!(e.rax, RAX0);
    assert_eq!(e.rbx, 0x80_0000_0000);
    assert_eq!(e.cr2, None);
    // A is set on the ancestors walked successfully: PML4E[1] (data route) and the fetch path
    // (PML4E[0], PDPT0[0], PD[2], code PTE). PDPT1[0] (the PS=1 entry) gets neither A nor D, and
    // the 2 MiB PDE is not touched.
    assert_eq!(
        e.entries,
        [
            0x2003 | A,
            0x3003 | A,
            0x4003 | A,
            0x7001 | A,
            0x20_0083 | NX,
            0x40_0083 | NX,
            0x5003 | A,
            0x4000_0083
        ]
    );
    assert_eq!(
        writes(e),
        [
            (0x1000, 8),
            (0x2000, 8),
            (0x3010, 8),
            (0x4000, 8),
            (0x1008, 8)
        ],
        "the data walk commits A on PML4E[1] only"
    );
    assert_eq!(callbacks, [0x1000, 0x2000, 0x3010, 0x4000, 0x1008]);
    // No data was transferred and nothing was stored.
    assert_eq!(m.physical_qword(0x3f_ff00), 0x5a5a_5a5a_5a5a_5a5a);
}

#[test]
fn injections_are_pairwise_distinguishable_by_stop_steps_retired_pfec_and_rax() {
    type Key = (String, u64, u64, Option<u32>, Vec<u64>);
    let run = |case: Stage4HugePagesCase, injection: Stage4HugePagesInjection| -> Key {
        let mut m = load([START, MIDDLE, END]);
        m.select_case(case);
        m.inject(injection).unwrap();
        let r = m.run(16, |_| {});
        let pfec = r
            .events
            .iter()
            .find_map(|e| e.page_fault.as_ref().and_then(|f| f.error_code));
        (
            format!("{:?}", r.stop),
            r.steps,
            r.retired,
            pfec,
            r.events.iter().map(|e| e.rax).collect(),
        )
    };
    use Stage4HugePagesCase::*;
    use Stage4HugePagesInjection as I;
    let cases = [
        ("none", run(TwoMiB, I::None)),
        ("pde_ps_cleared", run(TwoMiB, I::PdePsCleared)),
        ("offset_past_page", run(TwoMiB, I::OffsetPastPage)),
        ("negative", run(GibNegative, I::None)),
        ("pdpte_host_error", run(GibNegative, I::PdptePsHostError)),
    ];
    for (i, (na, a)) in cases.iter().enumerate() {
        for (nb, b) in &cases[i + 1..] {
            assert_ne!(a, b, "{na} and {nb} must be distinguishable");
        }
    }
    let get = |n: &str| cases.iter().find(|(name, _)| *name == n).unwrap().1.clone();
    assert_eq!(
        get("none"),
        (
            "Completed".to_string(),
            7,
            7,
            None,
            vec![START, MIDDLE, END, END, W, W, W]
        )
    );
    // PDE.PS cleared: the PDE is a pointer to a 4 KiB table whose entry 0 maps the decoy frame.
    // Offset 0 returns the decoy; offset 0x100000 is PT index 0x100, not present: #PF, PFEC 0.
    assert_eq!(
        get("pde_ps_cleared"),
        (
            "PageFault(ReadMiddle)".to_string(),
            2,
            1,
            Some(0x00),
            vec![DECOY, DECOY]
        )
    );
    // One byte past the 2 MiB page the next PDE maps another frame: the sentinel, no fault.
    assert_eq!(
        get("offset_past_page"),
        (
            "Completed".to_string(),
            7,
            7,
            None,
            vec![START, MIDDLE, SENTINEL, SENTINEL, W, W, W]
        )
    );
    assert_eq!(
        get("negative"),
        (
            "PageFault(GibRead)".to_string(),
            1,
            0,
            Some(0x09),
            vec![RAX0]
        )
    );
    assert_eq!(
        get("pdpte_host_error"),
        ("Backend(GibRead)".to_string(), 1, 0, None, vec![RAX0])
    );
}

#[test]
fn pde_ps_cleared_takes_the_4k_walk_and_pdpte_host_error_is_unsupported_page_size_without_a_fault()
{
    let mut m = load([START, MIDDLE, END]);
    m.inject(Stage4HugePagesInjection::PdePsCleared).unwrap();
    let r = m.run(16, |_| {});
    let e = &r.events[1];
    let pf = e.page_fault.as_ref().unwrap();
    assert_eq!(pf.error_code, Some(0x00));
    assert_eq!(pf.cr2, 0x90_0000);
    assert!(!e.retired);
    // The PDE is now a pointer entry: A set, PS clear, no D (D belongs to the leaf, here the PTE).
    assert_eq!(e.entries[4], (0x6003 | NX) | A);
    assert_eq!(e.entries[4] & (1 << 7), 0);

    let mut m = load([START, MIDDLE, END]);
    m.select_case(Stage4HugePagesCase::GibNegative);
    m.inject(Stage4HugePagesInjection::PdptePsHostError)
        .unwrap();
    let r = m.run(16, |_| {});
    assert_eq!(r.stop, Stage4HugePagesStop::Backend(GibRead));
    let e = &r.events[0];
    assert!(e.page_fault.is_none());
    assert!(!e.retired);
    assert_eq!(e.rbx, 0x80_0080_0000);
    assert!(
        e.memory_error
            .as_deref()
            .unwrap()
            .contains("UnsupportedPageSize"),
        "{:?}",
        e.memory_error
    );
    assert_eq!(e.rip, 0x40_0000);
    assert_eq!(e.rax, RAX0);
}

#[test]
fn offset_past_the_2_mib_page_reads_another_physical_frame() {
    let mut m = load([START, MIDDLE, END]);
    m.inject(Stage4HugePagesInjection::OffsetPastPage).unwrap();
    let r = m.run(16, |_| {});
    assert_eq!(r.stop, Stage4HugePagesStop::Completed);
    assert_eq!(r.events[2].rbx, 0xa0_0000);
    assert_eq!(r.events[2].rax, SENTINEL);
    assert_ne!(r.events[2].rax, END);
    // The neighbouring 2 MiB PDE (PD[5]) is the one that got A, not the first page's end qword.
    assert_eq!(r.events[2].entries[5], 0x40_0083 | NX | A);
    assert_eq!(writes(&r.events[2]), [(0x3028, 8)]);
}

#[test]
fn injections_of_the_other_case_are_refused() {
    use Stage4HugePagesCase::*;
    use Stage4HugePagesInjection as I;
    let mut m = load([START, MIDDLE, END]);
    assert!(m.inject(I::PdptePsHostError).is_err());
    m.select_case(GibNegative);
    assert!(m.inject(I::PdePsCleared).is_err());
    assert!(m.inject(I::OffsetPastPage).is_err());
    assert!(m.inject(I::None).is_ok());
    assert!(m.inject(I::PdptePsHostError).is_ok());
}

#[test]
fn budget_zero_to_seven_keeps_a_prefix_and_stops_with_budget_exhausted() {
    let full = load([START, MIDDLE, END]).run(16, |_| {});
    for budget in 0..=STAGE4_HUGE_PAGES_STEPS {
        let mut m = load([START, MIDDLE, END]);
        let report = m.run(budget, |_| {});
        assert_eq!(report.steps, budget);
        assert_eq!(report.retired, budget);
        assert_eq!(report.events, full.events[..budget as usize]);
        if budget < STAGE4_HUGE_PAGES_STEPS {
            assert_eq!(report.stop, Stage4HugePagesStop::BudgetExhausted);
        } else {
            assert_eq!(report.stop, Stage4HugePagesStop::Completed);
        }
        assert_eq!(report.serial.len(), usize::from(budget >= 6));
    }
    // The negative has one step: budget 0 runs none, budget 1 faults.
    let mut m = load([START, MIDDLE, END]);
    m.select_case(Stage4HugePagesCase::GibNegative);
    let r = m.run(0, |_| {});
    assert_eq!((r.stop, r.steps), (Stage4HugePagesStop::BudgetExhausted, 0));
    assert!(m.entries() == INITIAL, "no step, no walk");
}

#[test]
fn loader_rejects_any_other_image() {
    let good = image([START, MIDDLE, END]);
    assert!(matches!(
        load_stage4_huge_pages_probe(&good[1..]),
        Err(Stage4FlatProbeLoadError::ImageSize { .. })
    ));
    let mutate = |gpa: u64, bytes: &[u8]| {
        let mut b = good.clone();
        put(&mut b, gpa, bytes);
        b
    };
    // PDE[4] without PS (a 4 KiB table pointer), PDPT1[0] without PS, a changed instruction.
    for (gpa, bytes) in [
        (0x3020u64, (0x6003u64 | NX).to_le_bytes()),
        (0x5000, 0x3003u64.to_le_bytes()),
        (0x3020, (0x20_0003u64 | NX).to_le_bytes()),
        (0x3020, 0x20_00a3u64.to_le_bytes()),
    ] {
        match load_stage4_huge_pages_probe(&mutate(gpa, &bytes)) {
            Err(Stage4FlatProbeLoadError::Layout(Stage4FlatProbeLayoutError::PageTableEntry {
                gpa: g,
                ..
            })) => assert_eq!(g, gpa),
            other => panic!("expected a page-table rejection, got {:?}", other.err()),
        }
    }
    for gpa in [
        0x7000u64, 0x8000, 0x3f_ff00, 0x40_0000, 0x20_0800, 0x10_0000,
    ] {
        let mut b = good.clone();
        put(&mut b, gpa, &[0x91]);
        match load_stage4_huge_pages_probe(&b) {
            Err(Stage4FlatProbeLoadError::Layout(Stage4FlatProbeLayoutError::CodeBytes {
                gpa: page,
            })) => assert_eq!(page, gpa & !0xfff),
            other => panic!(
                "expected a page rejection at {gpa:#x}, got {:?}",
                other.err()
            ),
        }
    }
    // Colliding values: equal values, a value equal to the sentinel or to the stored value, and an
    // end value whose low byte equals the stored value's (the UART byte would not tell them apart).
    for values in [
        [START, START, END],
        [START, MIDDLE, START],
        [SENTINEL, MIDDLE, END],
        [START, W, END],
        [START, MIDDLE, 0x0f1e_2d3c_4b5a_69ab],
    ] {
        assert_eq!(
            load_stage4_huge_pages_probe(&image(values)).err(),
            Some(Stage4FlatProbeLoadError::Layout(
                Stage4FlatProbeLayoutError::DataQwordsEqual
            )),
            "{values:x?}"
        );
    }
}
