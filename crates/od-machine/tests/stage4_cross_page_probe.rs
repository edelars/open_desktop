use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{INTEGER_FLAGS, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, LoadedStage4CrossPageProbe, STAGE4_CROSS_PAGE_FINAL_RIP,
    STAGE4_CROSS_PAGE_IMAGE_BYTES, STAGE4_CROSS_PAGE_INITIAL_RAX, STAGE4_CROSS_PAGE_INSTRUCTIONS,
    STAGE4_CROSS_PAGE_PTES, STAGE4_CROSS_PAGE_STEP_RIPS, STAGE4_CROSS_PAGE_STEPS,
    STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_IMAGE_GPA, Stage4CrossPageEvent,
    Stage4CrossPageInjection, Stage4CrossPageOperand, Stage4CrossPageStep::*, Stage4CrossPageStop,
    Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError, load_stage4_cross_page_probe,
    step_paged_mov,
};

const NX: u64 = 1 << 63;
/// Bytes 11 22 33 | 44 55 66 77 88: three on data page 0, five on data page 1.
const VALUE_A: u64 = 0x8877_6655_4433_2211;
/// Same first-page bytes (11 22 33); every byte on the second page differs.
const VALUE_B: u64 = 0xa1b2_c3d4_e533_2211;
const SENTINEL: u64 = 0xdead_beef_cafe_babe;

fn put(b: &mut [u8], gpa: u64, v: &[u8]) {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    b[o..o + v.len()].copy_from_slice(v)
}

/// The fixed image written from the architecture (PTE formats, byte placement), not from the
/// module under test.
fn image(value: u64) -> Vec<u8> {
    let mut b = vec![0; STAGE4_CROSS_PAGE_IMAGE_BYTES];
    for (g, v) in [
        (0x1000, 0x2003),
        (0x2000, 0x3003),
        (0x3010, 0x4003),
        (0x3020, 0x5003),
        (0x4000, 0x7001),
        (0x4008, 0x9001),
        (0x5000, 0xb003 | NX),
        (0x5008, 0xd003 | NX),
    ] {
        put(&mut b, g, &u64::to_le_bytes(v));
    }
    put(&mut b, 0x7ffe, &[0x66, 0x8b]);
    put(
        &mut b,
        0x9000,
        &[
            0x03, 0x48, 0x8b, 0x03, 0x8b, 0x03, 0x66, 0x8b, 0x03, 0xee, 0xf4,
        ],
    );
    let raw = value.to_le_bytes();
    put(&mut b, 0xb000, &[0xa5; 8]);
    put(&mut b, 0xb800, &raw);
    put(&mut b, 0xb000 + 0xffd, &raw[..3]);
    put(&mut b, 0xd000, &raw[3..]);
    put(&mut b, 0xd000 + 0xff8, &[0x5a; 8]);
    b
}

fn load(value: u64) -> LoadedStage4CrossPageProbe {
    load_stage4_cross_page_probe(&image(value)).unwrap()
}

fn qword(m: &mut LoadedStage4CrossPageProbe, gpa: u64) -> u64 {
    m.bus_mut()
        .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
        .unwrap()
}

fn writes(e: &Stage4CrossPageEvent) -> Vec<u64> {
    e.committed_writes.iter().map(|r| r.start().get()).collect()
}

const A: u64 = 1 << 5;
const D: u64 = 1 << 6;

/// Leaf PTE state after the steps: A set on the pages that were walked, D never.
fn leaf(base: u64, accessed: bool) -> u64 {
    base | if accessed { A } else { 0 }
}

fn expected_rax(value: u64) -> [u64; 6] {
    let word = value & 0xffff;
    let dword = value & 0xffff_ffff;
    [
        (SENTINEL & !0xffff) | word,
        value,
        dword,
        dword,
        dword,
        dword,
    ]
}

#[test]
fn entry_state_is_the_fixed_loader_handoff_with_cpl0_ac_clear_and_nxe_set() {
    let mut m = load(VALUE_A);
    let s = m.state();
    assert_eq!(s.rip, 0x40_0ffe);
    assert_eq!(s.rflags, 2);
    assert_eq!(s.cr0, Some(0x8001_0033));
    assert_eq!(s.cr3, None);
    assert_eq!(s.cr4, Some(0x20));
    assert_eq!(s.gpr[Register::Rax as usize], STAGE4_CROSS_PAGE_INITIAL_RAX);
    assert_eq!(s.gpr[Register::Rbx as usize], 0x80_0ffd);
    assert_eq!(s.gpr[Register::Rdx as usize], 0x3f8);
    assert_eq!(m.cpl(), 0);
    assert!(!m.ac());
    assert!(m.nxe());
    assert_eq!(m.efer(), STAGE4_FLAT_PROBE_EFER);
    assert_eq!(m.efer() & (1 << 11), 1 << 11);
    assert_eq!(m.value(), VALUE_A);
    assert_eq!(m.ptes(), STAGE4_CROSS_PAGE_PTES);
    assert_eq!(m.ptes(), [0x7001, 0x9001, 0xb003 | NX, 0xd003 | NX]);
}

