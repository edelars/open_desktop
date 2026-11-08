use od_isa::execute::TranslationContextChange;
use od_machine::{
    AccessWidth, GuestPhysicalAddress, PagedProbeExit, STAGE4_CR3_MIXED_PROBE_IMAGE_BYTES,
    STAGE4_CR3_MIXED_PROBE_IMAGE_GPA, STAGE4_CR3_MIXED_PROBE_ROOT_A, STAGE4_CR3_MIXED_PROBE_ROOT_B,
    Stage4Cr3MixedProbeLoadError, load_stage4_cr3_mixed_probe,
};

const BASE: usize = STAGE4_CR3_MIXED_PROBE_IMAGE_GPA as usize;

/// The fixed image, spelled out independently of the loader (this is the spec).
fn image() -> Vec<u8> {
    let mut bytes = vec![0u8; STAGE4_CR3_MIXED_PROBE_IMAGE_BYTES];
    for (gpa, value) in [
        (0x1000usize, 0x2003u64), // root A PML4 -> PDPT
        (0x2000, 0x3003),         // PDPT -> PD
        (0x3010, 0x4003),         // PD index 2 -> code PT (A)
        (0x4000, 0xb001),         // code PT: GVA 0x400000 -> GPA 0xb000, present only
        (0x5000, 0x6003),         // root B PML4 -> PDPT
        (0x6000, 0x7003),         // PDPT -> PD
        (0x7010, 0x20_0083),      // PD index 2 -> 2 MiB leaf at GPA 0x200000
        (0x20_1000, 0x1000),      // data: root A, loaded by the guest
    ] {
        bytes[gpa - BASE..gpa - BASE + 8].copy_from_slice(&value.to_le_bytes());
    }
    let code_a = [0x0f, 0x22, 0xd8, 0x0f, 0x0b, 0, 0, 0, 0, 0x0f, 0x0b];
    let code_b = [0, 0, 0, 0x48, 0x8b, 0x03, 0x0f, 0x22, 0xd8, 0x0f, 0x0b];
    bytes[0xb000 - BASE..0xb000 - BASE + code_a.len()].copy_from_slice(&code_a);
    bytes[0x20_0000 - BASE..0x20_0000 - BASE + code_b.len()].copy_from_slice(&code_b);
    bytes
}

fn read(probe: &mut od_machine::LoadedStage4Cr3MixedProbe, gpa: u64) -> u64 {
    probe
        .bus_mut()
        .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
        .unwrap()
}

fn write(probe: &mut od_machine::LoadedStage4Cr3MixedProbe, gpa: u64, value: u64) {
    probe
        .bus_mut()
        .write_physical(
            GuestPhysicalAddress::new(gpa),
            AccessWidth::Qword,
            value,
            |_| {},
        )
        .unwrap();
}

#[test]
fn builder_artifact_matches_the_fixed_image_when_provided() {
    // `python3 tools/build/build_stage4_cr3_mixed_probe.py --output-dir work/stage4/<dir>`
    // then run with STAGE4_CR3_MIXED_PROBE_IMAGE=<abs path to probe.bin>.
    let Some(path) = std::env::var_os("STAGE4_CR3_MIXED_PROBE_IMAGE") else {
        eprintln!("STAGE4_CR3_MIXED_PROBE_IMAGE not set: builder artifact check skipped");
        return;
    };
    let artifact = std::fs::read(path).expect("read STAGE4_CR3_MIXED_PROBE_IMAGE");
    assert_eq!(
        artifact,
        image(),
        "builder output differs from the fixed image"
    );
    assert!(load_stage4_cr3_mixed_probe(&artifact).is_ok());
}

#[test]
fn loader_accepts_only_the_exact_image_and_places_it_verbatim() {
    let image = image();
    for offset in [
        0usize,
        0x2010,
        0x4000,
        0x6000,
        0xa000,
        0xb000,
        0x1ff000,
        0x1ff000 + 0xfff,
        0x200000,
        0x200fff,
    ] {
        let mut bad = image.clone();
        bad[offset.min(image.len() - 1)] ^= 1;
        assert!(
            matches!(
                load_stage4_cr3_mixed_probe(&bad),
                Err(Stage4Cr3MixedProbeLoadError::InvalidImage)
            ),
            "offset {offset:#x}"
        );
    }
    let mut longer = image.clone();
    longer.push(0);
    for wrong_length in [&image[..image.len() - 1], &longer[..], &[]] {
        assert!(matches!(
            load_stage4_cr3_mixed_probe(wrong_length),
            Err(Stage4Cr3MixedProbeLoadError::InvalidImage)
        ));
    }

    let mut probe = load_stage4_cr3_mixed_probe(&image).unwrap();
    for gpa in (0x1000..0x202000u64).step_by(8) {
        let offset = gpa as usize - BASE;
        assert_eq!(
            read(&mut probe, gpa),
            u64::from_le_bytes(image[offset..offset + 8].try_into().unwrap()),
            "{gpa:#x}"
        );
    }
    let state = probe.state();
    assert_eq!(state.cr3, Some(STAGE4_CR3_MIXED_PROBE_ROOT_A));
    assert_eq!(
        state.gpr[od_isa::Register::Rax as usize],
        STAGE4_CR3_MIXED_PROBE_ROOT_B
    );
    assert_eq!(
        (state.rip, state.gpr[od_isa::Register::Rbx as usize]),
        (0x40_0000, 0x40_1000)
    );
    assert_eq!(probe.rflags_defined(), od_isa::INTEGER_FLAGS);
    let state = probe.state();
    assert_eq!((state.cr0, state.cr4), (Some(0x8001_0033), Some(0x20)));
    assert_eq!(state.rflags, 2);
    assert_eq!(state.activity, Some(od_isa::CpuActivity::Running));
    assert_eq!(state.sti_shadow, Some(od_isa::StiShadow::Clear));
}