#[test]
fn success_case_fetches_across_code_frames_and_reads_across_data_frames() {
    let mut m = load(VALUE_A);
    let mut callbacks = Vec::new();
    let report = m.run(16, |r| callbacks.push(r.start().get()));
    assert_eq!(report.stop, Stage4CrossPageStop::Completed);
    assert_eq!(
        (report.steps, report.retired),
        (STAGE4_CROSS_PAGE_STEPS, STAGE4_CROSS_PAGE_INSTRUCTIONS)
    );
    assert_eq!(report.serial, [0x11]);
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(
        steps,
        [WordFetchCross, QwordRead, DwordRead, WordRead, Out, Halt]
    );

    // RIP after each retired instruction: 66 8B 03 (3), 48 8B 03 (3), 8B 03 (2), 66 8B 03 (3),
    // EE (1), HLT (1).
    let rips_after = [
        0x40_1001, 0x40_1004, 0x40_1006, 0x40_1009, 0x40_100a, 0x40_100b,
    ];
    let rax = expected_rax(VALUE_A);
    // Which pages the accumulated accesses have walked after each step (A set; D never).
    // Step 1 fetches code 0 and code 1 and reads data 0 (word at 0x800ffd/0x800ffe).
    // Step 2 is the first read that reaches data 1. Later steps walk nothing new.
    let accessed = [
        [true, true, true, false],
        [true, true, true, true],
        [true, true, true, true],
        [true, true, true, true],
        [true, true, true, true],
        [true, true, true, true],
    ];
    let bases = STAGE4_CROSS_PAGE_PTES;
    for (i, e) in report.events.iter().enumerate() {
        assert!(e.retired, "step {i} retired");
        assert_eq!(e.rip, rips_after[i], "RIP after step {i}");
        assert_eq!(e.rax, rax[i], "RAX after step {i}");
        assert_eq!(e.cpl, 0);
        assert_eq!(e.cr2, None, "no #PF, so no CR2");
        assert!(e.page_fault.is_none() && e.memory_error.is_none());
        let expected_ptes = [0, 1, 2, 3].map(|k| leaf(bases[k], accessed[i][k]));
        assert_eq!(e.ptes, expected_ptes, "leaf PTEs after step {i}");
        for pte in e.ptes {
            assert_eq!(pte & D, 0, "reads never set D (step {i})");
        }
    }
    // Committed A-bit writes: step 1 walks PML4E, PDPTE, PDE, code PTE 0, code PTE 1 (upper
    // entries are shared), then the data PDE and data PTE 0. Step 2 walks data PTE 1 only.
    assert_eq!(
        writes(&report.events[0]),
        [0x1000, 0x2000, 0x3010, 0x4000, 0x4008, 0x3020, 0x5000]
    );
    assert_eq!(writes(&report.events[1]), [0x5008]);
    for e in &report.events[2..] {
        assert!(e.committed_writes.is_empty());
    }
    assert_eq!(
        callbacks,
        [
            0x1000, 0x2000, 0x3010, 0x4000, 0x4008, 0x3020, 0x5000, 0x5008
        ]
    );
    let s = m.state();
    assert_eq!(s.rip, STAGE4_CROSS_PAGE_FINAL_RIP);
    assert_eq!(s.gpr[Register::Rax as usize], VALUE_A & 0xffff_ffff);
    assert_eq!(s.activity, Some(od_isa::CpuActivity::Halted));
    // The runner's instruction start addresses agree with the RIP evidence: each step starts
    // where the previous one ended, and the last one ends at the final RIP.
    assert_eq!(
        STAGE4_CROSS_PAGE_STEP_RIPS,
        [
            0x40_0ffe, 0x40_1001, 0x40_1004, 0x40_1006, 0x40_1009, 0x40_100a
        ]
    );
    for i in 0..6 {
        let next = STAGE4_CROSS_PAGE_STEP_RIPS
            .get(i + 1)
            .copied()
            .unwrap_or(STAGE4_CROSS_PAGE_FINAL_RIP);
        assert_eq!(
            report.events[i].rip, next,
            "step {i} ends where the next starts"
        );
    }
    // The data bytes themselves were not modified.
    assert_eq!(qword(&mut m, 0xb800), VALUE_A);
}

#[test]
fn second_page_value_drives_rax_and_first_page_bytes_do_not() {
    let a = {
        let mut m = load(VALUE_A);
        m.run(16, |_| {})
    };
    let b = {
        let mut m = load(VALUE_B);
        m.run(16, |_| {})
    };
    assert_eq!(a.stop, Stage4CrossPageStop::Completed);
    assert_eq!(b.stop, Stage4CrossPageStop::Completed);
    for (report, value) in [(&a, VALUE_A), (&b, VALUE_B)] {
        let rax = expected_rax(value);
        for (i, e) in report.events.iter().enumerate() {
            assert_eq!(e.rax, rax[i], "value {value:#x}, RAX after step {i}");
        }
        assert_eq!(report.serial, [0x11]);
    }
    // Step 1 reads only data page 0, which is identical for both values.
    assert_eq!(a.events[0].rax, b.events[0].rax);
    // From the first read of the second page on, the values differ.
    for i in 1..6 {
        assert_ne!(a.events[i].rax, b.events[i].rax, "step {i}");
    }
    assert_eq!(b.events[2].rax, 0xe533_2211);
}

#[test]
fn same_page_control_gives_the_same_values_without_touching_the_second_data_page() {
    let mut m = load(VALUE_A);
    m.select_operand(Stage4CrossPageOperand::SamePageControl);
    let report = m.run(16, |_| {});
    assert_eq!(report.stop, Stage4CrossPageStop::Completed);
    let rax = expected_rax(VALUE_A);
    for (i, e) in report.events.iter().enumerate() {
        assert_eq!(e.rax, rax[i], "control RAX after step {i}");
        assert_eq!(
            e.ptes[3], STAGE4_CROSS_PAGE_PTES[3],
            "data page 1 never walked"
        );
        assert_eq!(e.ptes[2] & A, A, "data page 0 walked");
    }
    // No walk of data PTE 1 in any step, so the 0x5008 write seen in the crossing run is gone.
    assert!(report.events.iter().all(|e| !writes(e).contains(&0x5008)));
    assert_eq!(report.serial, [0x11]);
}

#[test]
fn budget_zero_to_six_keeps_a_prefix_and_stops_with_budget_exhausted() {
    let full = load(VALUE_A).run(16, |_| {});
    for budget in 0..=STAGE4_CROSS_PAGE_STEPS {
        let mut m = load(VALUE_A);
        let report = m.run(budget, |_| {});
        assert_eq!(report.steps, budget);
        assert_eq!(report.retired, budget);
        assert_eq!(report.events.len(), budget as usize);
        assert_eq!(report.events[..], full.events[..budget as usize]);
        if budget < STAGE4_CROSS_PAGE_STEPS {
            assert_eq!(report.stop, Stage4CrossPageStop::BudgetExhausted);
        } else {
            assert_eq!(report.stop, Stage4CrossPageStop::Completed);
        }
    }
    // The UART byte appears only after the OUT step.
    assert!(load(VALUE_A).run(4, |_| {}).serial.is_empty());
    assert_eq!(load(VALUE_A).run(5, |_| {}).serial, [0x11]);
}

#[test]
fn injection_second_data_pte_not_present_faults_on_the_second_page_with_pfec_zero() {
    let mut m = load(VALUE_A);
    m.inject(Stage4CrossPageInjection::SecondDataPteNotPresent);
    let report = m.run(16, |_| {});
    // Step 1 retires (word read on data page 0); the qword read faults on page 1.
    assert_eq!(report.stop, Stage4CrossPageStop::PageFault(QwordRead));
    assert_eq!((report.steps, report.retired), (2, 1));
    let e = &report.events[1];
    assert!(!e.retired);
    assert_eq!(e.exit, "page_fault");
    let f = e.page_fault.as_ref().unwrap();
    // SDM Vol. 3A 4.7: P=0 (not present), W/R=0 (read), U/S=0 (supervisor), I/D=0 (data) -> 0.
    assert_eq!((f.vector, f.error_code), (14, Some(0x00)));
    assert_eq!(f.cr2, 0x80_1000, "first byte of the second page");
    assert_eq!(f.saved_rip, 0x40_1001);
    assert_eq!(f.access, "Read");
    // No retirement, RIP and RAX unchanged since step 1, nothing committed by this step.
    assert_eq!(e.rip, 0x40_1001);
    assert_eq!(e.rax, expected_rax(VALUE_A)[0]);
    assert_eq!(e.cr2, None);
    // A of the first data page is preserved; the not-present PTE is untouched; D is clear.
    assert_eq!(e.ptes[2], leaf(STAGE4_CROSS_PAGE_PTES[2], true));
    assert_eq!(e.ptes[3], 0);
    assert!(e.ptes.iter().all(|p| p & D == 0));
    assert!(report.serial.is_empty());
}