#[test]
fn whole_and_split_execution_match_and_switch_roots_twice() {
    let image = image();
    let (mut whole, mut split) = (
        load_stage4_cr3_mixed_probe(&image).unwrap(),
        load_stage4_cr3_mixed_probe(&image).unwrap(),
    );
    let (mut whole_writes, mut split_writes) = (Vec::new(), Vec::new());
    let whole_report = whole.run(
        3,
        || false,
        |r| whole_writes.push((r.start().get(), r.len())),
    );
    let mut split_changes = Vec::new();
    let mut retired = 0;
    for step in 0..3 {
        let report = split.run(
            1,
            || false,
            |r| split_writes.push((r.start().get(), r.len())),
        );
        assert_eq!(report.exit, PagedProbeExit::BudgetExhausted);
        retired += report.instructions_executed;
        if step == 0 {
            // After one retired MOV CR3 the switch is reported and nothing has been
            // fetched or walked under root B yet.
            assert_eq!(report.translation_context_changes.len(), 1);
            assert!(!split_writes.contains(&(0x5000, 8)));
            assert!(!split_writes.contains(&(0x7010, 8)));
        }
        split_changes.extend(report.translation_context_changes);
    }
    assert_eq!(whole_report.exit, PagedProbeExit::BudgetExhausted);
    assert_eq!(whole_report.instructions_executed, 3);
    assert_eq!(retired, 3);
    let expected_changes = vec![
        TranslationContextChange::Cr3Write {
            old_cr3: 0x1000,
            new_cr3: 0x5000,
        },
        TranslationContextChange::Cr3Write {
            old_cr3: 0x5000,
            new_cr3: 0x1000,
        },
    ];
    assert_eq!(whole_report.translation_context_changes, expected_changes);
    assert_eq!(split_changes, expected_changes);
    assert_eq!(whole.state(), split.state());
    assert_eq!(whole.rflags_defined(), split.rflags_defined());
    assert_eq!(whole_writes, split_writes);
    assert_eq!(whole.state().cr3, Some(0x1000));
    assert_eq!(whole.state().gpr[od_isa::Register::Rax as usize], 0x1000);
    assert_eq!(whole.state().rip, 0x40_0009);
    // Accessed bits only: code PTE (A), root-B tables and the 2 MiB PDE; no stores happened.
    for gpa in [0x4000, 0x5000, 0x6000, 0x7010] {
        assert_eq!(read(&mut whole, gpa) & 0x60, 0x20, "{gpa:#x}");
    }
    for gpa in (0..0x40_0000u64).step_by(8) {
        assert_eq!(read(&mut whole, gpa), read(&mut split, gpa), "{gpa:#x}");
    }
}

#[test]
fn nx_and_non_present_two_mib_leaf_fault_after_the_switch() {
    for (pde, error_code, sets_pde_accessed) in
        [((1u64 << 63) | 0x20_0083, 0x11u32, true), (0, 0x10, false)]
    {
        let mut probe = load_stage4_cr3_mixed_probe(&image()).unwrap();
        let first = probe.run(1, || false, |_| {});
        assert_eq!(first.instructions_executed, 1);
        assert_eq!(probe.state().cr3, Some(0x5000));
        write(&mut probe, 0x7010, pde);
        let result = probe.run(1, || false, |_| {});
        let PagedProbeExit::Fault(fault) = &result.exit else {
            panic!("pde {pde:#x}: {:?}", result.exit)
        };
        assert_eq!(fault.vector, 14);
        assert_eq!(fault.error_code, Some(error_code));
        assert_eq!(fault.linear_address, Some(0x40_0003));
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(probe.state().cr3, Some(0x5000));
        assert_eq!(probe.state().rip, 0x40_0003);
        assert!(result.translation_context_changes.is_empty());
        // The walk's accessed-bit prefix stays committed.
        assert_ne!(read(&mut probe, 0x5000) & 0x20, 0);
        assert_ne!(read(&mut probe, 0x6000) & 0x20, 0);
        assert_eq!(read(&mut probe, 0x7010) & 0x20 != 0, sets_pde_accessed);
    }
}

#[test]
fn larger_budget_reaches_the_ud2_sentinel_with_a_precise_ud() {
    let mut probe = load_stage4_cr3_mixed_probe(&image()).unwrap();
    let report = probe.run(10, || false, |_| {});
    let PagedProbeExit::Fault(fault) = &report.exit else {
        panic!("{:?}", report.exit)
    };
    assert_eq!((fault.vector, fault.error_code), (6, None));
    assert_eq!(report.instructions_executed, 3);
    assert_eq!(report.translation_context_changes.len(), 2);
    assert_eq!(probe.state().cr3, Some(0x1000));
    assert_eq!(probe.state().rip, 0x40_0009);
    // The sentinel does not retire and repeats on every further run.
    let again = probe.run(1, || false, |_| {});
    assert!(matches!(&again.exit, PagedProbeExit::Fault(f) if f.vector == 6));
    assert_eq!(again.instructions_executed, 0);
    assert_eq!(probe.state().rip, 0x40_0009);
}