#[test]
fn injection_second_code_pte_nx_faults_on_the_fetch_before_any_operand_walk() {
    let mut m = load(VALUE_A);
    m.inject(Stage4CrossPageInjection::SecondCodePteNx);
    let report = m.run(16, |_| {});
    assert_eq!(report.stop, Stage4CrossPageStop::PageFault(WordFetchCross));
    assert_eq!((report.steps, report.retired), (1, 0));
    let e = &report.events[0];
    let f = e.page_fault.as_ref().unwrap();
    // SDM Vol. 3A 4.7: P=1 (present, rights violation), W/R=0, U/S=0, I/D=1 (NX fetch) -> 0x11.
    assert_eq!((f.vector, f.error_code), (14, Some(0x11)));
    assert_eq!(f.cr2, 0x40_1000, "the byte on the second code page");
    assert_eq!(f.saved_rip, 0x40_0ffe);
    assert_eq!(f.access, "Fetch");
    assert_eq!(e.rip, 0x40_0ffe);
    assert_eq!(e.rax, STAGE4_CROSS_PAGE_INITIAL_RAX);
    // No operand walk: both data PTEs keep A and D clear and no data entry was written.
    assert_eq!(e.ptes[2], STAGE4_CROSS_PAGE_PTES[2]);
    assert_eq!(e.ptes[3], STAGE4_CROSS_PAGE_PTES[3]);
    assert!(!writes(e).contains(&0x5000) && !writes(e).contains(&0x3020));
    assert!(e.ptes.iter().all(|p| p & D == 0));
    // The rejected fetch walk keeps A on the NX code PTE (accepted S4-11d/f behaviour).
    assert_eq!(e.ptes[1], 0x8000_0000_0000_9021);
    // The CPU CR2 is not committed in this model; the #PF record holds the CR2 value.
    assert_eq!(e.cr2, None);
    assert!(report.serial.is_empty());
}

#[test]
fn injection_swapped_data_frames_reads_wrong_bytes_without_any_fault() {
    let mut m = load(VALUE_A);
    m.inject(Stage4CrossPageInjection::DataFramesSwapped);
    let report = m.run(16, |_| {});
    // Every step retires, so only the values can reveal the broken loader.
    assert_eq!(report.stop, Stage4CrossPageStop::Completed);
    assert_eq!((report.steps, report.retired), (6, 6));
    assert!(report.events.iter().all(|e| e.page_fault.is_none()));
    let good = expected_rax(VALUE_A);
    // Page 0 now maps frame 0xd000, whose last bytes are the 0x5a decoy; page 1 maps 0xb000
    // whose first bytes are the 0xa5 decoy.
    let wrong = [
        (SENTINEL & !0xffff) | 0x5a5a,
        0xa5a5_a5a5_a55a_5a5a,
        0xa55a_5a5a,
        0xa55a_5a5a,
        0xa55a_5a5a,
        0xa55a_5a5a,
    ];
    for (i, e) in report.events.iter().enumerate() {
        assert_eq!(e.rax, wrong[i], "step {i}");
        assert_ne!(e.rax, good[i], "step {i} differs from the correct case");
    }
    assert_eq!(report.serial, [0x5a]);
}

#[test]
fn injection_second_data_page_in_ram_hole_is_rejected_without_retirement_or_fault() {
    let mut m = load(VALUE_A);
    m.inject(Stage4CrossPageInjection::SecondDataPageHole);
    let report = m.run(16, |_| {});
    assert_eq!(report.stop, Stage4CrossPageStop::Backend(QwordRead));
    assert_eq!((report.steps, report.retired), (2, 1));
    let e = &report.events[1];
    assert!(!e.retired);
    assert_eq!(e.exit, "backend_rejected");
    assert!(e.page_fault.is_none(), "a hole is not an architectural #PF");
    assert!(
        e.memory_error
            .as_deref()
            .unwrap()
            .contains("OperandPhysical")
    );
    // Both walks complete before the RAM admission, so the second PTE keeps its A bit.
    assert_eq!(e.ptes[3], 0x3_0003 | NX | A);
    assert_eq!(e.rip, 0x40_1001);
    assert_eq!(e.rax, expected_rax(VALUE_A)[0]);
    assert_eq!(e.ptes[2] & D, 0);
    assert!(report.serial.is_empty());
}

#[test]
fn the_four_injections_are_distinguishable_by_steps_retired_stop_and_pfec() {
    let mut seen = Vec::new();
    for injection in [
        Stage4CrossPageInjection::SecondDataPteNotPresent,
        Stage4CrossPageInjection::SecondCodePteNx,
        Stage4CrossPageInjection::DataFramesSwapped,
        Stage4CrossPageInjection::SecondDataPageHole,
    ] {
        let mut m = load(VALUE_A);
        m.inject(injection);
        let r = m.run(16, |_| {});
        let pfec = r
            .events
            .iter()
            .find_map(|e| e.page_fault.as_ref())
            .map(|f| f.error_code);
        let last_rax = r.events.last().unwrap().rax;
        seen.push((r.stop, r.steps, r.retired, pfec, last_rax));
    }
    let clean = {
        let mut m = load(VALUE_A);
        let r = m.run(16, |_| {});
        (
            r.stop,
            r.steps,
            r.retired,
            None,
            r.events.last().unwrap().rax,
        )
    };
    for (i, a) in seen.iter().enumerate() {
        assert_ne!(*a, clean, "injection {i} differs from the clean case");
        for b in &seen[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn qword_read_keeps_its_behaviour_when_nxe_is_set_in_efer() {
    // Precondition of S4-33d: the S4-11b qword path (48 8B 03) with a crossing operand behaves
    // the same under NXE=0 and NXE=1 when no page carries NX on the fetch path.
    let mut results = Vec::new();
    for efer in [STAGE4_FLAT_PROBE_EFER & !(1 << 11), STAGE4_FLAT_PROBE_EFER] {
        let mut m = load(VALUE_A);
        // With NXE=0 bit 63 of a PTE is reserved (SDM Vol. 3A 4.5), so both runs use data PTEs
        // without NX. RIP is placed on the qword load inside code page 1, operand across pages.
        for (gpa, pte) in [(0x5000, 0xb003_u64), (0x5008, 0xd003)] {
            m.bus_mut()
                .write_physical(
                    GuestPhysicalAddress::new(gpa),
                    AccessWidth::Qword,
                    pte,
                    |_| {},
                )
                .unwrap();
        }
        let mut state = m.state().clone();
        state.rip = 0x40_1001;
        let mut writes = Vec::new();
        let report = step_paged_mov(
            &mut state,
            m.bus_mut(),
            0x1000,
            efer,
            AccessContext {
                mode: InstructionMode::Long64,
                privilege: PrivilegeLevel::Ring0,
                origin: AccessOrigin::Explicit,
            },
            INTEGER_FLAGS,
            |r| writes.push(r.start().get()),
        )
        .unwrap();
        assert_eq!(report.cpu.instructions_executed, 1, "efer {efer:#x}");
        assert_eq!(state.gpr[Register::Rax as usize], VALUE_A, "efer {efer:#x}");
        results.push((
            state.rip,
            state.gpr[Register::Rax as usize],
            writes,
            m.ptes(),
        ));
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn loader_rejects_wrong_size_corrupted_tables_code_and_indistinguishable_values() {
    let good = image(VALUE_A);
    assert!(matches!(
        load_stage4_cross_page_probe(&good[..good.len() - 1]),
        Err(Stage4FlatProbeLoadError::ImageSize { .. })
    ));
    // Corrupted second data PTE: made adjacent to the first data frame.
    let mut bad = good.clone();
    put(&mut bad, 0x5008, &u64::to_le_bytes(0xc003 | NX));
    assert!(matches!(
        load_stage4_cross_page_probe(&bad),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry { gpa: 0x5008, .. }
        ))
    ));
    // Corrupted instruction in the second code frame.
    let mut bad = good.clone();
    put(&mut bad, 0x9001, &[0x90]);
    assert!(matches!(
        load_stage4_cross_page_probe(&bad),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::CodeBytes { gpa: 0x9000 }
        ))
    ));
    // The image has no handler or other frame that could hide extra mappings.
    let mut bad = good.clone();
    put(&mut bad, 0x8000, &[1]);
    assert!(load_stage4_cross_page_probe(&bad).is_err());
    // The control copy must equal the value read across the boundary.
    let mut bad = good.clone();
    put(&mut bad, 0xb800, &[0]);
    assert!(load_stage4_cross_page_probe(&bad).is_err());
    // A value whose upper dword is zero would make the qword and dword steps indistinguishable.
    assert!(matches!(
        load_stage4_cross_page_probe(&image(0x4433_2211)),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::DataQwordsEqual
        ))
    ));
}
