use super::*;

#[test]
fn cvtps2pd_m64_schema_is_strict_and_keeps_controls_separate() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37by_cvtps2pd_m64_snan_subnormal.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongCvtps2pdMemoryRaxV1);

    let mut missing_controls = fixture.clone();
    missing_controls.cvtps2pd_m64_controls = None;
    assert!(missing_controls.validate().is_err());
    assert!(Fixture::from_json(&serde_json::to_vec(&missing_controls).unwrap()).is_err());

    let mut foreign_register_controls = fixture.clone();
    foreign_register_controls.cvtps2pd_controls = Some(Cvtps2pdControlSnapshot {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
        cr4_osxmmexcpt: true,
    });
    assert!(foreign_register_controls.validate().is_err());
    assert!(Fixture::from_json(&serde_json::to_vec(&foreign_register_controls).unwrap()).is_err());

    for invalid_mxcsr in [0x0000_cfc0, 0x0000_cf00, 0x0000_ce80] {
        let mut invalid = fixture.clone();
        invalid.initial_state.sse.as_mut().unwrap().mxcsr = HexU64(invalid_mxcsr);
        invalid.expected_exit = ExpectedExit::Unsupported;
        assert!(invalid.validate().is_ok());
    }
    let mut ac_set = fixture.clone();
    ac_set.initial_state.rflags = HexU64(2 | (1 << 18));
    ac_set.expected_exit = ExpectedExit::Unsupported;
    assert!(ac_set.validate().is_ok());
}

#[test]
fn maxps_m128_schema_is_exact_sse_only_and_rejects_tampering() {
    let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37h_maxps_masked_register.case.json"
    ))
    .unwrap();
    value["case_id"] = serde_json::json!("s2_37da_maxps_memory_rax_schema");
    value["profile"] = serde_json::json!("s2-long-maxps-memory-rax-v1");
    value["initial_state"]["gpr"][0] = serde_json::json!("0x0000000000200000");
    value["stop_rip"] = serde_json::json!("0x0000000000100003");
    value["expected_exit"] = serde_json::json!("stop_address");
    value["maxps_memory_rax_controls"] = serde_json::json!({
        "sse_available": true,
        "cr0_em": false,
        "cr0_ts": false,
        "cr4_osfxsr": true
    });
    let code = serde_json::json!({
        "region_id": "code",
        "guest_address": "0x0000000000100000",
        "length": 3,
        "permissions": {"read": true, "write": false, "execute": true},
        "initial_bytes_hex": "0f5f00",
        "comparison_mask_hex": "ffffff"
    });
    let data = serde_json::json!({
        "region_id": "data",
        "guest_address": "0x0000000000200000",
        "length": 16,
        "permissions": {"read": true, "write": false, "execute": false},
        "initial_bytes_hex": "0000803f000000400000404000008040",
        "comparison_mask_hex": "ffffffffffffffffffffffffffffffff"
    });
    value["memory"] = serde_json::json!([code, data]);

    let fixture = Fixture::from_json(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .fixture;
    assert_eq!(fixture.profile, Profile::LongMaxpsMemoryRaxV1);
    assert_eq!(
        fixture.maxps_memory_rax_controls.unwrap().sse_available,
        true
    );

    let mut missing_controls = fixture.clone();
    missing_controls.maxps_memory_rax_controls = None;
    assert!(missing_controls.validate().is_err());

    for bytes in ["660f5f00", "410f5f00", "670f5f00", "0f5f01", "0f5f00c0"] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        assert!(invalid.validate().is_err(), "accepted {bytes}");
    }

    let mut wrong_feature = fixture.clone();
    wrong_feature
        .maxps_memory_rax_controls
        .as_mut()
        .unwrap()
        .sse_available = false;
    assert!(wrong_feature.validate().is_err());

    let mut unsupported_mxcsr = fixture.clone();
    unsupported_mxcsr.initial_state.sse.as_mut().unwrap().mxcsr = HexU64(0x1f00);
    assert!(unsupported_mxcsr.validate().is_err());

    let mut foreign_control = value.clone();
    foreign_control["maxps_memory_rax_controls"]["sse2_available"] = serde_json::json!(true);
    assert!(Fixture::from_json(&serde_json::to_vec(&foreign_control).unwrap()).is_err());
}

#[test]
fn cli_rflags_comparison_includes_if_and_iopl_without_expanding_definedness() {
    let mask = INTEGER_FLAGS | (1 << 9) | (3 << 12);
    let initial = INTEGER_FLAGS | (2 << 12);
    assert!(rflags_differ(
        Profile::LongCliV1,
        mask,
        initial,
        initial ^ (1 << 9)
    ));
    assert!(rflags_differ(
        Profile::LongCliV1,
        mask,
        initial,
        initial ^ (1 << 12)
    ));
    assert!(!rflags_differ(
        Profile::LongCliV1,
        mask,
        initial,
        initial ^ (1 << 18)
    ));
    assert_eq!(INTEGER_FLAGS & ((1 << 9) | (3 << 12)), 0);
    assert!(!rflags_differ(
        Profile::LongIntegerV1,
        INTEGER_FLAGS,
        initial,
        initial ^ (1 << 9)
    ));
}

#[test]
fn cvttpd2pi_schema_requires_its_exact_four_byte_masked_profile() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37bl_cvttpd2pi_mm0_xmm1_masked.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongCvttpd2piRegisterMaskedV1);

    let mut missing_controls = fixture.clone();
    missing_controls.cvttpd2pi_register_controls = None;
    assert!(missing_controls.validate().is_err());

    let mut wrong_instruction = fixture.clone();
    wrong_instruction.memory[0].initial_bytes_hex = "660f2dc1".into();
    assert!(wrong_instruction.validate().is_err());

    let mut wrong_budget = fixture.clone();
    wrong_budget.instruction_budget = 2;
    assert!(wrong_budget.validate().is_err());

    let mut foreign_controls = fixture;
    foreign_controls.cvtpd2pi_register_controls = Some(Cvtpd2piControlSnapshot {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
    });
    assert!(foreign_controls.validate().is_err());
}

#[test]
fn cvtdq2ps_memory_pm_zero_schema_keeps_prerequisite_fault_priority() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37bt_cvtdq2ps_pm_clear_refusal.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.expected_exit, ExpectedExit::Unsupported);
    assert!(fixture.validate().is_ok());

    let mut missing_sse2_and_ts = fixture;
    let controls = missing_sse2_and_ts
        .cvtdq2ps_memory_controls
        .as_mut()
        .expect("literal carries memory-profile controls");
    controls.sse2_available = false;
    controls.cr0_ts = true;
    missing_sse2_and_ts.expected_exit = ExpectedExit::Exception;

    assert!(missing_sse2_and_ts.validate().is_ok());
    assert!(Fixture::from_json(&missing_sse2_and_ts.to_json().unwrap()).is_ok());
}

#[test]
fn iretq_cs_ss_snapshot_fields_are_opt_in_and_lossless() {
    let cpu = od_isa::LongModeState {
        cr4: None,
        cs_selector: Some(8),
        cs_cache: Some(od_isa::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 0xa,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(od_isa::DataSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0xc,
        }),
        gdtr: Some(od_isa::state::Gdtr {
            base: 0x200000,
            limit: 0x17,
        }),
        sti_shadow: Some(od_isa::StiShadow::Clear),
        activity: Some(od_isa::CpuActivity::Running),
        ..od_isa::LongModeState::default()
    };
    assert!(IntegerState::try_from(&cpu).is_err());
    assert!(IntegerState::from_cpu_for(&cpu, Profile::LongIntegerV1).is_err());
    assert!(od_isa::CpuState::try_from(&cpu).is_err());
    let projected = IntegerState::from_cpu_for(&cpu, Profile::LongIretqCpl0ValidV1).unwrap();
    let wire = serde_json::to_value(&projected).unwrap();
    assert_eq!(wire["cs_selector"], "0x0000000000000008");
    assert_eq!(wire["ss_cache_access"], "0x0000000000000093");
    let restored = projected.to_cpu_for(Profile::LongIretqCpl0ValidV1).unwrap();
    assert_eq!(restored.cs_cache, cpu.cs_cache);
    assert_eq!(restored.ss_cache, cpu.ss_cache);
    assert_eq!(restored.gdtr, cpu.gdtr);
}

#[test]
fn iretq_literal_fixture_matrix_is_schema_bounded() {
    for fixture in [
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_27k_iretq_if0_iopl0.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_27k_iretq_if0_iopl3.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_27k_iretq_if1_iopl0.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_27k_iretq_if1_iopl3.case.json")
            .as_slice(),
    ] {
        let loaded = Fixture::from_json(fixture).unwrap();
        assert_eq!(loaded.fixture.profile, Profile::LongIretqCpl0ValidV1);
    }
    let mut old = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_27j_hlt_cpl0_iopl0_if1.case.json"
    ))
    .unwrap()
    .fixture;
    old.initial_state.cs_selector = Some(HexU64(8));
    assert!(old.to_json().is_err());
}

#[test]
fn iretq_fixture_requires_exact_selected_cs_and_ss_descriptors() {
    let base = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_27k_iretq_if0_iopl0.case.json"
    ))
    .unwrap()
    .fixture;
    let mutations = [
        (13, 0x99), // CS data instead of code
        (13, 0x1b), // CS not present
        (13, 0x9a), // CS accessed bit clear
        (14, 0x8f), // CS L bit clear
        (21, 0x9b), // SS code instead of writable data
        (21, 0x13), // SS not present
        (21, 0x92), // SS accessed bit clear
        (22, 0x8f), // SS D/B bit clear
    ];
    for (offset, byte) in mutations {
        let mut fixture = base.clone();
        let gdt = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "gdt")
            .unwrap();
        let mut bytes = decode_bytes(&gdt.initial_bytes_hex).unwrap();
        bytes[offset] = byte;
        gdt.initial_bytes_hex = bytes.iter().map(|value| format!("{value:02x}")).collect();
        assert!(
            fixture.validate().is_err(),
            "accepted GDT mutation at {offset}"
        );
    }
}

#[test]
fn cli_fixture_cpl_and_system_flag_mask_are_profile_local() {
    let bytes =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_27h_cli_cpl0_iopl0_if0.case.json");
    let loaded = Fixture::from_json(bytes).unwrap();
    assert_eq!(loaded.fixture.cli_cpl, Some(CliCpl::Cpl0));
    assert_eq!(serde_json::to_value(&loaded.fixture).unwrap()["cli_cpl"], 0);

    let mut bad_cpl = serde_json::to_value(&loaded.fixture).unwrap();
    bad_cpl["cli_cpl"] = serde_json::json!(4);
    assert!(Fixture::from_json(&serde_json::to_vec(&bad_cpl).unwrap()).is_err());

    let mut old = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_27g_swapgs_once.case.json"
    ))
    .unwrap()
    .fixture;
    old.cli_cpl = Some(CliCpl::Cpl3);
    assert!(old.validate().is_err());
    old.cli_cpl = None;
    let old_json = serde_json::to_value(&old).unwrap();
    assert!(old_json.get("cli_cpl").is_none());
    old.rflags_mask = HexU64(old.rflags_mask.get() | (1 << 9));
    assert!(old.validate().is_err());
}

#[test]
fn swapgs_kernel_gs_base_preserves_some_zero_and_old_wire_shape() {
    let mut cpu = od_isa::LongModeState::default();
    cpu.kernel_gs_base = Some(0);
    let swapgs = IntegerState::from_cpu_for(&cpu, Profile::LongSwapgsV1).unwrap();
    let json = serde_json::to_value(&swapgs).unwrap();
    assert_eq!(json["kernel_gs_base"], "0x0000000000000000");
    let roundtrip: IntegerState = serde_json::from_value(json).unwrap();
    assert_eq!(
        roundtrip
            .to_cpu_for(Profile::LongSwapgsV1)
            .unwrap()
            .kernel_gs_base,
        Some(0)
    );
    assert!(roundtrip.to_cpu_for(Profile::LongIntegerV1).is_err());
    assert!(IntegerState::from_cpu_for(&cpu, Profile::LongIntegerV1).is_err());

    cpu.kernel_gs_base = None;
    let legacy = IntegerState::from_cpu_for(&cpu, Profile::LongIntegerV1).unwrap();
    let legacy_json = serde_json::to_value(legacy).unwrap();
    assert!(legacy_json.get("kernel_gs_base").is_none());
    assert!(IntegerState::from_cpu_for(&cpu, Profile::LongSwapgsV1).is_err());
}

#[test]
fn swapgs_fixture_profile_accepts_exactly_the_three_program_shapes() {
    for case_id in [
        "s2_27g_swapgs_once",
        "s2_27g_swapgs_double",
        "s2_27g_swapgs_gs_load_fault",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../tests/fixtures/x86/stage2/{case_id}.case.json"
        ));
        let fixture = Fixture::from_json(&std::fs::read(path).unwrap())
            .unwrap()
            .fixture;
        assert_eq!(fixture.profile, Profile::LongSwapgsV1);
        let mut lock = fixture.clone();
        lock.memory[0].initial_bytes_hex = "f00f01f8".into();
        lock.memory[0].length = 4;
        lock.memory[0].comparison_mask_hex = "ff".repeat(4);
        assert!(lock.validate().is_err(), "LOCK: {case_id}");
        let mut neighbor = fixture.clone();
        neighbor.memory[0].initial_bytes_hex = "0f01f9".into();
        neighbor.memory[0].length = 3;
        neighbor.memory[0].comparison_mask_hex = "ff".repeat(3);
        assert!(neighbor.validate().is_err(), "neighbor: {case_id}");
        let mut wrong_budget = fixture;
        wrong_budget.instruction_budget += 1;
        assert!(wrong_budget.validate().is_err(), "budget: {case_id}");
    }
}

#[test]
fn kernel_gs_base_fixture_profile_requires_both_bases_and_exact_programs() {
    for case_id in [
        "s2_26g_kernel_gs_base_rdmsr",
        "s2_26g_kernel_gs_base_wrmsr",
        "s2_26g_kernel_gs_base_wrmsr_swapgs_rdmsr",
        "s2_26g_kernel_gs_base_wrmsr_swapgs_gs_load",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../tests/fixtures/x86/stage2/{case_id}.case.json"
        ));
        let fixture = Fixture::from_json(&std::fs::read(path).unwrap())
            .unwrap()
            .fixture;
        assert_eq!(fixture.profile, Profile::LongKernelGsBaseV1);
        let mut missing = fixture.clone();
        missing.initial_state.kernel_gs_base = None;
        assert!(
            missing.validate().is_err(),
            "missing kernel GS base: {case_id}"
        );
        let mut prefix = fixture.clone();
        prefix.memory[0].initial_bytes_hex.insert_str(0, "48");
        prefix.memory[0].length += 1;
        prefix.memory[0].comparison_mask_hex.insert_str(0, "ff");
        assert!(prefix.validate().is_err(), "REX prefix: {case_id}");
        let mut wrong_budget = fixture.clone();
        wrong_budget.instruction_budget += 1;
        assert!(wrong_budget.validate().is_err(), "budget: {case_id}");
        let mut some_zero = fixture;
        some_zero.initial_state.kernel_gs_base = Some(HexU64(0));
        assert!(
            some_zero.validate().is_ok(),
            "Some(0) is present state: {case_id}"
        );
    }
}

fn loaded() -> LoadedFixture {
    let old = crate::case::Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
    ))
    .unwrap();
    let converted = Fixture::convert_v1(&old).unwrap();
    Fixture::from_json(&converted.to_json().unwrap()).unwrap()
}

#[test]
fn masked_subss_and_divss_memory_permission_rules_remain_unchanged() {
    let mut subss = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37dk_subss_m32_halfway.case.json"
    ))
    .unwrap()
    .fixture;
    subss
        .memory
        .iter_mut()
        .find(|region| region.region_id == "data")
        .unwrap()
        .permissions
        .write = true;
    assert!(subss.validate().is_ok());

    let mut mulss = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37dm_mulss_m32_memory_rax.case.json"
    ))
    .unwrap()
    .fixture;
    mulss
        .memory
        .iter_mut()
        .find(|region| region.region_id == "data")
        .unwrap()
        .permissions
        .write = true;
    assert!(mulss.validate().is_ok());

    let mut divss = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37do_divss_m32_memory_rax.case.json"
    ))
    .unwrap()
    .fixture;
    divss
        .memory
        .iter_mut()
        .find(|region| region.region_id == "data")
        .unwrap()
        .permissions
        .write = true;
    assert!(divss.validate().is_err());
}

fn result(input: &LoadedFixture) -> RunResult {
    let mut state = input.fixture.initial_state.clone();
    state.rip = input.fixture.stop_rip.unwrap();
    RunResult {
        schema_version: 2,
        case_id: input.fixture.case_id.clone(),
        input_sha256: input.input_sha256.clone(),
        profile: input.fixture.profile,
        backend: Backend::Interpreter,
        outcome: Outcome::StopAddress {},
        instructions_executed: Observation::Available { value: HexU64(1) },
        snapshot: Observation::Available {
            value: Snapshot {
                state,
                rflags_defined: HexU64(INTEGER_FLAGS),
                memory: input
                    .fixture
                    .memory
                    .iter()
                    .map(|r| MemoryRegionSnapshot {
                        region_id: r.region_id.clone(),
                        guest_address: r.guest_address,
                        bytes_hex: r.initial_bytes_hex.clone(),
                    })
                    .collect(),
                xorpd_prerequisites: input.fixture.xorpd_prerequisites,
                addss_memory_rax_controls: input.fixture.addss_memory_rax_controls,
                addss_memory_rax_unmasked_controls: input
                    .fixture
                    .addss_memory_rax_unmasked_controls,
                subss_memory_rax_unmasked_controls: input
                    .fixture
                    .subss_memory_rax_unmasked_controls,
                addsd_memory_rax_controls: input.fixture.addsd_memory_rax_controls,
                addsd_memory_rax_unmasked_controls: input
                    .fixture
                    .addsd_memory_rax_unmasked_controls,
                minss_invalid_controls: input.fixture.minss_invalid_controls,
                maxss_unmasked_controls: input.fixture.maxss_unmasked_controls,
                maxss_memory_unmasked_rax_controls: input
                    .fixture
                    .maxss_memory_unmasked_rax_controls,
                minsd_unmasked_controls: input.fixture.minsd_unmasked_controls,
                maxsd_unmasked_controls: input.fixture.maxsd_unmasked_controls,
                minpd_unmasked_controls: input.fixture.minpd_unmasked_controls,
                minps_unmasked_controls: input.fixture.minps_unmasked_controls,
                maxps_unmasked_controls: input.fixture.maxps_unmasked_controls,
                maxpd_unmasked_controls: input.fixture.maxpd_unmasked_controls,
                cvtdq2pd_controls: input.fixture.cvtdq2pd_controls,
                cvtdq2pd_memory_controls: input.fixture.cvtdq2pd_memory_controls,
                cvtdq2ps_controls: input.fixture.cvtdq2ps_controls,
                cvtdq2ps_memory_controls: input.fixture.cvtdq2ps_memory_controls,
                cvtps2dq_controls: input.fixture.cvtps2dq_controls,
                cvttps2dq_controls: input.fixture.cvttps2dq_controls,
                cvttps2dq_m128_controls: input.fixture.cvttps2dq_m128_controls,
                cvtps2dq_m128_controls: input.fixture.cvtps2dq_m128_controls,
                cvtpd2dq_controls: input.fixture.cvtpd2dq_controls,
                cvttpd2dq_controls: input.fixture.cvttpd2dq_controls,
                cvttpd2dq_m128_controls: input.fixture.cvttpd2dq_m128_controls,
                cvtpd2dq_m128_controls: input.fixture.cvtpd2dq_m128_controls,
                sqrtss_controls: input.fixture.sqrtss_controls,
                sqrtsd_controls: input.fixture.sqrtsd_controls,
                sqrtsd_memory_rax_unmasked_controls: input
                    .fixture
                    .sqrtsd_memory_rax_unmasked_controls,
                sqrtps_controls: input.fixture.sqrtps_controls,
                sqrtpd_controls: input.fixture.sqrtpd_controls,
                subpd_controls: input.fixture.subpd_controls,
                mulpd_controls: input.fixture.mulpd_controls,
                addpd_controls: input.fixture.addpd_controls,
                addpd_memory_rax_unmasked_controls: input
                    .fixture
                    .addpd_memory_rax_unmasked_controls,
                cvtpd2ps_controls: input.fixture.cvtpd2ps_controls,
                addsd_controls: input.fixture.addsd_controls,
                addss_controls: input.fixture.addss_controls,
                addps_controls: input.fixture.addps_controls,
                divpd_controls: input.fixture.divpd_controls,
                divsd_controls: input.fixture.divsd_controls,
                divsd_memory_rax_unmasked_controls: input
                    .fixture
                    .divsd_memory_rax_unmasked_controls,
                divps_controls: input.fixture.divps_controls,
                divss_controls: input.fixture.divss_controls,
                divss_memory_rax_unmasked_controls: input
                    .fixture
                    .divss_memory_rax_unmasked_controls,
                mulsd_controls: input.fixture.mulsd_controls,
                mulsd_memory_rax_unmasked_controls: input
                    .fixture
                    .mulsd_memory_rax_unmasked_controls,
                mulss_controls: input.fixture.mulss_controls,
                mulss_memory_rax_unmasked_controls: input
                    .fixture
                    .mulss_memory_rax_unmasked_controls,
                mulps_controls: input.fixture.mulps_controls,
                subps_controls: input.fixture.subps_controls,
                subsd_controls: input.fixture.subsd_controls,
                subsd_memory_rax_unmasked_controls: input
                    .fixture
                    .subsd_memory_rax_unmasked_controls,
                subss_controls: input.fixture.subss_controls,
                cvtss2sd_controls: input.fixture.cvtss2sd_controls,
                cvtsd2ss_controls: input.fixture.cvtsd2ss_controls,
                cvtpi2pd_memory_controls: input.fixture.cvtpi2pd_memory_controls,
                cvtpi2pd_register_controls: input.fixture.cvtpi2pd_register_controls,
                cvtpi2ps_memory_controls: input.fixture.cvtpi2ps_memory_controls,
                cvtpi2ps_register_controls: input.fixture.cvtpi2ps_register_controls,
                cvttps2pi_memory_controls: input.fixture.cvttps2pi_memory_controls,
                cvtps2pi_memory_controls: input.fixture.cvtps2pi_memory_controls,
                cvtps2pi_register_controls: input.fixture.cvtps2pi_register_controls,
                cvtpd2pi_register_controls: input.fixture.cvtpd2pi_register_controls,
                cvttpd2pi_register_controls: input.fixture.cvttpd2pi_register_controls,
                cvttpd2pi_memory_controls: input.fixture.cvttpd2pi_memory_controls,
                cvtpd2pi_memory_controls: input.fixture.cvtpd2pi_memory_controls,
                cvtps2pd_controls: input.fixture.cvtps2pd_controls,
                cvtps2pd_m64_controls: input.fixture.cvtps2pd_m64_controls,
                cvtss2sd_m32_controls: input.fixture.cvtss2sd_m32_controls,
                cvtsd2ss_m64_controls: input.fixture.cvtsd2ss_m64_controls,
                cvtpd2ps_m128_controls: input.fixture.cvtpd2ps_m128_controls,
                comiss_ucomiss_controls: input.fixture.comiss_ucomiss_controls,
                cmpss_controls: input.fixture.cmpss_controls,
                cmpsd_controls: input.fixture.cmpsd_controls,
                cmppd_controls: input.fixture.cmppd_controls,
                cmpps_controls: input.fixture.cmpps_controls,
                comisd_ucomisd_controls: input.fixture.comisd_ucomisd_controls,
                comisd_memory_rax_controls: input.fixture.comisd_memory_rax_controls,
                comisd_memory_rax_invalid_controls: input
                    .fixture
                    .comisd_memory_rax_invalid_controls,
                ucomisd_memory_rax_controls: input.fixture.ucomisd_memory_rax_controls,
                ucomisd_memory_rax_invalid_controls: input
                    .fixture
                    .ucomisd_memory_rax_invalid_controls,
                comiss_memory_rax_controls: input.fixture.comiss_memory_rax_controls,
                comiss_memory_invalid_rax_controls: input
                    .fixture
                    .comiss_memory_invalid_rax_controls,
                ucomiss_memory_rax_controls: input.fixture.ucomiss_memory_rax_controls,
                ucomiss_memory_invalid_rax_controls: input
                    .fixture
                    .ucomiss_memory_invalid_rax_controls,
                cmpss_memory_rax_controls: input.fixture.cmpss_memory_rax_controls,
                cmpsd_memory_rax_controls: input.fixture.cmpsd_memory_rax_controls,
                cmppd_memory_rax_controls: input.fixture.cmppd_memory_rax_controls,
                cmpps_memory_rax_controls: input.fixture.cmpps_memory_rax_controls,
                minsd_memory_rax_controls: input.fixture.minsd_memory_rax_controls,
                maxss_memory_rax_controls: input.fixture.maxss_memory_rax_controls,
                minss_memory_rax_controls: input.fixture.minss_memory_rax_controls,
                minss_memory_unmasked_rax_controls: input
                    .fixture
                    .minss_memory_unmasked_rax_controls,
                maxsd_memory_rax_controls: input.fixture.maxsd_memory_rax_controls,
                minpd_memory_rax_controls: input.fixture.minpd_memory_rax_controls,
                minps_memory_rax_controls: input.fixture.minps_memory_rax_controls,
                sqrtps_memory_rax_controls: input.fixture.sqrtps_memory_rax_controls,
                sqrtps_memory_rax_unmasked_controls: input
                    .fixture
                    .sqrtps_memory_rax_unmasked_controls,
                rcpps_memory_rax_controls: input.fixture.rcpps_memory_rax_controls,
                rcpss_memory_rax_controls: input.fixture.rcpss_memory_rax_controls,
                rsqrtps_memory_rax_controls: input.fixture.rsqrtps_memory_rax_controls,
                rsqrtss_memory_rax_controls: input.fixture.rsqrtss_memory_rax_controls,
                addps_memory_rax_controls: input.fixture.addps_memory_rax_controls,
                addps_memory_rax_unmasked_controls: input
                    .fixture
                    .addps_memory_rax_unmasked_controls,
                subps_memory_rax_unmasked_controls: input
                    .fixture
                    .subps_memory_rax_unmasked_controls,
                sqrtss_memory_rax_controls: input.fixture.sqrtss_memory_rax_controls,
                sqrtss_memory_rax_unmasked_controls: input
                    .fixture
                    .sqrtss_memory_rax_unmasked_controls,
                sqrtpd_memory_rax_controls: input.fixture.sqrtpd_memory_rax_controls,
                sqrtpd_memory_rax_unmasked_controls: input
                    .fixture
                    .sqrtpd_memory_rax_unmasked_controls,
                sqrtsd_memory_rax_controls: input.fixture.sqrtsd_memory_rax_controls,
                maxpd_memory_rax_controls: input.fixture.maxpd_memory_rax_controls,
                maxps_memory_rax_controls: input.fixture.maxps_memory_rax_controls,
                cvtsi2sd_r32_controls: input.fixture.cvtsi2sd_r32_controls,
                cvtsi2sd_r32_memory_controls: input.fixture.cvtsi2sd_r32_memory_controls,
                cvtsi2ss_r32_memory_controls: input.fixture.cvtsi2ss_r32_memory_controls,
                cvtsi2ss_r64_memory_controls: input.fixture.cvtsi2ss_r64_memory_controls,
                cvtsi2sd_r64_controls: input.fixture.cvtsi2sd_r64_controls,
                cvtsi2sd_r64_memory_controls: input.fixture.cvtsi2sd_r64_memory_controls,
                cvtsi2ss_controls: input.fixture.cvtsi2ss_controls,
                cvtss2si_controls: input.fixture.cvtss2si_controls,
                cvtsd2si_controls: input.fixture.cvtsd2si_controls,
                cvtsd2si_r32_memory_controls: input.fixture.cvtsd2si_r32_memory_controls,
                cvtsd2si_r64_memory_controls: input.fixture.cvtsd2si_r64_memory_controls,
                cvttsd2si_r32_memory_controls: input.fixture.cvttsd2si_r32_memory_controls,
                cvttsd2si_r64_memory_controls: input.fixture.cvttsd2si_r64_memory_controls,
                cvtss2si_r32_memory_controls: input.fixture.cvtss2si_r32_memory_controls,
                cvtss2si_r64_memory_controls: input.fixture.cvtss2si_r64_memory_controls,
                cvttss2si_r32_memory_controls: input.fixture.cvttss2si_r32_memory_controls,
                cvttss2si_r64_memory_controls: input.fixture.cvttss2si_r64_memory_controls,
                fxsave64_cr0_controls: input.fixture.fxsave64_cr0_controls,
                fxrstor64_cr0_controls: input.fixture.fxrstor64_cr0_controls,
                rdtsc_source_calls: None,
                exception: Observation::Available { value: None },
            },
        },
        build: BuildMetadata {
            revision: "test-only".into(),
            target: "test-only".into(),
            profile: "test-only".into(),
            compiler: "test-only".into(),
        },
        machine: MachineMetadata {
            host_os: "test-only".into(),
            host_arch: "test-only".into(),
            cpu_model: None,
            kernel: None,
            page_size: None,
        },
    }
}

#[test]
fn comisd_m64_oracle_prioritizes_nan_over_subnormal_and_preserves_x87() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37co_comisd_m64_qnan_subnormal_priority.case.json"
    ))
    .unwrap();
    let mut expected = result(&loaded);
    let output = snapshot(&mut expected);
    output.state.rflags = HexU64(0x0247);
    output.state.sse.as_mut().unwrap().mxcsr = HexU64(0x0181);
    assert!(expected.validate_for(&loaded).is_ok());

    let mut tampered = expected;
    snapshot(&mut tampered)
        .state
        .x87
        .as_mut()
        .unwrap()
        .status_word = HexU64(0x0080);
    assert!(tampered.validate_for(&loaded).is_err());
}

#[test]
fn ucomisd_m64_oracle_keeps_qnan_ie_and_de_clear_and_preserves_x87() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37cp_ucomisd_m64_qnan_subnormal_priority.case.json"
    ))
    .unwrap();
    let mut expected = result(&loaded);
    let output = snapshot(&mut expected);
    output.state.rflags = HexU64(0x0247);
    output.state.sse.as_mut().unwrap().mxcsr = HexU64(0x0180);
    assert!(expected.validate_for(&loaded).is_ok());

    for mxcsr in [0x0181, 0x0182] {
        let mut tampered = expected.clone();
        snapshot(&mut tampered).state.sse.as_mut().unwrap().mxcsr = HexU64(mxcsr);
        assert!(tampered.validate_for(&loaded).is_err());
    }
    let mut tampered = expected;
    snapshot(&mut tampered)
        .state
        .x87
        .as_mut()
        .unwrap()
        .status_word = HexU64(0x0080);
    assert!(tampered.validate_for(&loaded).is_err());
}

#[test]
fn comiss_m32_oracle_raises_ie_for_qnan_before_subnormal_and_preserves_x87() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37cq_comiss_m32_qnan_subnormal_priority.case.json"
    ))
    .unwrap();
    for (lhs, rhs, flags, mxcsr) in [
        (0x7fc1_2345u32, 1u32, 0x0247u64, 0x0181u64), // QNaN + subnormal: IE, no DE
        (0x7f81_2345u32, 1u32, 0x0247u64, 0x0181u64), // SNaN + subnormal: IE, no DE
        (1u32, 0x3f80_0000u32, 0x0203u64, 0x0182u64), // subnormal + finite: DE
    ] {
        let mut case = loaded.clone();
        case.fixture.initial_state.sse.as_mut().unwrap().xmm[0] = format!(
            "{}{}",
            lhs.to_le_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "010000000000f07f01000000"
        );
        case.fixture.memory[1].initial_bytes_hex = rhs
            .to_le_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut expected = result(&case);
        let output = snapshot(&mut expected);
        output.state.rflags = HexU64(flags);
        output.state.sse.as_mut().unwrap().mxcsr = HexU64(mxcsr);
        let validation = expected.validate_for(&case);
        assert!(
            validation.is_ok(),
            "unexpected oracle rejection: {validation:?}"
        );

        for wrong_status in [0x0180, 0x0183] {
            let mut tampered = expected.clone();
            snapshot(&mut tampered).state.sse.as_mut().unwrap().mxcsr = HexU64(wrong_status);
            assert!(tampered.validate_for(&case).is_err());
        }
        let mut tampered = expected;
        snapshot(&mut tampered)
            .state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(0x0080);
        assert!(tampered.validate_for(&case).is_err());
    }
}

#[test]
fn ucomiss_m32_oracle_keeps_qnan_ie_and_de_clear_and_preserves_x87() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37cr_ucomiss_m32_qnan_subnormal_priority.case.json"
    ))
    .unwrap();
    let mut expected = result(&loaded);
    let output = snapshot(&mut expected);
    output.state.rflags = HexU64(0x0247);
    output.state.sse.as_mut().unwrap().mxcsr = HexU64(0x0180);
    assert!(expected.validate_for(&loaded).is_ok());

    for mxcsr in [0x0181, 0x0182, 0x0183] {
        let mut tampered = expected.clone();
        snapshot(&mut tampered).state.sse.as_mut().unwrap().mxcsr = HexU64(mxcsr);
        assert!(tampered.validate_for(&loaded).is_err());
    }

    let mut signaling = loaded.clone();
    signaling.fixture.initial_state.sse.as_mut().unwrap().xmm[0] =
        "4523817f010000000000f07f00000000".into();
    let mut signaling_expected = result(&signaling);
    let output = snapshot(&mut signaling_expected);
    output.state.rflags = HexU64(0x0247);
    output.state.sse.as_mut().unwrap().mxcsr = HexU64(0x0181);
    assert!(signaling_expected.validate_for(&signaling).is_ok());
}

fn snapshot(result: &mut RunResult) -> &mut Snapshot {
    match &mut result.snapshot {
        Observation::Available { value } => value,
        _ => panic!("test snapshot"),
    }
}

fn x87_fixture() -> LoadedFixture {
    let mut fixture = loaded().fixture;
    fixture.case_id = "local_x87_state_nop".into();
    fixture.profile = Profile::LongX87StateV1;
    fixture.initial_state.rip = HexU64(0x1000);
    fixture.initial_state.x87 = Some(X87Snapshot::from(&od_isa::state::X87State::fixture_seed()));
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.guest_address = HexU64(0x1000);
    code.length = 1;
    code.initial_bytes_hex = "90".into();
    code.comparison_mask_hex = "ff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x1001));
    fixture.expected_exit = ExpectedExit::StopAddress;
    Fixture::from_json(&fixture.to_json().unwrap()).unwrap()
}

fn x87_sse_fixture() -> LoadedFixture {
    let mut fixture = x87_fixture().fixture;
    fixture.case_id = "local_x87_sse_state_nop".into();
    fixture.profile = Profile::LongX87SseStateV1;
    fixture.initial_state.sse = Some(SseSnapshot {
        xmm: std::array::from_fn(|register| {
            (0..16)
                .map(|byte| format!("{:02x}", register * 16 + byte))
                .collect()
        }),
        mxcsr: HexU64(0xdead_beef),
        diagnostic_mxcsr_mask: HexU64(u64::from(PROFILE_SSE_MXCSR_DIAGNOSTIC_MASK)),
    });
    Fixture::from_json(&fixture.to_json().unwrap()).unwrap()
}

#[test]
fn x87_sse_state_profile_roundtrips_raw_state_and_compares_every_xmm_and_mxcsr() {
    let input = x87_sse_fixture();
    let state = input
        .fixture
        .initial_state
        .to_cpu_for(Profile::LongX87SseStateV1)
        .unwrap();
    let sse = state.sse.as_ref().unwrap();
    assert_eq!(sse.xmm[0], std::array::from_fn(|i| i as u8));
    assert_eq!(sse.xmm[15], std::array::from_fn(|i| (240 + i) as u8));
    assert_eq!(sse.mxcsr, 0xdead_beef);
    assert_eq!(
        IntegerState::from_cpu_for(&state, Profile::LongX87SseStateV1)
            .unwrap()
            .sse,
        input.fixture.initial_state.sse
    );
    assert_eq!(
        state.x87,
        input
            .fixture
            .initial_state
            .x87
            .as_ref()
            .map(|value| value.to_cpu().unwrap())
    );

    let baseline = result(&input);
    for register in 0..16 {
        let mut changed = baseline.clone();
        let snapshot = snapshot(&mut changed);
        snapshot.state.sse.as_mut().unwrap().xmm[register].replace_range(0..2, "ff");
        assert!(
            compare(&input, &baseline, &changed)
                .unwrap()
                .contains(&"sse".into())
        );
    }
    let mut changed = baseline.clone();
    snapshot(&mut changed).state.sse.as_mut().unwrap().mxcsr = HexU64(0xdead_beee);
    assert!(
        compare(&input, &baseline, &changed)
            .unwrap()
            .contains(&"sse".into())
    );
}

#[test]
fn x87_selector_snapshot_preserves_availability_width_and_high_pointer_values() {
    let mut state = od_isa::state::X87State::fixture_seed();
    state.instruction_pointer = 0xfedc_ba98_7654_3210;
    state.data_pointer = 0x8765_4321_fedc_ba98;
    for instruction_selector in [None, Some(0), Some(1), Some(0x8000), Some(0xffff)] {
        for data_selector in [None, Some(0), Some(1), Some(0x8000), Some(0xffff)] {
            state.instruction_selector = instruction_selector;
            state.data_selector = data_selector;
            let snapshot = X87Snapshot::from(&state);
            let json = serde_json::to_vec(&snapshot).unwrap();
            let decoded: X87Snapshot = serde_json::from_slice(&json).unwrap();
            assert_eq!(decoded, snapshot);
            assert_eq!(decoded.to_cpu().unwrap(), state);
        }
    }
    let mut too_wide = X87Snapshot::from(&state);
    too_wide.instruction_selector = Some(HexU64(0x1_0000));
    assert!(too_wide.to_cpu().is_err());
    let mut data_too_wide = X87Snapshot::from(&state);
    data_too_wide.data_selector = Some(HexU64(0x1_0000));
    assert!(data_too_wide.to_cpu().is_err());
}

#[test]
fn x87_selector_profile_requires_both_selectors_and_rejects_them_in_old_profiles() {
    let mut fixture = x87_sse_fixture().fixture;
    fixture.case_id = "local_x87_selector_snapshot_nop".into();
    fixture.profile = Profile::LongX87SelectorsStateV1;
    let x87 = fixture.initial_state.x87.as_mut().unwrap();
    x87.instruction_selector = Some(HexU64(0x2b));
    x87.data_selector = Some(HexU64(0x53));
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());
    let selector_cpu = fixture
        .initial_state
        .to_cpu_for(Profile::LongX87SelectorsStateV1)
        .unwrap();
    assert!(IntegerState::from_cpu_for(&selector_cpu, Profile::LongX87SseStateV1).is_err());

    for (instruction_selector, data_selector) in
        [(None, Some(HexU64(0x53))), (Some(HexU64(0x2b)), None)]
    {
        let mut missing = fixture.clone();
        let x87 = missing.initial_state.x87.as_mut().unwrap();
        x87.instruction_selector = instruction_selector;
        x87.data_selector = data_selector;
        assert!(missing.validate().is_err());
    }

    let mut missing_x87 = fixture.clone();
    missing_x87.initial_state.x87 = None;
    assert!(missing_x87.validate().is_err());
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut too_wide = fixture.clone();
    too_wide
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .instruction_selector = Some(HexU64(0x1_0000));
    assert!(too_wide.validate().is_err());
    let mut malformed = serde_json::to_value(&fixture).unwrap();
    malformed["initial_state"]["x87"]["instruction_selector"] =
        serde_json::json!("not-a-hex-value");
    assert!(Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err());
    for field in ["instruction_selector", "data_selector"] {
        let mut null_selector = serde_json::to_value(&fixture).unwrap();
        null_selector["initial_state"]["x87"][field] = serde_json::Value::Null;
        assert!(Fixture::from_json(&serde_json::to_vec(&null_selector).unwrap()).is_err());
    }

    let mut old = x87_sse_fixture().fixture;
    old.initial_state.x87.as_mut().unwrap().instruction_selector = Some(HexU64(0x2b));
    assert!(old.validate().is_err());
    let old_with_null = x87_sse_fixture().fixture.to_json().unwrap();
    let mut old_with_null: serde_json::Value = serde_json::from_slice(&old_with_null).unwrap();
    old_with_null["initial_state"]["x87"]["instruction_selector"] = serde_json::Value::Null;
    assert!(Fixture::from_json(&serde_json::to_vec(&old_with_null).unwrap()).is_err());

    let mut unknown = serde_json::to_value(&fixture).unwrap();
    unknown["initial_state"]["x87"]["unrecognized"] = serde_json::json!("0x0");
    assert!(Fixture::from_json(&serde_json::to_vec(&unknown).unwrap()).is_err());

    let left = result(&Fixture::from_json(&fixture.to_json().unwrap()).unwrap());
    let mut right = left.clone();
    if let Observation::Available { value } = &mut right.snapshot {
        value.state.x87.as_mut().unwrap().data_selector = Some(HexU64(0x54));
    }
    let loaded = Fixture::from_json(&fixture.to_json().unwrap()).unwrap();
    let difference = compare(&loaded, &left, &right).unwrap();
    assert!(difference.iter().any(|difference| difference == "x87"));

    let before = left.snapshot.clone();
    let mut no_instruction_selector = left.clone();
    if let Observation::Available { value } = &mut no_instruction_selector.snapshot {
        value.state.x87.as_mut().unwrap().instruction_selector = None;
    }
    let (Observation::Available { value: before }, Observation::Available { value: after }) =
        (before, no_instruction_selector.snapshot)
    else {
        panic!("selector-profile snapshots must be available")
    };
    assert_ne!(before.state.x87, after.state.x87);
}

#[test]
fn fxsave_rip_profile_is_exact_and_requires_geometry_and_both_selectors() {
    let raw = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39c_fxsave_rip_relative_literal.case.json"
    );
    let base = Fixture::from_json(raw).unwrap().fixture;
    assert_eq!(base.profile, Profile::LongFxsaveRipV1);
    let state = base
        .initial_state
        .to_cpu_for(Profile::LongFxsaveRipV1)
        .unwrap();
    assert_eq!(state.x87.as_ref().unwrap().instruction_selector, Some(0x2b));
    assert_eq!(state.x87.as_ref().unwrap().data_selector, Some(0x53));

    for code in [
        "480fae056bf91700",
        "660fae056bf91700",
        "670fae056bf91700",
        "f30fae056bf91700",
        "0fae006bf91700",
        "0fae046bf91700",
        "0fae0d6bf91700",
        "0fae4d00",
    ] {
        let mut invalid = base.clone();
        invalid.memory[0].initial_bytes_hex = code.into();
        invalid.memory[0].length = (code.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(code.len() / 2);
        assert!(invalid.validate().is_err(), "{code}");
    }

    let mut unaligned = base.clone();
    unaligned.memory[1].guest_address = HexU64(0x200001);
    assert!(unaligned.validate().is_err());
    let mut wrong_length = base.clone();
    wrong_length.memory[1].length = 511;
    wrong_length.memory[1].initial_bytes_hex.truncate(1022);
    wrong_length.memory[1].comparison_mask_hex.truncate(1022);
    assert!(wrong_length.validate().is_err());
    let mut read_only = base.clone();
    read_only.memory[1].permissions.write = false;
    assert!(read_only.validate().is_err());
    let mut no_x87_selector = base.clone();
    no_x87_selector
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .instruction_selector = None;
    assert!(no_x87_selector.validate().is_err());
    let mut no_sse = base.clone();
    no_sse.initial_state.sse = None;
    assert!(no_sse.validate().is_err());

    let mut legacy_profile = base;
    legacy_profile.profile = Profile::LongX87SseStateV1;
    assert!(legacy_profile.validate().is_err());
}

#[test]
fn fxsave64_rax_profile_is_exact_and_selectors_are_independently_optional() {
    let raw = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39d_fxsave64_rax_disp8_literal.case.json"
    );
    let base = Fixture::from_json(raw).unwrap().fixture;
    assert_eq!(base.profile, Profile::LongFxsave64RaxV1);
    let state = base
        .initial_state
        .to_cpu_for(Profile::LongFxsave64RaxV1)
        .unwrap();
    let x87 = state.x87.as_ref().unwrap();
    assert_eq!(x87.instruction_selector, None);
    assert_eq!(x87.data_selector, None);

    let mut known_selectors = base.clone();
    known_selectors
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .instruction_selector = Some(HexU64(0x2b));
    known_selectors
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .data_selector = Some(HexU64(0x53));
    assert!(known_selectors.validate().is_ok());
    known_selectors
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .instruction_selector = None;
    assert!(known_selectors.validate().is_ok());
    let x87 = known_selectors.initial_state.x87.as_mut().unwrap();
    x87.data_selector = None;
    x87.instruction_selector = Some(HexU64(0));
    assert!(known_selectors.validate().is_ok());

    for code in [
        "480fae4041",    // different displacement
        "490fae4040",    // REX.B changes the base
        "4c0fae4040",    // another REX form
        "4848 0fae4040", // repeated REX
        "66480fae4040",  // operand-size prefix
        "67480fae4040",  // address-size prefix
        "f3480fae4040",  // group-1 prefix
        "64480fae4040",  // segment prefix
        "480fae4140",    // different base register
        "480fae4b40",    // FXRSTOR64
    ] {
        let code = code.replace(' ', "");
        let mut invalid = base.clone();
        invalid.memory[0].initial_bytes_hex = code.clone();
        invalid.memory[0].length = (code.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(code.len() / 2);
        assert!(invalid.validate().is_err(), "{code}");
    }

    let mut wrong_base = base.clone();
    wrong_base.initial_state.gpr[0] = HexU64(0x0000_0000_001f_ffc1);
    assert!(wrong_base.validate().is_err());
    let mut wrapped_ea = base.clone();
    wrapped_ea.initial_state.gpr[0] = HexU64(u64::MAX - 0x3f);
    wrapped_ea.memory[1].guest_address = HexU64(0);
    assert!(wrapped_ea.validate().is_ok());

    let mut noncanonical_ea = base.clone();
    noncanonical_ea.initial_state.gpr[0] = HexU64(0x0000_7fff_ffff_ffc0);
    noncanonical_ea.memory[1].guest_address = HexU64(0x0000_8000_0000_0000);
    assert!(noncanonical_ea.validate().is_err());
    let mut overflowing_image = base.clone();
    overflowing_image.initial_state.gpr[0] = HexU64(0xffff_ffff_ffff_fec0);
    overflowing_image.memory[1].guest_address = HexU64(0xffff_ffff_ffff_ff00);
    assert!(overflowing_image.validate().is_err());
    let mut wrong_stop = base.clone();
    wrong_stop.stop_rip = Some(HexU64(0x80694));
    assert!(wrong_stop.validate().is_err());
    let mut no_sse = base.clone();
    no_sse.initial_state.sse = None;
    assert!(no_sse.validate().is_err());
}

#[test]
fn fxrstor64_rbx_profile_is_exact_readable_and_preserves_optional_selectors() {
    let raw = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39e_fxrstor64_rbx_disp8_literal.case.json"
    );
    let base = Fixture::from_json(raw).unwrap().fixture;
    assert_eq!(base.profile, Profile::LongFxrstor64RbxV1);
    assert_eq!(
        base.memory
            .iter()
            .find(|region| region.region_id == "data")
            .unwrap()
            .permissions
            .read,
        true
    );
    assert_eq!(
        base.memory
            .iter()
            .find(|region| region.region_id == "data")
            .unwrap()
            .permissions
            .write,
        false
    );
    let state = base
        .initial_state
        .to_cpu_for(Profile::LongFxrstor64RbxV1)
        .unwrap();
    assert_eq!(state.x87.as_ref().unwrap().instruction_selector, None);
    assert_eq!(state.x87.as_ref().unwrap().data_selector, None);

    let mut present_selectors = base.clone();
    let x87 = present_selectors.initial_state.x87.as_mut().unwrap();
    x87.instruction_selector = Some(HexU64(0));
    x87.data_selector = Some(HexU64(0x53));
    assert!(present_selectors.validate().is_ok());

    for invalid in ["480fae4b41", "480fae4340", "490fae4b40", "66480fae4b40"] {
        let mut changed = base.clone();
        changed
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .initial_bytes_hex = invalid.to_string();
        assert!(changed.validate().is_err(), "{invalid}");
    }
    let mut unreadable = base;
    unreadable
        .memory
        .iter_mut()
        .find(|region| region.region_id == "data")
        .unwrap()
        .permissions
        .read = false;
    assert!(unreadable.validate().is_err());
}

#[test]
fn integer_snapshot_export_rejects_ds_hidden_cache() {
    let mut state = od_isa::LongModeState::default();
    state.ds_cache = Some(od_isa::state::DsDescriptorCache {
        base: 0x1234_5678,
        limit: 0xffff,
        access: 0x93,
        flags: 0,
    });
    assert!(IntegerState::try_from(&state).is_err());
    assert!(IntegerState::from_cpu_for(&state, Profile::LongIntegerV1).is_err());
}

#[test]
fn integer_snapshot_export_rejects_es_only_and_combined_segment_caches() {
    let es_cache = od_isa::state::DataSegmentDescriptorCache {
        base: 0x8765_4321,
        limit: 0xffff,
        access: 0x93,
        flags: 0,
    };
    let mut es_only = od_isa::LongModeState::default();
    es_only.es_cache = Some(es_cache);
    assert!(IntegerState::try_from(&es_only).is_err());
    assert!(IntegerState::from_cpu_for(&es_only, Profile::LongIntegerV1).is_err());

    let mut both = es_only.clone();
    both.ds_cache = Some(od_isa::state::DsDescriptorCache {
        base: 0x1234_5678,
        limit: 0xffff,
        access: 0x93,
        flags: 0,
    });
    assert!(IntegerState::try_from(&both).is_err());
    assert!(IntegerState::from_cpu_for(&both, Profile::LongIntegerV1).is_err());
}

#[test]
fn integer_snapshot_export_rejects_present_idtr_even_when_zero() {
    let mut state = od_isa::LongModeState::default();
    state.idtr = Some(od_isa::state::Idtr { base: 0, limit: 0 });
    assert!(IntegerState::try_from(&state).is_err());
    assert!(IntegerState::from_cpu_for(&state, Profile::LongIntegerV1).is_err());
}

#[test]
fn integer_snapshot_export_rejects_present_cs_ss_state_even_when_zero() {
    let mut state = od_isa::LongModeState::default();
    state.cs_selector = Some(0);
    assert!(IntegerState::try_from(&state).is_err());
    assert!(IntegerState::from_cpu_for(&state, Profile::LongIntegerV1).is_err());
    state.cs_selector = None;
    state.ss_selector = Some(0);
    assert!(IntegerState::try_from(&state).is_err());
    assert!(IntegerState::from_cpu_for(&state, Profile::LongIntegerV1).is_err());
    state.ss_selector = None;
    state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x9b,
        flags: 2,
    });
    assert!(IntegerState::try_from(&state).is_err());
    state.cs_cache = None;
    state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
        base: 0,
        limit: u32::MAX,
        access: 0x93,
        flags: 0,
    });
    assert!(IntegerState::from_cpu_for(&state, Profile::LongIntegerV1).is_err());
}

#[test]
fn x87_sse_profile_has_strict_wire_scope_and_raw_mxcsr_validation() {
    let mut fixture = x87_sse_fixture().fixture;
    let mut wrong_width = fixture.clone();
    wrong_width.initial_state.sse.as_mut().unwrap().xmm[0].pop();
    assert!(wrong_width.validate().is_err());
    let mut wrong_case = fixture.clone();
    wrong_case.initial_state.sse.as_mut().unwrap().xmm[0].make_ascii_uppercase();
    assert!(wrong_case.validate().is_err());
    let mut wrong_mxcsr = fixture.clone();
    wrong_mxcsr.initial_state.sse.as_mut().unwrap().mxcsr = HexU64(u64::from(u32::MAX) + 1);
    assert!(wrong_mxcsr.validate().is_err());
    let mut wrong_mask = fixture.clone();
    wrong_mask
        .initial_state
        .sse
        .as_mut()
        .unwrap()
        .diagnostic_mxcsr_mask = HexU64(0xffff);
    assert!(wrong_mask.validate().is_err());

    let mut old_x87 = x87_fixture().fixture;
    old_x87.initial_state.sse = fixture.initial_state.sse.clone();
    assert!(old_x87.validate().is_err());
    let mut old_integer = loaded().fixture;
    let old_wire = serde_json::to_value(&old_integer).unwrap();
    assert!(old_wire["initial_state"].get("sse").is_none());
    let mut old_with_null = old_wire.clone();
    old_with_null["initial_state"]["sse"] = serde_json::Value::Null;
    assert!(Fixture::from_json(&serde_json::to_vec(&old_with_null).unwrap()).is_err());
    old_integer.initial_state.sse = fixture.initial_state.sse.take();
    assert!(old_integer.validate().is_err());

    let mut wrong_program = x87_sse_fixture().fixture;
    wrong_program.memory[0].initial_bytes_hex = "dbe3".into();
    wrong_program.memory[0].length = 2;
    wrong_program.memory[0].comparison_mask_hex = "ffff".into();
    wrong_program.stop_rip = Some(HexU64(0x1002));
    assert!(wrong_program.validate().is_err());
}

#[test]
fn xorpd_profile_is_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_17_xorpd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongXorpdRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450f57c1");
    for bytes in [
        "0f57c1",
        "f2660f57c1",
        "f3660f57c1",
        "67660f57c1",
        "f0660f57c1",
        "2e660f57c1",
        "0f6657c1",
        "66480f57c1",
        "66420f57c1",
        "66660f57c1",
        "6640410f57c1",
        "660f5701",
        "660f57",
        "c5f957c1",
    ] {
        let mut wrong = fixture.clone();
        wrong.memory[0].initial_bytes_hex = bytes.into();
        wrong.memory[0].length = (bytes.len() / 2) as u32;
        wrong.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        wrong.stop_rip = Some(HexU64(
            wrong.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(wrong.validate().is_err(), "{bytes}");
    }
}

#[test]
fn xorpd_prerequisite_profile_requires_explicit_controls_and_exact_encoding() {
    let bytes =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_38b_xorpd_sse2_absent.case.json");
    let loaded = Fixture::from_json(bytes).unwrap();
    assert_eq!(loaded.fixture.profile, Profile::LongXorpdPrerequisitesV1);
    assert_eq!(
        loaded.fixture.xorpd_prerequisites,
        Some(XorpdPrerequisiteSnapshot {
            sse2_available: false,
            cr0_em: false,
            cr0_ts: true,
            cr4_osfxsr: false,
        })
    );

    let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    value.as_object_mut().unwrap().remove("xorpd_prerequisites");
    assert!(Fixture::from_json(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut missing_sse: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    missing_sse["initial_state"]
        .as_object_mut()
        .unwrap()
        .remove("sse");
    assert!(Fixture::from_json(&serde_json::to_vec(&missing_sse).unwrap()).is_err());

    value["xorpd_prerequisites"] = serde_json::json!({
        "sse2_available": true,
        "cr0_em": false,
        "cr0_ts": false
    });
    assert!(Fixture::from_json(&serde_json::to_vec(&value).unwrap()).is_err());

    value["xorpd_prerequisites"]["cr4_osfxsr"] = serde_json::Value::Bool(true);
    value["memory"][0]["initial_bytes_hex"] = serde_json::Value::String("660f57c0".into());
    assert!(Fixture::from_json(&serde_json::to_vec(&value).unwrap()).is_err());

    let mut old = loaded.fixture.clone();
    old.profile = Profile::LongXorpdRegV1;
    old.xorpd_prerequisites = Some(XorpdPrerequisiteSnapshot {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
    });
    assert!(old.validate().is_err());
}

#[test]
fn xorpd_prerequisite_snapshot_roundtrips_and_control_mismatch_does_not_compare_equal() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38b_xorpd_sse2_absent.case.json"
    ))
    .unwrap()
    .fixture;
    fixture.case_id = "s2_38b_xorpd_success_snapshot".into();
    fixture.xorpd_prerequisites = Some(XorpdPrerequisiteSnapshot {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
    });
    fixture.expected_exit = ExpectedExit::StopAddress;
    let input = Fixture::from_json(&fixture.to_json().unwrap()).unwrap();

    let left = result(&input);
    let encoded = left.to_json().unwrap();
    let decoded = RunResult::from_json(&encoded).unwrap();
    assert_eq!(
        match decoded.snapshot {
            Observation::Available { value } => value.xorpd_prerequisites,
            _ => None,
        },
        input.fixture.xorpd_prerequisites
    );

    let mut right = left.clone();
    snapshot(&mut right).xorpd_prerequisites = Some(XorpdPrerequisiteSnapshot {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: true,
        cr4_osfxsr: true,
    });
    let mismatches = compare(&input, &left, &right).unwrap();
    assert!(mismatches.contains(&"xorpd_prerequisites".to_owned()));
    assert!(mismatches.contains(&"right.xorpd_prerequisites".to_owned()));

    let old = result(&loaded());
    let old_bytes = old.to_json().unwrap();
    let old_json: serde_json::Value = serde_json::from_slice(&old_bytes).unwrap();
    assert!(
        old_json["snapshot"]["value"]
            .get("xorpd_prerequisites")
            .is_none()
    );
    let old_roundtrip = RunResult::from_json(&old_bytes).unwrap();
    assert!(
        matches!(old_roundtrip.snapshot, Observation::Available { value } if value.xorpd_prerequisites.is_none())
    );
}

#[test]
fn fxsave64_cr0_snapshot_is_strict_echo_and_comparison_input() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39f_fxsave64_cr0_clear_success.case.json"
    ))
    .unwrap();
    let left = result(&input);
    let encoded = left.to_json().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        json["snapshot"]["value"]["fxsave64_cr0_controls"],
        serde_json::json!({"cr0_em": false, "cr0_ts": false})
    );
    assert!(RunResult::from_json(&encoded).is_ok());

    let mut changed = left.clone();
    snapshot(&mut changed).fxsave64_cr0_controls = Some(Fxsave64Cr0Snapshot {
        cr0_em: true,
        cr0_ts: false,
    });
    let differences = compare(&input, &left, &changed).unwrap();
    assert!(differences.contains(&"fxsave64_cr0_controls".into()));
    assert!(differences.contains(&"right.fxsave64_cr0_controls".into()));

    let legacy = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39d_fxsave64_rax_disp8_literal.case.json"
    ))
    .unwrap();
    assert_eq!(legacy.fixture.fxsave64_cr0_controls, None);
    let legacy_result = result(&legacy);
    let legacy_json: serde_json::Value =
        serde_json::from_slice(&legacy_result.to_json().unwrap()).unwrap();
    assert!(
        legacy_json["snapshot"]["value"]
            .get("fxsave64_cr0_controls")
            .is_none()
    );
}

#[test]
fn fxrstor64_cr0_snapshot_is_strict_echo_and_comparison_input() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39g_fxrstor64_cr0_clear_success.case.json"
    ))
    .unwrap();
    let left = result(&input);
    let encoded = left.to_json().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        json["snapshot"]["value"]["fxrstor64_cr0_controls"],
        serde_json::json!({"cr0_em": false, "cr0_ts": false})
    );
    assert!(RunResult::from_json(&encoded).is_ok());

    let mut changed = left.clone();
    snapshot(&mut changed).fxrstor64_cr0_controls = Some(Fxrstor64Cr0Snapshot {
        cr0_em: true,
        cr0_ts: false,
    });
    let differences = compare(&input, &left, &changed).unwrap();
    assert!(differences.contains(&"fxrstor64_cr0_controls".into()));
    assert!(differences.contains(&"right.fxrstor64_cr0_controls".into()));

    let legacy = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39e_fxrstor64_rbx_disp8_literal.case.json"
    ))
    .unwrap();
    assert_eq!(legacy.fixture.fxrstor64_cr0_controls, None);
    let legacy_result = result(&legacy);
    let legacy_json: serde_json::Value =
        serde_json::from_slice(&legacy_result.to_json().unwrap()).unwrap();
    assert!(
        legacy_json["snapshot"]["value"]
            .get("fxrstor64_cr0_controls")
            .is_none()
    );
}

#[test]
fn minss_invalid_controls_fixture_and_result_are_strict_and_backward_compatible() {
    let bytes = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38c_minss_unmasked_invalid.case.json"
    );
    let input = Fixture::from_json(bytes).unwrap();
    assert_eq!(input.fixture.profile, Profile::LongMinssInvalidControlsV1);
    let mut missing: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    missing
        .as_object_mut()
        .unwrap()
        .remove("minss_invalid_controls");
    assert!(Fixture::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());
    for field in [
        "sse_available",
        "cr0_em",
        "cr0_ts",
        "cr4_osfxsr",
        "cr4_osxmmexcpt",
    ] {
        let mut missing_field: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        missing_field["minss_invalid_controls"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(Fixture::from_json(&serde_json::to_vec(&missing_field).unwrap()).is_err());
    }
    let mut missing_sse: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    missing_sse["initial_state"]
        .as_object_mut()
        .unwrap()
        .remove("sse");
    assert!(Fixture::from_json(&serde_json::to_vec(&missing_sse).unwrap()).is_err());
    let mut wrong_code: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    wrong_code["memory"][0]["initial_bytes_hex"] = serde_json::Value::String("f30f5dc0".into());
    assert!(Fixture::from_json(&serde_json::to_vec(&wrong_code).unwrap()).is_err());
    for mxcsr in [
        "0x0000000000001e00", // missing precision mask
        "0x0000000000001f40", // DAZ
        "0x0000000100001f00", // reserved bit
    ] {
        let mut invalid_mxcsr: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        invalid_mxcsr["initial_state"]["sse"]["mxcsr"] = serde_json::Value::String(mxcsr.into());
        assert!(Fixture::from_json(&serde_json::to_vec(&invalid_mxcsr).unwrap()).is_err());
    }

    let mut left = result(&input);
    snapshot(&mut left).state.sse.as_mut().unwrap().mxcsr = HexU64(0x1f01);
    let encoded = left.to_json().unwrap();
    assert_eq!(RunResult::from_json(&encoded).unwrap(), left);
    let mut missing_result_control = left.clone();
    snapshot(&mut missing_result_control).minss_invalid_controls = None;
    assert!(missing_result_control.to_json().is_err());
    let mut right = left.clone();
    snapshot(&mut right)
        .minss_invalid_controls
        .as_mut()
        .unwrap()
        .cr4_osxmmexcpt = true;
    let mismatch = compare(&input, &left, &right).unwrap();
    assert!(mismatch.contains(&"minss_invalid_controls".to_owned()));
    assert!(mismatch.contains(&"right.minss_invalid_controls".to_owned()));

    let older = result(&loaded());
    let json: serde_json::Value = serde_json::from_slice(&older.to_json().unwrap()).unwrap();
    assert!(
        json["snapshot"]["value"]
            .get("minss_invalid_controls")
            .is_none()
    );
}

#[test]
fn minss_unmasked_denormal_profile_has_versioned_mxcsr_and_snapshot_contract() {
    let bytes = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38d_minss_masked_invalid_unmasked_denormal.case.json"
    );
    let input = Fixture::from_json(bytes).unwrap();
    assert_eq!(input.fixture.profile, Profile::LongMinssUnmaskedDenormalV1);
    assert_eq!(
        input.fixture.minss_invalid_controls.unwrap().cr4_osxmmexcpt,
        true
    );

    let template: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    for im in [false, true] {
        for dm in [false, true] {
            for rc in 0..4u32 {
                let mut candidate = template.clone();
                let base = 0x1e00 | (u32::from(im) << 7) | (u32::from(dm) << 8) | (rc << 13) | 0x35;
                candidate["initial_state"]["sse"]["mxcsr"] =
                    serde_json::Value::String(format!("0x{base:016x}"));
                assert!(
                    Fixture::from_json(&serde_json::to_vec(&candidate).unwrap()).is_ok(),
                    "IM={im}, DM={dm}, RC={rc}"
                );
            }
        }
    }
    for mxcsr in [
        "0x0000000100001e00", // reserved bit
        "0x0000000000001e40", // DAZ
        "0x0000000000001c00", // missing ZE mask
    ] {
        let mut invalid: serde_json::Value = template.clone();
        invalid["initial_state"]["sse"]["mxcsr"] = serde_json::Value::String(mxcsr.into());
        assert!(Fixture::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }

    let mut missing_control: serde_json::Value = template.clone();
    missing_control
        .as_object_mut()
        .unwrap()
        .remove("minss_invalid_controls");
    assert!(Fixture::from_json(&serde_json::to_vec(&missing_control).unwrap()).is_err());
    let mut wrong_instruction: serde_json::Value = template;
    wrong_instruction["memory"][0]["initial_bytes_hex"] =
        serde_json::Value::String("f30f5dc0".into());
    assert!(Fixture::from_json(&serde_json::to_vec(&wrong_instruction).unwrap()).is_err());

    let mut left = result(&input);
    snapshot(&mut left).state.sse.as_mut().unwrap().mxcsr = HexU64(0x1e83);
    let wire = left.to_json().unwrap();
    assert_eq!(RunResult::from_json(&wire).unwrap(), left);
    let mut missing_snapshot_control = left.clone();
    snapshot(&mut missing_snapshot_control).minss_invalid_controls = None;
    assert!(missing_snapshot_control.to_json().is_err());
    let mut right = left.clone();
    snapshot(&mut right)
        .minss_invalid_controls
        .as_mut()
        .unwrap()
        .cr4_osxmmexcpt = false;
    let differences = compare(&input, &left, &right).unwrap();
    assert!(differences.contains(&"minss_invalid_controls".to_owned()));

    // The S2-38c profile name and stricter DM-mask contract remain unchanged.
    let old = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38c_minss_unmasked_invalid.case.json"
    ))
    .unwrap();
    assert_eq!(old.fixture.profile, Profile::LongMinssInvalidControlsV1);
    assert!(old.fixture.initial_state.sse.as_ref().unwrap().mxcsr.get() & 0x100 != 0);
}

#[test]
fn s2_38e_fixture_is_exact_and_image_mxcsr_controls_minss_policy() {
    let bytes = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38e_fp_init_fxrstor64_minss.case.json"
    );
    let loaded = Fixture::from_json(bytes).unwrap();
    assert_eq!(
        loaded.fixture.profile,
        Profile::LongCr0Cr4FpInitFxrstor64MinssV1
    );
    let mut baseline: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let base_hex = baseline["memory"][1]["initial_bytes_hex"]
        .as_str()
        .unwrap()
        .to_owned();
    let base = decode_bytes(&base_hex).unwrap();

    // Same independently-authored restore image and operands; changing only
    // IM changes arithmetic #XM into masked completion.
    let mut im_masked = base.clone();
    let old_mxcsr = u32::from_le_bytes(im_masked[24..28].try_into().unwrap());
    im_masked[24..28].copy_from_slice(&(old_mxcsr | (1 << 7)).to_le_bytes());
    baseline["memory"][1]["initial_bytes_hex"] =
        serde_json::Value::String(crate::memory::encode_bytes(&im_masked));
    baseline["expected_exit"] = serde_json::Value::String("stop_address".into());
    assert!(Fixture::from_json(&serde_json::to_vec(&baseline).unwrap()).is_ok());
    assert_eq!(
        u32::from_le_bytes(base[24..28].try_into().unwrap())
            ^ u32::from_le_bytes(im_masked[24..28].try_into().unwrap()),
        1 << 7
    );

    // Same operands with only DM toggled; a positive subnormal is masked or
    // raises #XM according to the restored image's DM bit.
    let mut dm_pair: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    let mut subnormal_image = base.clone();
    subnormal_image[160..164].copy_from_slice(&1u32.to_le_bytes());
    subnormal_image[176..180].copy_from_slice(&0x3f80_0000u32.to_le_bytes());
    let mxcsr = u32::from_le_bytes(subnormal_image[24..28].try_into().unwrap()) & !(1 << 7);
    subnormal_image[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    dm_pair["memory"][1]["initial_bytes_hex"] =
        serde_json::Value::String(crate::memory::encode_bytes(&subnormal_image));
    dm_pair["expected_exit"] = serde_json::Value::String("exception".into());
    assert!(Fixture::from_json(&serde_json::to_vec(&dm_pair).unwrap()).is_ok());
    let mut dm_masked = subnormal_image.clone();
    dm_masked[24..28].copy_from_slice(&(mxcsr | (1 << 8)).to_le_bytes());
    dm_pair["memory"][1]["initial_bytes_hex"] =
        serde_json::Value::String(crate::memory::encode_bytes(&dm_masked));
    dm_pair["expected_exit"] = serde_json::Value::String("stop_address".into());
    assert!(Fixture::from_json(&serde_json::to_vec(&dm_pair).unwrap()).is_ok());
    assert_eq!(
        u32::from_le_bytes(subnormal_image[24..28].try_into().unwrap())
            ^ u32::from_le_bytes(dm_masked[24..28].try_into().unwrap()),
        1 << 8
    );

    let qnan_fault = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38e_fp_init_fxrstor64_minss.case.json"
    ))
    .unwrap();
    let qnan_masked = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38e_minss_masked_qnan.case.json"
    ))
    .unwrap();
    let qnan_left = decode_bytes(&qnan_fault.fixture.memory[1].initial_bytes_hex).unwrap();
    let qnan_right = decode_bytes(&qnan_masked.fixture.memory[1].initial_bytes_hex).unwrap();
    assert!(
        qnan_fault.fixture.memory[0].initial_bytes_hex
            == qnan_masked.fixture.memory[0].initial_bytes_hex
    );
    assert!(
        qnan_left
            .iter()
            .zip(&qnan_right)
            .enumerate()
            .all(|(index, (a, b))| {
                if (24..28).contains(&index) {
                    *a ^ *b == if index == 24 { 0x80 } else { 0 }
                } else {
                    a == b
                }
            })
    );
    let subnormal_fault = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38e_unmasked_subnormal.case.json"
    ))
    .unwrap();
    let subnormal_masked_fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38e_masked_subnormal.case.json"
    ))
    .unwrap();
    let subnormal_left =
        decode_bytes(&subnormal_fault.fixture.memory[1].initial_bytes_hex).unwrap();
    let subnormal_right =
        decode_bytes(&subnormal_masked_fixture.fixture.memory[1].initial_bytes_hex).unwrap();
    assert!(
        subnormal_fault.fixture.memory[0].initial_bytes_hex
            == subnormal_masked_fixture.fixture.memory[0].initial_bytes_hex
    );
    assert!(
        subnormal_left
            .iter()
            .zip(&subnormal_right)
            .enumerate()
            .all(|(index, (a, b))| {
                if (24..28).contains(&index) {
                    *a ^ *b == if index == 25 { 1 } else { 0 }
                } else {
                    a == b
                }
            })
    );

    let mut unexpected_controls: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    unexpected_controls["minss_invalid_controls"] = serde_json::json!({
        "sse_available": true,
        "cr0_em": false,
        "cr0_ts": false,
        "cr4_osfxsr": true,
        "cr4_osxmmexcpt": true
    });
    assert!(Fixture::from_json(&serde_json::to_vec(&unexpected_controls).unwrap()).is_err());
    let mut wrong_tail: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    wrong_tail["memory"][0]["initial_bytes_hex"] = serde_json::Value::String(format!(
        "{}f30f5dc0",
        &wrong_tail["memory"][0]["initial_bytes_hex"]
            .as_str()
            .unwrap()[..66]
    ));
    assert!(Fixture::from_json(&serde_json::to_vec(&wrong_tail).unwrap()).is_err());
    let make_result = |input: &LoadedFixture| {
        let mut cpu = input
            .fixture
            .initial_state
            .to_cpu_for(input.fixture.profile)
            .unwrap();
        let mut memory = crate::memory::FlatMemory::from_fixture(&input.fixture).unwrap();
        let execution = od_isa::execute::run_with_cr4_fxrstor64_minss_profile(
            &mut cpu,
            &mut memory,
            od_isa::execute::RunConfig {
                instruction_budget: input.fixture.instruction_budget,
                stop_rip: input.fixture.stop_rip.map(HexU64::get),
                context: od_isa::memory::AccessContext {
                    privilege: od_isa::memory::PrivilegeLevel::Ring0,
                    ..od_isa::memory::AccessContext::default()
                },
            },
            || false,
        );
        let mut output = result(input);
        output.outcome = match execution.exit {
            od_isa::execute::RunExit::StopAddress => Outcome::StopAddress {},
            od_isa::execute::RunExit::Fault(_) => Outcome::Exception {},
            other => panic!("unexpected local S2-38e result: {other:?}"),
        };
        output.instructions_executed = Observation::Available {
            value: HexU64(execution.instructions_executed),
        };
        let is_exception = matches!(output.outcome, Outcome::Exception {});
        let snap = snapshot(&mut output);
        snap.state = IntegerState::from_cpu_for(&cpu, input.fixture.profile).unwrap();
        snap.rflags_defined = HexU64(execution.rflags_defined);
        snap.memory = memory.snapshot();
        if is_exception {
            snap.exception = Observation::Available {
                value: Some(crate::result::ExceptionSnapshot {
                    vector: 19,
                    error_code: Observation::Unavailable {
                        reason: "not modeled for #XM".into(),
                    },
                    faulting_rip: HexU64(input.fixture.initial_state.rip.get() + 33),
                    fault_address: Observation::Unavailable {
                        reason: "not applicable to #XM".into(),
                    },
                }),
            };
        }
        output
    };
    let expected = make_result(&loaded);
    let encoded = expected.to_json();
    assert!(encoded.is_ok(), "S2-38e result validation: {encoded:?}");
    assert!(compare(&loaded, &expected, &expected).unwrap().is_empty());

    for success_input in [qnan_masked, subnormal_masked_fixture] {
        let output = make_result(&success_input);
        assert_eq!(output.outcome, Outcome::StopAddress {});
        assert_eq!(
            output.instructions_executed,
            Observation::Available { value: HexU64(10) }
        );
        assert!(
            compare(&success_input, &output, &output)
                .unwrap()
                .is_empty()
        );
    }
    let mut wrong_status = expected.clone();
    let state = snapshot(&mut wrong_status);
    let old_mxcsr = state.state.sse.as_ref().unwrap().mxcsr.get();
    state.state.sse.as_mut().unwrap().mxcsr = HexU64(old_mxcsr ^ 4);
    assert!(compare(&loaded, &wrong_status, &wrong_status).is_err());
}

#[test]
fn orpd_profile_is_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_19_orpd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongOrpdRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450f56c1");
    for bytes in [
        "0f56c1", // prefix-free ORPS
        "f2660f56c1",
        "f3660f56c1",
        "67660f56c1",
        "f0660f56c1",
        "2e660f56c1",
        "0f6656c1",
        "66480f56c1",
        "66420f56c1",
        "66660f56c1",
        "6640410f56c1",
        "660f5601",
        "660f56",
        "c5f956c1",
        "62f17d0856c1",
        "660f54c1", // ANDPD
        "660f55c1", // ANDNPD
        "660f57c1", // XORPD
        "660febc1", // POR
    ] {
        let mut wrong = fixture.clone();
        wrong.memory[0].initial_bytes_hex = bytes.into();
        wrong.memory[0].length = (bytes.len() / 2) as u32;
        wrong.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        wrong.stop_rip = Some(HexU64(
            wrong.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(wrong.validate().is_err(), "{bytes}");
    }
}

#[test]
fn andnpd_profile_is_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_20_andnpd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongAndnpdRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450f55c1");
    for bytes in [
        "0f55c1", // legacy encoding without mandatory 66 is outside profile
        "f2660f55c1",
        "f3660f55c1",
        "67660f55c1",
        "f0660f55c1",
        "2e660f55c1",
        "0f6655c1",
        "66480f55c1",
        "66420f55c1",
        "66660f55c1",
        "6640410f55c1",
        "660f5501",
        "660f55",
        "c5f955c1",
        "62f17d0855c1",
        "660f54c1", // ANDPD
        "660f56c1", // ORPD
        "660f57c1", // XORPD
        "0f55c1",   // ANDNPS
        "660fdbc1", // PAND
        "660fdfc1", // PANDN
    ] {
        let mut wrong = fixture.clone();
        wrong.memory[0].initial_bytes_hex = bytes.into();
        wrong.memory[0].length = (bytes.len() / 2) as u32;
        wrong.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        wrong.stop_rip = Some(HexU64(
            wrong.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(wrong.validate().is_err(), "{bytes}");
    }
}

#[test]
fn xorps_profile_is_separate_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_7a_xorps_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongXorpsRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "450f57c1");
    let mut bare = fixture.clone();
    bare.memory[0].initial_bytes_hex = "0f57c1".into();
    bare.memory[0].length = 3;
    bare.memory[0].comparison_mask_hex = "ffffff".into();
    bare.stop_rip = Some(HexU64(bare.initial_state.rip.get() + 3));
    bare.validate().unwrap();
    for bytes in [
        "660f57c1", // mandatory prefix selects another instruction family
        "f20f57c1",
        "f30f57c1",
        "670f57c1",
        "f00f57c1",
        "480f57c1",   // REX.W
        "420f57c1",   // REX.X
        "40410f57c1", // duplicate REX
        "0f5701",     // memory operand
        "0f57",       // truncated ModRM
        "c5f857c1",   // VEX
    ] {
        let mut wrong_program = fixture.clone();
        wrong_program.memory[0].initial_bytes_hex = bytes.to_owned();
        wrong_program.memory[0].length = (bytes.len() / 2) as u32;
        wrong_program.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        wrong_program.stop_rip = Some(HexU64(
            wrong_program.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(wrong_program.validate().is_err(), "{bytes}");
    }
    let mut wrong_profile = fixture.clone();
    wrong_profile.profile = Profile::LongMovupsRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pxor_profile_is_separate_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_1_pxor_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPxorRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fefc1");
    for bytes in [
        "0fefc1",
        "f2660fefc1",
        "f3660fefc1",
        "67660fefc1",
        "f0660fefc1",
        "66480fefc1",
        "66420fefc1",
        "6640410fefc1",
        "660fef01",
        "660fef",
        "c5f8efc1",
        "c4e178efc1",
        "62f17c08efc1",
    ] {
        let mut wrong_program = fixture.clone();
        wrong_program.memory[0].initial_bytes_hex = bytes.to_owned();
        wrong_program.memory[0].length = (bytes.len() / 2) as u32;
        wrong_program.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        wrong_program.stop_rip = Some(HexU64(
            wrong_program.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(wrong_program.validate().is_err(), "{bytes}");
    }
    let mut wrong_profile = fixture.clone();
    wrong_profile.profile = Profile::LongMovupsRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pand_profile_is_separate_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_49_pand_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPandRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fdbc1");
    fixture.validate().unwrap();
    for bytes in [
        "0fdbc1", // MMX
        "f2660fdbc1",
        "f3660fdbc1",
        "67660fdbc1",
        "f0660fdbc1",
        "66480fdbc1", // REX.W
        "66420fdbc1", // REX.X
        "6640410fdbc1",
        "660fefc1", // PXOR
        "660fdfc1", // PANDN
        "660febc1", // POR
        "660fdb01", // memory
        "660fdb",
        "c5f9dbc1",
        "c4e179dbc1",
        "62f17d08dbc1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPxorRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pandn_profile_is_separate_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_50_pandn_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPandnRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fdfc1");
    fixture.validate().unwrap();
    for bytes in [
        "0fdfc1", // MMX
        "f2660fdfc1",
        "f3660fdfc1",
        "67660fdfc1",
        "f0660fdfc1",
        "66480fdfc1", // REX.W
        "66420fdfc1", // REX.X
        "6640410fdfc1",
        "660fefc1", // PXOR
        "660fdbc1", // PAND
        "660febc1", // POR
        "660fdf01", // memory
        "660fdf",   // truncated
        "660fdbc1", // PAND sibling
        "660fdb01", // PAND memory sibling
        "c5f9dfc1",
        "c4e179dfc1",
        "62f17d08dfc1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPandRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn por_profile_is_separate_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_51_por_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPorRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450febc1");
    fixture.validate().unwrap();
    for bytes in [
        "0febc1", // MMX
        "f2660febc1",
        "f3660febc1",
        "67660febc1",
        "f0660febc1",
        "66480febc1",   // REX.W
        "66420febc1",   // REX.X
        "6640410febc1", // duplicate REX
        "660feb01",     // memory
        "660feb",       // truncated
        "660fefc1",     // PXOR sibling
        "660fdbc1",     // PAND sibling
        "660fdfc1",     // PANDN sibling
        "c5f9ebc1",     // VEX
        "c4e179ebc1",   // VEX3
        "62f17d08ebc1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPandRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pavgb_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_52_pavgb_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPavgbRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fe0c1");
    fixture.validate().unwrap();
    for bytes in [
        "0fe0c1", // MMX
        "f2660fe0c1",
        "f3660fe0c1",
        "67660fe0c1",
        "f0660fe0c1",
        "66480fe0c1",   // REX.W
        "66420fe0c1",   // REX.X
        "6640410fe0c1", // duplicate REX
        "660fe001",     // memory
        "660fe0",       // truncated
        "660fe3c1",     // PAVGW sibling
        "660febc1",     // POR sibling
        "c5f9e0c1",     // VEX
        "c4e179e0c1",   // VEX3
        "62f17d08e0c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPorRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pavgw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_53_pavgw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPavgwRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fe3c1");
    fixture.validate().unwrap();
    for bytes in [
        "0fe3c1", // MMX
        "f2660fe3c1",
        "f3660fe3c1",
        "67660fe3c1",
        "f0660fe3c1",
        "66480fe3c1",   // REX.W
        "66420fe3c1",   // REX.X
        "6640410fe3c1", // duplicate REX
        "660fe301",     // memory
        "660fe3",       // truncated
        "660fe0c1",     // PAVGB sibling
        "660febc1",     // POR sibling
        "c5f9e3c1",     // VEX
        "c4e179e3c1",   // VEX3
        "62f17d08e3c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPavgbRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pmaxub_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_54_pmaxub_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPmaxubRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fdec1");
    fixture.validate().unwrap();
    for bytes in [
        "0fdec1", // MMX
        "f2660fdec1",
        "f3660fdec1",
        "67660fdec1",
        "f0660fdec1",
        "66480fdec1",   // REX.W
        "66420fdec1",   // REX.X
        "6640410fdec1", // duplicate REX
        "660fde01",     // memory
        "660fde",       // truncated
        "660fe3c1",     // PAVGW sibling
        "660febc1",     // POR sibling
        "c5f9dec1",     // VEX
        "c4e179dec1",   // VEX3
        "62f17d08dec1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPavgwRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pminub_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_55_pminub_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPminubRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fdac1");
    fixture.validate().unwrap();
    for bytes in [
        "0fdac1", // MMX
        "f2660fdac1",
        "f3660fdac1",
        "67660fdac1",
        "f0660fdac1",
        "66480fdac1",   // REX.W
        "66420fdac1",   // REX.X
        "6640410fdac1", // duplicate REX
        "660fda01",     // memory
        "660fda",       // truncated
        "660fdec1",     // PMAXUB sibling
        "660febc1",     // POR sibling
        "c5f9dac1",     // VEX
        "c4e179dac1",   // VEX3
        "62f17d08dac1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPmaxubRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pmaxsw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_56_pmaxsw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPmaxswRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450feec1");
    fixture.validate().unwrap();
    for bytes in [
        "0feec1", // MMX
        "f2660feec1",
        "f3660feec1",
        "67660feec1",
        "f0660feec1",
        "66480feec1",   // REX.W
        "66420feec1",   // REX.X
        "6640410feec1", // duplicate REX
        "660fee01",     // memory
        "660fee",       // truncated
        "660fdac1",     // PMINUB sibling
        "660febc1",     // POR sibling
        "c5f9eec1",     // VEX
        "c4e179eec1",   // VEX3
        "62f17d08eec1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPminubRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pminsw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_57_pminsw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPminswRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450feac1");
    fixture.validate().unwrap();
    for bytes in [
        "0feac1", // MMX
        "f2660feac1",
        "f3660feac1",
        "67660feac1",
        "f0660feac1",
        "66480feac1",   // REX.W
        "66420feac1",   // REX.X
        "6640410feac1", // duplicate REX
        "660fea01",     // memory
        "660fea",       // truncated
        "660feec1",     // PMAXSW sibling
        "660febc1",     // POR sibling
        "c5f9eac1",     // VEX
        "c4e179eac1",   // VEX3
        "62f17d08eac1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPmaxswRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pmullw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_58_pmullw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPmullwRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fd5c1");
    fixture.validate().unwrap();
    for bytes in [
        "0fd5c1", // MMX
        "f2660fd5c1",
        "f3660fd5c1",
        "67660fd5c1",
        "f0660fd5c1",
        "66480fd5c1",   // REX.W
        "66420fd5c1",   // REX.X
        "6640410fd5c1", // duplicate REX
        "660fd501",     // memory
        "660fd5",       // truncated
        "660feac1",     // PMAXSW sibling
        "660febc1",     // POR sibling
        "c5f9d5c1",     // VEX
        "c4e179d5c1",   // VEX3
        "62f17d08d5c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPminswRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pmulhuw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_59_pmulhuw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPmulhuwRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fe4c1");
    fixture.validate().unwrap();
    for bytes in [
        "0fe4c1", // MMX
        "f2660fe4c1",
        "f3660fe4c1",
        "67660fe4c1",
        "f0660fe4c1",
        "66480fe4c1",   // REX.W
        "66420fe4c1",   // REX.X
        "6640410fe4c1", // duplicate REX
        "660fe401",     // memory
        "660fe4",       // truncated
        "660fd5c1",     // PMAXSW sibling
        "660febc1",     // POR sibling
        "c5f9e4c1",     // VEX
        "c4e179e4c1",   // VEX3
        "62f17d08e4c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPmullwRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pmuludq_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_62_pmuludq_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPmuludqRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450ff4c1");
    fixture.validate().unwrap();
    for bytes in [
        "0ff4c1", // MMX
        "f2660ff4c1",
        "f3660ff4c1",
        "67660ff4c1",
        "f0660ff4c1",
        "66480ff4c1",   // REX.W
        "66420ff4c1",   // REX.X
        "6640410ff4c1", // duplicate REX
        "660ff401",     // memory
        "660ff4",       // truncated
        "660fe4c1",     // PMAXSW sibling
        "660febc1",     // POR sibling
        "c5f9f4c1",     // VEX
        "c4e179f4c1",   // VEX3
        "62f17d08f4c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPmulhuwRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pmaddwd_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_63_pmaddwd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPmaddwdRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450ff5c1");
    fixture.validate().unwrap();
    for bytes in [
        "0ff5c1", // MMX
        "f2660ff5c1",
        "f3660ff5c1",
        "67660ff5c1",
        "f0660ff5c1",
        "66480ff5c1",   // REX.W
        "66420ff5c1",   // REX.X
        "6640410ff5c1", // duplicate REX
        "660ff501",     // memory
        "660ff5",       // truncated
        "660ff4c1",     // PMULUDQ sibling
        "660febc1",     // POR sibling
        "c5f9f5c1",     // VEX
        "c4e179f5c1",   // VEX3
        "62f17d08f5c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPmuludqRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn psadbw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_60_psadbw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPsadbwRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450ff6c1");
    fixture.validate().unwrap();
    for bytes in [
        "0ff6c1", // MMX
        "f2660ff6c1",
        "f3660ff6c1",
        "67660ff6c1",
        "f0660ff6c1",
        "66480ff6c1",   // REX.W
        "66420ff6c1",   // REX.X
        "6640410ff6c1", // duplicate REX
        "660ff601",     // memory
        "660ff6",       // truncated
        "660fe4c1",     // PMAXSW sibling
        "660febc1",     // POR sibling
        "c5f9f6c1",     // VEX
        "c4e179f6c1",   // VEX3
        "62f17d08f6c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPmulhuwRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn pmovmskb_xmm_gpr_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_61_pmovmskb_xmm_gpr.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPmovmskbXmmGprV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fd7c1");
    fixture.validate().unwrap();
    for bytes in [
        "0fd7c1", // MMX
        "f2660fd7c1",
        "f3660fd7c1",
        "67660fd7c1",
        "f0660fd7c1",
        "66480fd7c1",   // REX.W
        "66420fd7c1",   // REX.X
        "6640410fd7c1", // duplicate REX
        "660fd701",     // memory
        "660fd7",       // truncated
        "660ff6c1",     // PMAXSW sibling
        "660febc1",     // POR sibling
        "c5f9d7c1",     // VEX
        "c4e179d7c1",   // VEX3
        "62f17d08d7c1", // EVEX
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPsadbwRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn paddb_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_2_paddb_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPaddbRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450ffcc1");
    fixture.validate().unwrap();
    for bytes in [
        "0ffcc1", // MMX
        "f2660ffcc1",
        "f3660ffcc1",
        "67660ffcc1",
        "f0660ffcc1",
        "66480ffcc1", // REX.W
        "66420ffcc1", // REX.X
        "6640410ffcc1",
        "660ffc01", // memory
        "660ffc",
        "c5f9fcc1",
        "c4e179fcc1",
        "62f17d08fcc1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPxorRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn psubb_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_3_psubb_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPsubbRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450ff8c1");
    fixture.validate().unwrap();
    for bytes in [
        "0ff8c1", // MMX
        "f2660ff8c1",
        "f3660ff8c1",
        "67660ff8c1",
        "f0660ff8c1",
        "66480ff8c1", // REX.W
        "66420ff8c1", // REX.X
        "6640410ff8c1",
        "660ff801", // memory
        "660ff8",
        "c5f9f8c1",
        "c4e179f8c1",
        "62f17d08f8c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPxorRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn paddusb_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_4_paddusb_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPaddusbRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fdcc1");
    fixture.validate().unwrap();
    for bytes in [
        "0fdcc1", // MMX
        "f2660fdcc1",
        "f3660fdcc1",
        "67660fdcc1",
        "f0660fdcc1",
        "66480fdcc1", // REX.W
        "66420fdcc1", // REX.X
        "6640410fdcc1",
        "660fdc01", // memory
        "660fdc",
        "c5f9dcc1",
        "c4e179dcc1",
        "62f17d08dcc1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPxorRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn packuswb_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_33_packuswb_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPackuswbRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450f67c1");
    fixture.validate().unwrap();
    for bytes in [
        "0f67c1", // MMX
        "f2660f67c1",
        "f3660f67c1",
        "67660f67c1",
        "f0660f67c1",
        "66480f67c1", // REX.W
        "66420f67c1", // REX.X
        "6640410f67c1",
        "660f6701", // memory
        "660f67",
        "c5f967c1",
        "c4e17967c1",
        "62f17d0867c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPxorRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn packssdw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_34_packssdw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPackssdwRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450f6bc1");
    fixture.validate().unwrap();
    for bytes in [
        "0f6bc1",
        "f2660f6bc1",
        "f3660f6bc1",
        "67660f6bc1",
        "f0660f6bc1",
        "66480f6bc1",
        "66420f6bc1",
        "6640410f6bc1",
        "660f6b01",
        "660f6b",
        "c5f96bc1",
        "c4e1796bc1",
        "62f17d086bc1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.to_owned();
        let len = (bytes.len() / 2) as u32;
        invalid.memory[0].length = len;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(len as usize);
        invalid.stop_rip = Some(HexU64(invalid.initial_state.rip.get() + u64::from(len)));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing = fixture.clone();
    missing.initial_state.sse = None;
    assert!(missing.validate().is_err());
    let mut wrong = fixture;
    wrong.profile = Profile::LongPackuswbRegV1;
    assert!(wrong.validate().is_err());
}

#[test]
fn andpd_profile_is_exact_and_rejects_neighboring_logic_forms() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_18_andpd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongAndpdRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450f54c1");
    for bytes in [
        "0f54c1",
        "f2660f54c1",
        "f3660f54c1",
        "67660f54c1",
        "66480f54c1",
        "66420f54c1",
        "66660f54c1",
        "6666450f54c1",
        "66450f5401",
        "66450f54",
        "66c5f854c1",
        "66c4e17854c1",
        "6262",
        "660fdbc1",
        "66450fdbc1",
        "660f55c1",
        "660f56c1",
        "660f57c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        let len = (bytes.len() / 2) as u32;
        invalid.memory[0].length = len;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(len as usize);
        invalid.stop_rip = Some(HexU64(invalid.initial_state.rip.get() + u64::from(len)));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_state = fixture.clone();
    missing_state.initial_state.sse = None;
    assert!(missing_state.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongXorpsRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn andps_profile_is_separate_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_7b_andps_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongAndpsRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "450f54c1");
    let mut bare = fixture.clone();
    bare.memory[0].initial_bytes_hex = "0f54c1".into();
    bare.memory[0].length = 3;
    bare.memory[0].comparison_mask_hex = "ffffff".into();
    bare.stop_rip = Some(HexU64(bare.initial_state.rip.get() + 3));
    bare.validate().unwrap();
    for bytes in [
        "660f54c1",
        "f20f54c1",
        "f30f54c1",
        "670f54c1",
        "f00f54c1",
        "480f54c1",
        "420f54c1",
        "40410f54c1",
        "0f5401",
        "0f54",
        "c5f854c1",
        "c4e17854c1",
        "62f17c0854c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut wrong_profile = fixture.clone();
    wrong_profile.profile = Profile::LongXorpsRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn psubusb_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_5_psubusb_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPsubusbRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fd8c1");
    fixture.validate().unwrap();
    for bytes in [
        "0fd8c1", // MMX
        "f2660fd8c1",
        "f3660fd8c1",
        "67660fd8c1",
        "f0660fd8c1",
        "66480fd8c1", // REX.W
        "66420fd8c1", // REX.X
        "6640410fd8c1",
        "660fd801", // memory
        "660fd8",
        "c5f9d8c1",
        "c4e179d8c1",
        "62f17d08d8c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut wrong_profile = fixture;
    wrong_profile.profile = Profile::LongPxorRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn paddw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_6_paddw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPaddwRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450ffdc1");
    fixture.validate().unwrap();
    for bytes in [
        "0ffdc1",
        "f2660ffdc1",
        "f3660ffdc1",
        "67660ffdc1",
        "f0660ffdc1",
        "66480ffdc1",
        "6642 0ffdc1",
        "6640410ffdc1",
        "660ffd01",
        "660ffd",
        "c5f9fdc1",
        "c4e179fdc1",
        "62f17d08fdc1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.replace(' ', "");
        let len = (invalid.memory[0].initial_bytes_hex.len() / 2) as u32;
        invalid.memory[0].length = len;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(len as usize);
        invalid.stop_rip = Some(HexU64(invalid.initial_state.rip.get() + u64::from(len)));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing = fixture.clone();
    missing.initial_state.sse = None;
    assert!(missing.validate().is_err());
    let mut wrong = fixture;
    wrong.profile = Profile::LongPaddbRegV1;
    assert!(wrong.validate().is_err());
}

#[test]
fn psubw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_7_psubw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPsubwRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450ff9c1");
    fixture.validate().unwrap();
    for bytes in [
        "0ff9c1",
        "f2660ff9c1",
        "f3660ff9c1",
        "67660ff9c1",
        "f0660ff9c1",
        "66480ff9c1",
        "6642 0ff9c1",
        "6640410ff9c1",
        "660ff901",
        "660ff9",
        "c5f9f9c1",
        "c4e179f9c1",
        "62f17d08f9c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.replace(' ', "");
        let len = (invalid.memory[0].initial_bytes_hex.len() / 2) as u32;
        invalid.memory[0].length = len;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(len as usize);
        invalid.stop_rip = Some(HexU64(invalid.initial_state.rip.get() + u64::from(len)));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing = fixture.clone();
    missing.initial_state.sse = None;
    assert!(missing.validate().is_err());
    let mut wrong = fixture;
    wrong.profile = Profile::LongPaddbRegV1;
    assert!(wrong.validate().is_err());
}

#[test]
fn paddusw_profile_is_exact_requires_raw_state_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_8_paddusw_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongPadduswRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "66450fddc1");
    fixture.validate().unwrap();
    for bytes in [
        "0fddc1",
        "f2660fddc1",
        "f3660fddc1",
        "67660fddc1",
        "f0660fddc1",
        "66480fddc1",
        "6642 0fddc1",
        "6640410fddc1",
        "660fdd01",
        "660fdd",
        "c5f9ddc1",
        "c4e179ddc1",
        "62f17d08ddc1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.replace(' ', "");
        let len = (invalid.memory[0].initial_bytes_hex.len() / 2) as u32;
        invalid.memory[0].length = len;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(len as usize);
        invalid.stop_rip = Some(HexU64(invalid.initial_state.rip.get() + u64::from(len)));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing = fixture.clone();
    missing.initial_state.sse = None;
    assert!(missing.validate().is_err());
    let mut wrong = fixture;
    wrong.profile = Profile::LongPaddbRegV1;
    assert!(wrong.validate().is_err());
}

#[test]
fn orps_profile_is_separate_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_7c_orps_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongOrpsRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "450f56c1");
    let mut bare = fixture.clone();
    bare.memory[0].initial_bytes_hex = "0f56c1".into();
    bare.memory[0].length = 3;
    bare.memory[0].comparison_mask_hex = "ffffff".into();
    bare.stop_rip = Some(HexU64(bare.initial_state.rip.get() + 3));
    bare.validate().unwrap();
    for bytes in [
        "660f56c1",
        "f20f56c1",
        "f30f56c1",
        "670f56c1",
        "f00f56c1",
        "480f56c1",
        "420f56c1",
        "40410f56c1",
        "0f5601",
        "0f56",
        "c5f856c1",
        "c4e17856c1",
        "62f17c0856c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut wrong_profile = fixture.clone();
    wrong_profile.profile = Profile::LongAndpsRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn andnps_profile_is_separate_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_7d_andnps_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongAndnpsRegV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "450f55c1");
    let mut bare = fixture.clone();
    bare.memory[0].initial_bytes_hex = "0f55c1".into();
    bare.memory[0].length = 3;
    bare.memory[0].comparison_mask_hex = "ffffff".into();
    bare.stop_rip = Some(HexU64(bare.initial_state.rip.get() + 3));
    bare.validate().unwrap();
    for bytes in [
        "660f55c1",
        "f20f55c1",
        "f30f55c1",
        "670f55c1",
        "f00f55c1",
        "480f55c1",
        "420f55c1",
        "40410f55c1",
        "0f5501",
        "0f55",
        "c5f855c1",
        "c4e17855c1",
        "62f17c0855c1",
    ] {
        let mut invalid = fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut wrong_profile = fixture.clone();
    wrong_profile.profile = Profile::LongOrpsRegV1;
    assert!(wrong_profile.validate().is_err());
}

#[test]
fn movss_store_register_profile_is_separate_exact_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_8a_movss_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut store = fixture.clone();
    store.profile = Profile::LongMovssStoreRegV1;
    store.memory[0].initial_bytes_hex = "f3450f11c1".into();
    store.memory[0].comparison_mask_hex = "ffffffffff".into();
    store.validate().unwrap();

    let mut bare = store.clone();
    bare.memory[0].initial_bytes_hex = "f30f11c1".into();
    bare.memory[0].length = 4;
    bare.memory[0].comparison_mask_hex = "ffffffff".into();
    bare.stop_rip = Some(HexU64(bare.initial_state.rip.get() + 4));
    bare.validate().unwrap();

    for bytes in [
        "f3450f10c1", // opposite MOVSS direction
        "f30f1101",   // memory form
        "f20f11c1",
        "660f11c1",
        "67f30f11c1",   // address-size override
        "2ef30f11c1",   // segment override
        "f0f30f11c1",   // LOCK
        "f3f30f11c1",   // duplicate mandatory prefix
        "40f30f11c1",   // REX before mandatory prefix
        "f3480f11c1",   // REX.W
        "f3420f11c1",   // REX.X
        "f340410f11c1", // duplicate REX
        "c5f811c1",     // VEX
        "c4e17a11c1",   // VEX
        "62f17e0811c1", // EVEX
        "f3450f11",     // truncated five-byte encoding
        "f30f11",       // truncated four-byte encoding
    ] {
        let mut invalid = store.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut load_profile = fixture;
    load_profile.memory[0].initial_bytes_hex = "f3450f11c1".into();
    assert!(load_profile.validate().is_err());
}

#[test]
fn movsd_register_profile_is_exact_f2_legacy_register_form() {
    let original = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_9a_movsd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(original.profile, Profile::LongMovsdRegV1);
    original.validate().unwrap();

    for bytes in [
        "f20f1001",     // memory source form
        "f30f10c1",     // MOVSS
        "660f10c1",     // MOVUPD
        "0f10c1",       // MOVUPS
        "f20f11c1",     // reverse direction
        "f2480f10c1",   // REX.W
        "f2420f10c1",   // REX.X
        "40f20f10c1",   // REX before mandatory F2
        "f240410f10c1", // duplicate REX
        "c5f810c1",     // VEX MOVUPS-like encoding
        "c4e17b10c1",   // VEX MOVSD
        "62f17f0810c1", // EVEX MOVSD
        "f2a5",         // string MOVSD is separate
        "f20f1000",     // ModR/M memory operand
    ] {
        let mut fixture = original.clone();
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(fixture.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movsd_store_register_profile_is_exact_f2_legacy_reverse_register_form() {
    let original = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_9b_movsd_store_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(original.profile, Profile::LongMovsdStoreRegV1);
    original.validate().unwrap();

    for bytes in [
        "f20f1101",     // memory destination
        "f30f11c1",     // MOVSS store
        "660f11c1",     // MOVUPD store
        "0f11c1",       // MOVUPS store
        "f20f10c1",     // forward direction
        "f2480f11c1",   // REX.W
        "f2420f11c1",   // REX.X
        "40f20f11c1",   // REX before mandatory F2
        "f240410f11c1", // duplicate REX
        "c5f811c1",     // VEX
        "c4e17b11c1",   // VEX MOVSD
        "62f17f0811c1", // EVEX
        "f2a5",         // string MOVSD is separate
        "f20f1100",     // ModR/M memory operand
    ] {
        let mut fixture = original.clone();
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(fixture.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movups_profile_admits_both_directions_and_rejects_out_of_profile_forms() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_2_movups_register.case.json"
    ))
    .unwrap()
    .fixture;
    let store_fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_3_movups_store_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(store_fixture.memory[0].initial_bytes_hex, "0f11c1");
    for bytes in [
        "660f10c1", // 66 selects MOVUPD in the adjacent SSE2 family.
        "f20f10c1",
        "f30f10c1",
        "2e0f10c1",
        "670f10c1",
        "f00f10c1",
        "40410f10c1", // duplicate REX
        "480f10c1",   // REX.W is excluded by this local form
        "420f10c1",   // REX.X is excluded by this local form
        "0f1001",     // memory ModR/M form
        "400f10",     // truncated before ModR/M
        "4c0f10c1",   // REX.W plus REX.R is excluded by this profile.
        "660f11c1",
        "f20f11c1",
        "f30f11c1",
        "2e0f11c1",
        "670f11c1",
        "f00f11c1",
        "40410f11c1",
        "480f11c1",
        "420f11c1",
        "0f1101",
        "400f11",
        "4c0f11c1",
        "c5f811c1",
        "c4e17811c1",
        "62f17c0811c1",
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        let length = (bytes.len() / 2) as u32;
        code.length = length;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(rejected.initial_state.rip.get() + u64::from(length)));
        assert!(rejected.validate().is_err(), "{bytes}");
    }

    for bytes in ["0f11c1", "450f11c1"] {
        let mut accepted = store_fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        let length = (bytes.len() / 2) as u32;
        code.length = length;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(accepted.initial_state.rip.get() + u64::from(length)));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }

    let mut too_many_xmms = serde_json::to_value(&fixture).unwrap();
    too_many_xmms["initial_state"]["sse"]["xmm"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::Value::String("00".repeat(16)));
    assert!(Fixture::from_json(&serde_json::to_vec(&too_many_xmms).unwrap()).is_err());
}

#[test]
fn movaps_register_profile_is_distinct_and_rejects_neighboring_encodings() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_11_movaps_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovapsRegV1);
    for bytes in [
        "0f2801",       // memory
        "0f29c1",       // reverse-direction MOVAPS
        "660f28c1",     // MOVAPD
        "f20f28c1",     // F2
        "f30f28c1",     // F3
        "640f28c1",     // segment override
        "670f28c1",     // address override
        "f00f28c1",     // LOCK
        "40410f28c1",   // duplicate REX
        "480f28c1",     // REX.W
        "420f28c1",     // REX.X
        "0f10c1",       // MOVUPS
        "c5f828c1",     // VEX
        "62f17c0828c1", // EVEX
        "400f28",       // truncated
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movaps_reverse_register_profile_is_distinct_and_rejects_neighboring_encodings() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_12_movaps_reverse_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovapsReverseRegV1);
    for bytes in [
        "0f2900",       // memory
        "0f28c1",       // forward-direction MOVAPS
        "0f11c1",       // MOVUPS
        "660f29c1",     // MOVAPD
        "66660f29c1",   // duplicate legacy prefix
        "f20f29c1",     // F2
        "f30f29c1",     // F3
        "640f29c1",     // segment override
        "64640f29c1",   // duplicate segment prefix
        "670f29c1",     // address override
        "f00f29c1",     // LOCK
        "40410f29c1",   // duplicate REX
        "480f29c1",     // REX.W
        "420f29c1",     // REX.X
        "c5f829c1",     // VEX
        "62f17c0829c1", // EVEX
        "400f29",       // truncated
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movapd_register_profile_requires_mandatory_66_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_13_movapd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovapdRegV1);
    for bytes in [
        "66450f28c1", // accepted independent REX.R/B axes
        "660f28c1",   // accepted low registers
    ] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    for bytes in [
        "0f28c1",       // no mandatory 66
        "660f29c1",     // reverse MOVAPD
        "660f10c1",     // MOVUPD
        "660f2800",     // memory
        "f2660f28c1",   // F2
        "f3660f28c1",   // F3
        "66660f28c1",   // duplicate 66
        "6640400f28c1", // duplicate REX
        "40660f28c1",   // REX before legacy prefix
        "64660f28c1",   // segment
        "67660f28c1",   // address size
        "f0660f28c1",   // LOCK
        "66480f28c1",   // REX.W
        "66420f28c1",   // REX.X
        "c5f928c1",     // VEX
        "62f1fd0828c1", // EVEX
        "66450f28",     // truncated
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movupd_register_profile_requires_mandatory_66_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_15_movupd_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovupdRegV1);
    for bytes in ["66450f10c1", "660f10c1"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    for bytes in [
        "0f10c1",       // MOVUPS without the mandatory 66 prefix.
        "660f11c1",     // Reverse direction is a different encoding.
        "660f28c1",     // MOVAPD.
        "660f1000",     // Memory operand.
        "f20f10c1",     // MOVSD scalar form.
        "f30f10c1",     // MOVSS scalar form.
        "66f20f10c1",   // F2 in addition to mandatory 66.
        "66f30f10c1",   // F3 in addition to mandatory 66.
        "66660f10c1",   // Duplicate mandatory prefix.
        "40660f10c1",   // REX before legacy prefix.
        "26660f10c1",   // CS segment prefix.
        "2e660f10c1",   // CS segment prefix.
        "36660f10c1",   // SS segment prefix.
        "3e660f10c1",   // DS segment prefix.
        "64660f10c1",   // Segment prefix.
        "65660f10c1",   // GS segment prefix.
        "67660f10c1",   // Address-size prefix.
        "f0660f10c1",   // LOCK.
        "66480f10c1",   // REX.W.
        "66420f10c1",   // REX.X.
        "6640450f10c1", // Duplicate REX.
        "c5f910c1",     // VEX.
        "62f1fd0810c1", // EVEX.
        "66450f10",     // Truncated ModR/M.
        "66450f10c1f2", // More than one instruction.
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movupd_reverse_register_profile_requires_mandatory_66_and_rejects_neighbors() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_16_movupd_reverse_register.case.json"
    ))
    .unwrap();
    let fixture = loaded.fixture;
    assert_eq!(fixture.profile, Profile::LongMovupdReverseRegV1);
    for bytes in [
        "0f11c1",
        "660f10c1",
        "660f29c1",
        "0f10c1",
        "f20f11c1",
        "f30f11c1",
        "660f1100",
        "66660f11c1",
        "40660f11c1",
        "26660f11c1",
        "2e660f11c1",
        "36660f11c1",
        "3e660f11c1",
        "64660f11c1",
        "65660f11c1",
        "67660f11c1",
        "f0660f11c1",
        "66480f11c1",
        "66420f11c1",
        "c5f911c1",
        "62f1fd0811c1",
        "66450f11",
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movapd_reverse_register_profile_requires_mandatory_66_and_rejects_neighbors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_14_movapd_reverse_register.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovapdReverseRegV1);
    for bytes in ["66450f29c1", "660f29c1"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    for bytes in [
        "0f29c1",         // missing mandatory 66 (MOVAPS is a separate profile)
        "0f10c1",         // MOVUPS load form
        "0f11c1",         // MOVUPS store form
        "660f28c1",       // forward MOVAPD
        "660f10c1",       // MOVUPD register load form
        "660f11c1",       // MOVUPD
        "660f1000",       // MOVUPD memory load
        "660f2900",       // MOVAPD memory
        "f2660f29c1",     // F2
        "f3660f29c1",     // F3
        "66660f29c1",     // duplicate 66
        "40660f29c1",     // misplaced 66 after REX
        "64660f29c1",     // segment
        "67660f29c1",     // address override
        "f0660f29c1",     // LOCK
        "66480f29c1",     // REX.W
        "66420f29c1",     // REX.X
        "6640400f29c1",   // duplicate REX
        "66c5f929c1",     // VEX
        "6662f1fd0829c1", // EVEX
        "66450f29",       // truncated
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movups_memory_load_profile_is_separate_and_exactly_bounds_rax_source() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_4_movups_memory_load_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovupsMemLoadV1);
    for bytes in ["0f1000", "0f1038", "440f1000", "440f1038"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }

    for bytes in [
        "0f1001",         // [RCX]
        "0f104000",       // displacement
        "0f108000000000", // disp32
        "0f100400",       // SIB
        "410f1000",       // REX.B / [R8]
        "420f1000",       // REX.X
        "480f1000",       // REX.W
        "660f1000",
        "f20f1000",
        "f30f1000",
        "f00f1000",
        "670f1000",
        "2e0f1000",
        "40440f1000",   // duplicate REX
        "0f1100",       // store direction
        "0f10c0",       // register source
        "c5f81000",     // VEX
        "c4e1781000",   // VEX3
        "62f17c081000", // EVEX
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }

    let mut missing_data = fixture.clone();
    missing_data.memory.pop();
    assert!(missing_data.validate().is_err());
    let mut writable_data = fixture.clone();
    writable_data.memory[1].permissions.write = true;
    assert!(writable_data.validate().is_err());
    let mut short_data = fixture;
    short_data.memory[1].length = 16;
    short_data.memory[1].initial_bytes_hex.truncate(32);
    short_data.memory[1].comparison_mask_hex.truncate(32);
    assert!(short_data.validate().is_err());
}

#[test]
fn movupd_memory_load_profile_is_exact_and_rejects_neighboring_forms() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_21_movupd_memory_load_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovupdMemLoadV1);
    for bytes in ["660f1000", "66400f1000", "66440f1000"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }

    for bytes in [
        "660f10c0",         // register form
        "660f1100",         // reverse/store form
        "0f1000",           // MOVUPS without mandatory 66
        "f2660f1000",       // scalar MOVSD prefix
        "f3660f1000",       // scalar MOVSS prefix
        "66660f1000",       // duplicate mandatory prefix
        "40660f1000",       // invalid prefix order
        "6640440f1000",     // duplicate REX prefix
        "660f104000",       // disp8
        "660f108000000000", // disp32
        "660f100400",       // SIB
        "660f100500000000", // RIP-relative
        "66410f1000",       // REX.B
        "66420f1000",       // REX.X
        "66480f1000",       // REX.W
        "67660f1000",       // address size override
        "c5f91000",         // VEX
        "c4e1791000",       // VEX3
        "62f17d081000",     // EVEX
        "660f10",           // truncated ModR/M
        "660f",             // truncated opcode
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movupd_memory_store_profile_is_exact_and_rejects_neighboring_forms() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_22_movupd_memory_store_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovupdMemStoreV1);
    for bytes in ["660f1100", "66400f1100", "66440f1100"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    let mut bad_budget = fixture.clone();
    bad_budget.instruction_budget = 0;
    assert!(bad_budget.validate().is_err());
    let mut bad_stop = fixture.clone();
    bad_stop.stop_rip = Some(HexU64(bad_stop.initial_state.rip.get() + 6));
    assert!(bad_stop.validate().is_err());
    for bytes in [
        "660f1000",
        "660f11c0",
        "0f1100",
        "f2660f1100",
        "66660f1100",
        "40660f1100",
        "66410f1100",
        "66420f1100",
        "66480f1100",
        "660f114000",
        "660f110400",
        "660f110500000000",
        "67660f1100",
        "c5f91100",
        "62f17d081100",
        "660f11",
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
}

#[test]
fn movss_memory_load_profile_is_exact_f3_rax_m32_only() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_8c_movss_memory_load_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovssMemLoadV1);
    for bytes in ["f30f1000", "f3440f1000"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    for bytes in [
        "f20f1000",
        "66f30f1000",
        "f3410f1000",
        "f3420f1000",
        "f3480f1000",
        "f30f1001",
        "f30f100400",
        "f30f104000",
        "f30f1100",
        "f30f10c0",
        "f3440f1001",
        "f3c5f81000",
        "f362f17e081000",
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
    let mut short_data = fixture;
    short_data.memory[1].length = 3;
    short_data.memory[1].initial_bytes_hex.truncate(6);
    short_data.memory[1].comparison_mask_hex.truncate(6);
    assert!(short_data.validate().is_err());
}

#[test]
fn movsd_memory_load_profile_is_exact_f2_rax_m64_only() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_9c_movsd_memory_load_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovsdMemLoadV1);
    for bytes in ["f20f1000", "f2400f1000", "f2440f1000"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    for bytes in [
        "f20f1001",         // other base register
        "f20f1004",         // SIB
        "f20f104000",       // disp8
        "f20f108000000000", // disp32
        "f2410f1000",       // REX.B
        "f2420f1000",       // REX.X
        "f2480f1000",       // REX.W
        "f240410f1000",     // duplicate REX
        "40f20f1000",       // REX before mandatory prefix
        "f2f20f1000",       // duplicate mandatory prefix
        "66f20f1000",
        "f30f1000",
        "67f20f1000",
        "64f20f1000",
        "f0f20f1000",
        "f20f1100", // store direction
        "f20f10c0", // register form
        "f20f10",   // truncated ModR/M
        "c5fb1000", // VEX
        "c4e17b1000",
        "62f1ff081000", // EVEX
        "f2a5",         // string MOVSD
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }

    let mut short_data = fixture;
    let data = short_data
        .memory
        .iter_mut()
        .find(|region| region.region_id == "data")
        .unwrap();
    data.length = 8;
    data.initial_bytes_hex.truncate(16);
    data.comparison_mask_hex.truncate(16);
    assert!(short_data.validate().is_err());
}

#[test]
fn movsd_memory_store_profile_is_exact_f2_rax_m64_only() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_9d_movsd_memory_store_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovsdMemStoreV1);
    for bytes in ["f20f1100", "f2440f1100", "f2400f1100"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    for bytes in [
        "f20f1101",
        "f20f1104",
        "f20f114000",
        "f20f118000000000",
        "f2410f1100",
        "f2420f1100",
        "f2480f1100",
        "f240410f1100",
        "40f20f1100",
        "f2f20f1100",
        "66f20f1100",
        "f30f1100",
        "67f20f1100",
        "64f20f1100",
        "f0f20f1100",
        "f20f1000",
        "f20f11c0",
        "f20f11",
        "f2440f11",
        "f2a5",
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
    let mut read_only = fixture.clone();
    read_only.memory[1].permissions.write = false;
    assert!(read_only.validate().is_err());
    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
    let mut short_data = fixture;
    short_data.memory[1].length = 8;
    short_data.memory[1].initial_bytes_hex.truncate(16);
    short_data.memory[1].comparison_mask_hex.truncate(16);
    assert!(short_data.validate().is_err());
}

#[test]
fn movss_memory_store_profile_is_exact_f3_rax_m32_only() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_8d_movss_memory_store_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovssMemStoreV1);
    for bytes in ["f30f1100", "f3440f1100"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }
    for bytes in [
        "f20f1100",
        "66f30f1100",
        "f3410f1100",
        "f3420f1100",
        "f3480f1100",
        "f30f1101",
        "f30f110400",
        "f30f114000",
        "f30f1000",
        "f30f11c0",
        "f3440f1101",
        "40f30f1100",
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }
    let mut no_write = fixture.clone();
    no_write.memory[1].permissions.write = false;
    assert!(no_write.validate().is_err());
    let mut missing_data = fixture.clone();
    missing_data.memory.pop();
    assert!(missing_data.validate().is_err());
    let mut short_data = fixture;
    short_data.memory[1].length = 3;
    short_data.memory[1].initial_bytes_hex.truncate(6);
    short_data.memory[1].comparison_mask_hex.truncate(6);
    assert!(short_data.validate().is_err());
}

#[test]
fn movups_memory_store_profile_is_separate_and_exactly_bounds_rax_destination() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_5_movups_memory_store_rax.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongMovupsMemStoreV1);
    for bytes in ["0f1100", "0f1138", "440f1100", "440f1138"] {
        let mut accepted = fixture.clone();
        let code = accepted
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        accepted.stop_rip = Some(HexU64(
            accepted.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(accepted.validate().is_ok(), "{bytes}");
    }

    for bytes in [
        "0f1101",         // [RCX]
        "0f114000",       // displacement
        "0f118000000000", // disp32
        "0f110400",       // SIB
        "410f1100",       // REX.B / [R8]
        "420f1100",       // REX.X
        "480f1100",       // REX.W
        "660f1100",
        "f20f1100",
        "f30f1100",
        "f00f1100",
        "670f1100",
        "2e0f1100",
        "40440f1100",   // duplicate REX
        "0f1000",       // load direction
        "0f11c0",       // register destination
        "c5f81100",     // VEX
        "c4e1781100",   // VEX3
        "62f17c081100", // EVEX
    ] {
        let mut rejected = fixture.clone();
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        rejected.stop_rip = Some(HexU64(
            rejected.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(rejected.validate().is_err(), "{bytes}");
    }

    let mut readonly = fixture.clone();
    readonly.memory[1].permissions.write = false;
    assert!(readonly.validate().is_err());
    let mut state_only = fixture.clone();
    state_only.profile = Profile::LongX87SseStateV1;
    assert!(state_only.validate().is_err());
}

#[test]
fn movups_memory_base_profile_admits_base_disp_and_rejects_other_address_forms() {
    let original = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_5_movups_memory_store_rax.case.json"
    ))
    .unwrap()
    .fixture;
    for (bytes, base_register, base_value) in [
        ("0f1000", 0usize, 0x0020_0001u64),                // [RAX]
        ("410f1000", 8usize, 0x0020_0001),                 // [R8], no displacement
        ("0f1040f9", 0usize, 0x0020_0008),                 // [RAX-7]
        ("0f1145f9", 5usize, 0x0020_0008),                 // [RBP-7], SS default
        ("410f1045ff", 13usize, 0x0020_0002),              // [R13-1], DS default
        ("450f1140f9", 8usize, 0x0020_0008),               // REX.R+B, [R8-7], XMM8 source
        ("0f108000000080", 0usize, 0x8020_0000),           // signed disp32 minimum
        ("0f1080ffffff7f", 0usize, 0xffff_ffff_8020_0002), // signed disp32 maximum
    ] {
        for opcode in [0x10, 0x11] {
            let mut fixture = original.clone();
            fixture.profile = Profile::LongMovupsMemBaseV1;
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            let mut instruction = bytes
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect::<Vec<_>>();
            let opcode_index = if instruction[0] == 0x0f { 1 } else { 2 };
            instruction[opcode_index] = opcode;
            code.length = instruction.len() as u32;
            code.initial_bytes_hex = instruction
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            code.comparison_mask_hex = "ff".repeat(instruction.len());
            fixture.stop_rip = Some(HexU64(
                fixture.initial_state.rip.get() + instruction.len() as u64,
            ));
            fixture.initial_state.gpr[base_register] = HexU64(base_value);
            assert!(fixture.validate().is_ok(), "{bytes} opcode={opcode:#x}");
        }
    }

    for bytes in [
        "0f1004",         // truncated forbidden SIB
        "0f100400",       // SIB
        "0f100500000000", // RIP relative
        "420f1000",       // REX.X
        "480f1000",       // REX.W
        "670f1000",       // address-size override
        "440f1000", // truncated/no displacement REX.R is okay but old code shape has no rex.B; this remains valid below
        "0f1000",   // RAX-only form is valid in new profile
    ] {
        let mut fixture = original.clone();
        fixture.profile = Profile::LongMovupsMemBaseV1;
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + u64::from(code.length),
        ));
        let accepted = fixture.validate().is_ok();
        if matches!(bytes, "440f1000" | "0f1000") {
            assert!(accepted, "{bytes}");
        } else {
            assert!(!accepted, "{bytes}");
        }
    }
}

#[test]
fn movupd_memory_base_profile_is_separate_and_requires_66() {
    let original = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_6_movups_memory_base.case.json"
    ))
    .unwrap()
    .fixture;
    for xmm in 0u8..16 {
        for opcode in [0x10u8, 0x11] {
            let rex = if xmm >= 8 { vec![0x44] } else { Vec::new() };
            let mut bytes = vec![0x66];
            bytes.extend(rex);
            bytes.extend([0x0f, opcode, (xmm & 7) << 3]);
            let hex = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let mut fixture = original.clone();
            fixture.profile = Profile::LongMovupdMemBaseV1;
            let code = fixture
                .memory
                .iter_mut()
                .find(|r| r.region_id == "code")
                .unwrap();
            code.length = bytes.len() as u32;
            code.initial_bytes_hex = hex;
            code.comparison_mask_hex = "ff".repeat(bytes.len());
            fixture.stop_rip = Some(HexU64(
                fixture.initial_state.rip.get() + u64::from(code.length),
            ));
            assert!(fixture.validate().is_ok(), "xmm={xmm} opcode={opcode:#x}");
        }
    }

    for (bytes, base_index, base_value) in [
        ("660f1000", 0usize, 0x0020_0001u64),
        ("66440f1000", 0usize, 0x0020_0001),   // REX.R only
        ("66410f1040f9", 8usize, 0x0020_0008), // REX.B only
        ("66450f1140f9", 8usize, 0x0020_0008),
    ] {
        let mut fixture = original.clone();
        fixture.profile = Profile::LongMovupdMemBaseV1;
        fixture.initial_state.gpr[base_index] = HexU64(base_value);
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(
            fixture.validate().is_ok(),
            "{bytes}: {:?}",
            fixture.validate()
        );

        fixture.profile = Profile::LongMovupsMemBaseV1;
        assert!(fixture.validate().is_err(), "old profile admitted {bytes}");
    }

    for bad_encoding in [
        "0f1000",           // missing mandatory 66
        "66660f1000",       // duplicate mandatory prefix
        "41660f1000",       // REX before mandatory prefix
        "64660f1000",       // segment override
        "f3660f1000",       // scalar prefix
        "67660f1000",       // address-size override
        "66480f1000",       // REX.W
        "66420f1000",       // REX.X
        "660f10c0",         // register ModR/M
        "660f1004",         // truncated SIB selector
        "660f100400",       // SIB
        "660f1005",         // truncated RIP-relative disp32
        "660f1005f9ff0f00", // RIP-relative target reaches valid data but is excluded
        "660f1040",         // truncated disp8
        "660f1080000000",   // truncated disp32
    ] {
        let mut fixture = original.clone();
        fixture.profile = Profile::LongMovupdMemBaseV1;
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (bad_encoding.len() / 2) as u32;
        code.initial_bytes_hex = bad_encoding.into();
        code.comparison_mask_hex = "ff".repeat(bad_encoding.len() / 2);
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + u64::from(code.length),
        ));
        assert!(fixture.validate().is_err(), "admitted {bad_encoding}");
    }

    let mut missing_66 = original;
    missing_66.profile = Profile::LongMovupdMemBaseV1;
    assert!(missing_66.validate().is_err());

    for bytes in [
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_23_movupd_memory_base_load_r9_disp32.case.json").as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_23_movupd_memory_base_store_r8_disp8.case.json").as_slice(),
    ] {
        let fixture = Fixture::from_json(bytes).unwrap().fixture;
        assert_eq!(fixture.profile, Profile::LongMovupdMemBaseV1);
        assert!(fixture.validate().is_ok(), "{}", fixture.case_id);
        let mut old_exact = fixture;
        old_exact.profile = Profile::LongMovupdMemLoadV1;
        assert!(old_exact.validate().is_err());
    }
}

#[test]
fn movupd_sib_profile_checks_the_original_encoding_class() {
    for bytes in [
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_load_r9_r10_scale2.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_store_rax_r12_scale8.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_nobase_load.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_nobase_store.case.json"
        )
        .as_slice(),
    ] {
        let fixture = Fixture::from_json(bytes).unwrap().fixture;
        assert_eq!(fixture.profile, Profile::LongMovupdMemSibV1);
        assert!(fixture.validate().is_ok(), "{}", fixture.case_id);
        for old_profile in [
            Profile::LongMovupdMemLoadV1,
            Profile::LongMovupdMemStoreV1,
            Profile::LongMovupsMemLoadV1,
            Profile::LongMovupsMemStoreV1,
            Profile::LongMovupdMemBaseV1,
            Profile::LongMovupdMemRipV1,
            Profile::LongMovupsMemBaseV1,
        ] {
            let mut old = fixture.clone();
            old.profile = old_profile;
            assert!(old.validate().is_err(), "{old_profile:?} admitted SIB");
        }
    }

    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_load_r9_r10_scale2.case.json"
    ))
    .unwrap()
    .fixture;
    for malformed in [
        "660f1004",       // truncated SIB
        "66480f100408",   // REX.W
        "67660f100408",   // address-size override
        "66660f100408",   // duplicate mandatory prefix
        "41660f100408",   // REX before 66
        "f3660f100408",   // scalar prefix
        "660f100400",     // no mandatory 66 SIB class
        "66470f1004",     // SIB missing
        "66470f104451",   // missing disp8
        "66470f108485",   // missing disp32
        "c5f81004",       // VEX
        "62f17c08100408", // EVEX
    ] {
        let mut bad = template.clone();
        let code = bad
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = malformed.into();
        code.length = (malformed.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(malformed.len() / 2);
        bad.stop_rip = Some(HexU64(bad.initial_state.rip.get() + u64::from(code.length)));
        assert!(bad.validate().is_err(), "admitted {malformed}");
    }
}

#[test]
fn x87_schema_admits_fdivr_forms_and_inexact_no_stop_case() {
    let mut fixture = x87_fixture().fixture;
    fixture.case_id = "local_fdivr_exact".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x1002));
    fixture.expected_exit = ExpectedExit::StopAddress;
    {
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
    }
    for index in 0xF8u8..=0xFF {
        fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .initial_bytes_hex = format!("d8{index:02x}");
        fixture.validate().unwrap();
    }

    for destination in 0u8..=7 {
        fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .initial_bytes_hex = format!("dcf{destination:x}");
        if destination == 1 {
            fixture.stop_rip = None;
            fixture.expected_exit = ExpectedExit::Unsupported;
        } else {
            fixture.stop_rip = Some(HexU64(0x1002));
            fixture.expected_exit = ExpectedExit::StopAddress;
        }
        fixture.validate().unwrap();
    }

    for index in 0u8..=7 {
        fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .initial_bytes_hex = format!("dcf{:x}", 8 | index);
        if index == 1 {
            fixture.stop_rip = None;
            fixture.expected_exit = ExpectedExit::Unsupported;
        } else {
            fixture.stop_rip = Some(HexU64(0x1002));
            fixture.expected_exit = ExpectedExit::StopAddress;
        }
        fixture.validate().unwrap();
    }

    fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap()
        .initial_bytes_hex = "d8f9".into();
    fixture.stop_rip = None;
    fixture.expected_exit = ExpectedExit::Unsupported;
    fixture.validate().unwrap();
    fixture.stop_rip = Some(HexU64(0x1002));
    fixture.expected_exit = ExpectedExit::Unsupported;
    assert!(fixture.validate().is_err());

    fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap()
        .initial_bytes_hex = "dcf9".into();
    fixture.stop_rip = None;
    fixture.expected_exit = ExpectedExit::Unsupported;
    fixture.validate().unwrap();
    fixture.stop_rip = Some(HexU64(0x1002));
    assert!(fixture.validate().is_err());
}

#[test]
fn x87_schema_admits_d9_fst_ftst_fnstcw_and_a_budget_one_source_rejection() {
    let mut fixture = x87_fixture().fixture;
    fixture.case_id = "local_fabs".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x1002));
    fixture.expected_exit = ExpectedExit::StopAddress;
    {
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = 2;
        code.initial_bytes_hex = "d9e4".into();
        code.comparison_mask_hex = "ffff".into();
    }
    for second in 0u8..=u8::MAX {
        let mut candidate = fixture.clone();
        candidate
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .initial_bytes_hex = format!("d9{second:02x}");
        let accepted = candidate.validate().is_ok();
        assert_eq!(
            accepted,
            matches!(
                second,
                0x00 | 0x10 | 0x18 | 0x28 | 0x38 | 0xd0 | 0xc0..=0xc6
                    | 0xc8..=0xcf | 0xe0 | 0xe1 | 0xe4 | 0xe5 | 0xe8 | 0xee | 0xf6 | 0xf7 | 0xfc
            ),
            "D9 {second:02x}"
        );
    }
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());

    fixture.case_id = "local_ftst_invalid_source".into();
    fixture.stop_rip = None;
    fixture.expected_exit = ExpectedExit::Unsupported;
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());
}

#[test]
fn x87_fxch_st1_schema_admits_only_the_three_exact_streams() {
    let mut fixture = x87_fixture().fixture;
    fixture.case_id = "local_fxch_st1_seeded_top7".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x1002));
    fixture.expected_exit = ExpectedExit::StopAddress;
    {
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = 2;
        code.initial_bytes_hex = "d9c9".into();
        code.comparison_mask_hex = "ffff".into();
    }
    let x87 = fixture.initial_state.x87.as_mut().unwrap();
    x87.control_word = HexU64(0x037f);
    x87.status_word = HexU64(0x3a00); // TOP=7 and C1 set.
    x87.tag_word = HexU64(0);
    x87.registers[7] = "0000000000000080ff3f".into();
    x87.registers[0] = "00000000000000c00040".into();
    fixture.validate().unwrap();

    for source in 0u8..=7 {
        let exchange = format!("d9{:02x}", 0xc8 + source);
        for (code_bytes, length, budget) in [
            (exchange.clone(), 2, 1),
            (format!("{exchange}dfe0"), 4, 2),
            (format!("{exchange}{exchange}dfe0"), 6, 3),
        ] {
            let mut composition = fixture.clone();
            composition.case_id = format!("local_fxch_{code_bytes}");
            composition.instruction_budget = budget;
            composition.stop_rip = Some(HexU64(0x1000 + length));
            let code = composition
                .memory
                .iter_mut()
                .find(|r| r.region_id == "code")
                .unwrap();
            code.length = length as u32;
            code.initial_bytes_hex = code_bytes.into();
            code.comparison_mask_hex = "ff".repeat(length as usize);
            composition.validate().unwrap();
        }
    }

    for alias in ["d9c7", "d909", "66d9c9", "d9c8d9c9dfe0", "d9c8dfe0d9c9"] {
        let mut rejected = fixture.clone();
        rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap()
            .initial_bytes_hex = alias.into();
        assert!(rejected.validate().is_err(), "{alias}");
    }

    let mut wrong_tag = fixture.clone();
    wrong_tag.case_id = "local_fxch_st1_wrong_tag".into();
    wrong_tag.stop_rip = None;
    wrong_tag.expected_exit = ExpectedExit::Unsupported;
    wrong_tag.initial_state.x87.as_mut().unwrap().tag_word = HexU64(3);
    wrong_tag.validate().unwrap();

    let mut canonical_zero_is_not_a_reject = wrong_tag.clone();
    canonical_zero_is_not_a_reject.case_id = "local_fxch_canonical_zero_is_valid".into();
    let x87 = canonical_zero_is_not_a_reject
        .initial_state
        .x87
        .as_mut()
        .unwrap();
    x87.tag_word = HexU64(1 << 14);
    x87.registers[7] = "00000000000000000080".into();
    x87.registers[0] = "00000000000000c00040".into();
    canonical_zero_is_not_a_reject.stop_rip = Some(HexU64(0x1002));
    canonical_zero_is_not_a_reject.expected_exit = ExpectedExit::StopAddress;
    canonical_zero_is_not_a_reject.validate().unwrap();

    let mut canonical_zero_reject = canonical_zero_is_not_a_reject.clone();
    canonical_zero_reject.case_id = "local_fxch_canonical_zero_not_unsupported".into();
    canonical_zero_reject.stop_rip = None;
    canonical_zero_reject.instruction_budget = 1;
    canonical_zero_reject.expected_exit = ExpectedExit::Unsupported;
    assert!(canonical_zero_reject.validate().is_err());

    for (physical, malformed) in [
        (7usize, "01000000000000000000"), // Nonzero significand, exponent zero.
        (7usize, "00000000000000000001"), // Zero significand, nonzero exponent.
        (0usize, "01000000000000000000"),
        (0usize, "00000000000000000001"),
    ] {
        let mut malformed_zero = wrong_tag.clone();
        malformed_zero.case_id = format!("local_fxch_malformed_zero_{physical}_{malformed}");
        let x87 = malformed_zero.initial_state.x87.as_mut().unwrap();
        x87.tag_word = HexU64(1 << (physical * 2));
        x87.registers[physical] = malformed.into();
        malformed_zero.validate().unwrap();
    }
}

#[test]
fn x87_top_adjustment_schema_admits_only_the_six_exact_streams() {
    let mut fixture = x87_fixture().fixture;
    fixture.case_id = "local_top_adjustment_seeded_c1_mixed_tags".into();
    let x87 = fixture.initial_state.x87.as_mut().unwrap();
    x87.control_word = HexU64(0x037f);
    x87.status_word = HexU64(0x3a00); // TOP=7 and C1 set.
    x87.tag_word = HexU64(0xe4e4); // Mixed tags, including Empty.
    for physical in 0..8 {
        x87.registers[physical] = format!("{:020x}", 0x1020_3040_5060_7080u128 + physical as u128);
    }

    for (stream, budget) in [
        ("d9f7", 1),
        ("d9f6", 1),
        ("d9f7dfe0", 2),
        ("d9f6dfe0", 2),
        ("d9f7d9f6dfe0", 3),
        ("d9f6d9f7dfe0", 3),
    ] {
        let mut exact = fixture.clone();
        exact.case_id = format!("local_top_adjustment_{stream}");
        exact.instruction_budget = budget;
        exact.stop_rip = Some(HexU64(0x1000 + (stream.len() / 2) as u64));
        let code = exact
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (stream.len() / 2) as u32;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(stream.len() / 2);
        exact.validate().unwrap();
    }

    for stream in [
        "d9f5",
        "d9f8",
        "66d9f7",
        "d9f7d9f7dfe0",
        "d9f7d9f6",
        "d9f6dfe0d9f7",
    ] {
        let mut rejected = fixture.clone();
        rejected.case_id = format!("local_top_adjustment_alias_{stream}");
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (stream.len() / 2) as u32;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(stream.len() / 2);
        assert!(rejected.validate().is_err(), "{stream}");
    }

    for invalid_environment in [(0x0000, 0x3800), (0x027f, 0x3800), (0x037f, 0x3801)] {
        let mut rejected = fixture.clone();
        rejected.case_id = "local_top_adjustment_bad_environment".into();
        rejected.instruction_budget = 1;
        rejected.stop_rip = None;
        rejected.expected_exit = ExpectedExit::Unsupported;
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = 2;
        code.initial_bytes_hex = "d9f7".into();
        code.comparison_mask_hex = "ffff".into();
        let x87 = rejected.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(invalid_environment.0);
        x87.status_word = HexU64(invalid_environment.1);
        rejected.validate().unwrap();
    }
}

#[test]
fn x87_fld_sti_schema_admits_only_indexed_streams_and_bad_sources_reject() {
    let mut fixture = x87_fixture().fixture;
    fixture.case_id = "local_fld_st0_seeded_normal_top_wrap".into();
    let x87 = fixture.initial_state.x87.as_mut().unwrap();
    x87.control_word = HexU64(0x037f);
    x87.status_word = HexU64(0x3a00); // TOP=7, prior C1 set.
    x87.tag_word = HexU64(0xffff);
    x87.registers[7] = "0000000000000080ff3f".into();
    x87.tag_word = HexU64(x87.tag_word.get() & !(3 << 14)); // physical 7 Valid
    x87.tag_word = HexU64((x87.tag_word.get() & !(3 << 12)) | (3 << 12)); // push slot 6 Empty
    let code = fixture
        .memory
        .iter_mut()
        .find(|r| r.region_id == "code")
        .unwrap();
    code.length = 2;
    code.initial_bytes_hex = "d9c0".into();
    code.comparison_mask_hex = "ffff".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x1002));

    for source in 0u8..7 {
        for (stream, budget) in [
            (format!("d9c{source:x}"), 1),
            (format!("d9c{source:x}dfe0"), 2),
        ] {
            let mut exact = fixture.clone();
            exact.case_id = format!("local_fld_sti_{stream}");
            exact.instruction_budget = budget;
            let length = (stream.len() / 2) as u32;
            exact.stop_rip = Some(HexU64(0x1000 + u64::from(length)));
            let x87 = exact.initial_state.x87.as_mut().unwrap();
            x87.status_word = HexU64(0x0200); // TOP=0, prior C1 set.
            x87.tag_word = HexU64(u64::from(u16::MAX));
            x87.registers[usize::from(source)] = "0000000000000080ff3f".into();
            x87.tag_word = HexU64(x87.tag_word.get() & !(3 << (u32::from(source) * 2)));
            x87.tag_word = HexU64((x87.tag_word.get() & !(3 << 14)) | (3 << 14));
            let code = exact
                .memory
                .iter_mut()
                .find(|r| r.region_id == "code")
                .unwrap();
            code.length = length;
            code.initial_bytes_hex = stream.clone();
            code.comparison_mask_hex = "ff".repeat(length as usize);
            exact.validate().unwrap();
        }
    }
    for stream in ["d9e8d9c0dfe0", "d9eed9c0dfe0"] {
        let mut exact = fixture.clone();
        exact.case_id = format!("local_fld_st0_producer_{stream}");
        exact.instruction_budget = 3;
        exact.stop_rip = Some(HexU64(0x1006));
        let code = exact
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = 6;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(6);
        exact.validate().unwrap();
    }

    for stream in ["d9c7", "66d9c0", "d9c0d9c0dfe0", "d9e8d9c0"] {
        let mut rejected = fixture.clone();
        rejected.case_id = format!("local_fld_st0_alias_{stream}");
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (stream.len() / 2) as u32;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(stream.len() / 2);
        assert!(rejected.validate().is_err(), "{stream}");
    }
    for stream in ["d9c7", "66d9c1", "d9c1d9c1dfe0", "d9e8d9c1dfe0"] {
        let mut rejected = fixture.clone();
        rejected.case_id = format!("local_fld_sti_neighbor_{stream}");
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = (stream.len() / 2) as u32;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(stream.len() / 2);
        assert!(rejected.validate().is_err(), "{stream}");
    }

    let mut bad_source = fixture.clone();
    bad_source.case_id = "local_fld_st0_bad_source_reject".into();
    bad_source.stop_rip = None;
    bad_source.expected_exit = ExpectedExit::Unsupported;
    bad_source.instruction_budget = 1;
    bad_source.initial_state.x87.as_mut().unwrap().tag_word = HexU64(
        bad_source
            .initial_state
            .x87
            .as_ref()
            .unwrap()
            .tag_word
            .get()
            | (3 << 14),
    );
    bad_source.validate().unwrap();

    let mut occupied = fixture.clone();
    occupied.case_id = "local_fld_st0_occupied_destination_reject".into();
    occupied.stop_rip = None;
    occupied.expected_exit = ExpectedExit::Unsupported;
    occupied.instruction_budget = 1;
    occupied.initial_state.x87.as_mut().unwrap().tag_word =
        HexU64(occupied.initial_state.x87.as_ref().unwrap().tag_word.get() & !(3 << 12));
    occupied.validate().unwrap();
}

#[test]
fn x87_ffree_sti_schema_admits_only_the_nineteen_exact_streams() {
    let base = x87_fixture().fixture;
    let mut allowed = (0xc0u8..=0xc7)
        .map(|modrm| (format!("dd{modrm:02x}"), 1))
        .collect::<Vec<_>>();
    allowed.extend((0xc0u8..=0xc7).map(|modrm| (format!("dd{modrm:02x}dfe0"), 2)));
    allowed.extend([
        ("ddc0d9c0dfe0".into(), 3),
        ("ddc0d9f7d9e8dfe0".into(), 4),
        ("ddc7d9e8dfe0".into(), 3),
    ]);
    assert_eq!(allowed.len(), 19);
    for (stream, budget) in allowed {
        let mut fixture = base.clone();
        fixture.case_id = format!("local_ffree_sti_{stream}");
        fixture.instruction_budget = budget;
        let length = (stream.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(0x1000 + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = length;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(length as usize);
        fixture.validate().unwrap();
    }
    for stream in [
        "ddc8",
        "dd00",
        "66ddc0",
        "f3ddc7",
        "ddc0ddc0",
        "ddc0d9f7d9e8",
        "9bddc0",
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("local_ffree_st0_forbidden_{stream}");
        let length = (stream.len() / 2) as u32;
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.length = length;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(fixture.validate().is_err(), "{stream}");
    }
}

#[test]
fn x87_fnop_schema_admits_only_the_four_bounded_streams() {
    let base = x87_fixture().fixture;
    for (stream, budget) in [
        ("d9d0", 1),
        ("d9d0dfe0", 2),
        ("d9d0d9d0dfe0", 3),
        ("ddc7d9d0d9e8dfe0", 4),
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("local_fnop_{stream}");
        fixture.instruction_budget = budget;
        let length = (stream.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(0x1000 + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(length as usize);
        fixture.validate().unwrap();
    }
    for stream in [
        "9bd9d0",
        "66d9d0",
        "f2d9d0",
        "f3d9d0",
        "2ed9d0",
        "36d9d0",
        "3ed9d0",
        "26d9d0",
        "64d9d0",
        "65d9d0",
        "67d9d0",
        "40d9d0",
        "48d9d0",
        "d9d1",
        "d9d2",
        "d9d3",
        "d9d4",
        "d9d5",
        "d9d6",
        "d9d7",
        "d9d0d9d0",
        "ddc0d9d0d9e8dfe0",
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("local_fnop_forbidden_{stream}");
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (stream.len() / 2) as u32;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(stream.len() / 2);
        assert!(fixture.validate().is_err(), "{stream}");
    }
}

#[test]
fn x87_fxam_schema_admits_only_the_seven_requested_streams() {
    let base = x87_fixture().fixture;
    for (stream, budget) in [
        ("d9e5", 1),
        ("d9e5dfe0", 2),
        ("d9e5d9e5dfe0", 3),
        ("d9e8d9e5dfe0", 3),
        ("d9eed9e5dfe0", 3),
        ("d9e8d9e0d9e5dfe0", 4),
        ("d9eed9e0d9e5dfe0", 4),
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("local_fxam_{stream}");
        fixture.instruction_budget = budget;
        let length = (stream.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(0x1000 + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(length as usize);
        fixture.validate().unwrap();
    }
    for stream in [
        "9bd9e5", "66d9e5", "f2d9e5", "f3d9e5", "2ed9e5", "d9e4", "d9e6", "d9e5d9e5",
    ] {
        let mut fixture = base.clone();
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (stream.len() / 2) as u32;
        code.initial_bytes_hex = stream.into();
        code.comparison_mask_hex = "ff".repeat(stream.len() / 2);
        assert!(fixture.validate().is_err(), "{stream}");
    }
}

#[test]
fn fld_m32_m64_budget_one_normal_unsupported_rejects_admitted_sticky_status() {
    for (width, opcode, normal_bytes) in [(4, "d900", "0000803f"), (8, "dd00", "000000000000f03f")]
    {
        let mut fixture = x87_fixture().fixture;
        fixture.case_id = format!("local_fld_m{width}_normal_sf_rejection");
        fixture.instruction_budget = 1;
        fixture.stop_rip = None;
        fixture.expected_exit = ExpectedExit::Unsupported;
        fixture.initial_state.gpr[0] = HexU64(0x2000);
        {
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.length = 2;
            code.initial_bytes_hex = opcode.into();
            code.comparison_mask_hex = "ffff".into();
        }
        fixture.memory.push(MemoryRegionSpec {
            region_id: "normal_operand".into(),
            guest_address: HexU64(0x2000),
            length: width,
            permissions: MemoryPermissions {
                read: true,
                write: false,
                execute: false,
            },
            initial_bytes_hex: normal_bytes.into(),
            comparison_mask_hex: "ff".repeat(width as usize),
        });

        fixture.initial_state.x87.as_mut().unwrap().status_word = HexU64(0x0040);
        fixture.initial_state.x87.as_mut().unwrap().control_word = HexU64(0x037f);
        fixture.initial_state.x87.as_mut().unwrap().tag_word = HexU64(u16::MAX.into());
        assert!(
            fixture.validate().is_ok(),
            "{opcode} SF Unsupported fixture"
        );

        for admitted_status in [0x0004, 0x0008, 0x0041] {
            fixture.initial_state.x87.as_mut().unwrap().status_word = HexU64(admitted_status);
            assert!(
                fixture.validate().is_err(),
                "{opcode} admitted FSW={admitted_status:04x} must not justify Unsupported"
            );
        }
    }
}

#[test]
fn x87_schema_admits_only_fcom_sti_register_forms_and_two_operand_rejection() {
    let mut fixture = x87_fixture().fixture;
    fixture.case_id = "local_fcom_sti".into();
    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x1002));
    fixture.expected_exit = ExpectedExit::StopAddress;
    {
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = 2;
        code.initial_bytes_hex = "d8d0".into();
        code.comparison_mask_hex = "ffff".into();
    }
    for source in 0u8..=7 {
        fixture.case_id = format!("local_fcom_st{source}");
        fixture.stop_rip = Some(HexU64(0x1002));
        fixture.expected_exit = ExpectedExit::StopAddress;
        fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .initial_bytes_hex = format!("d8d{source:x}");
        fixture.validate().unwrap();

        fixture.stop_rip = None;
        fixture.expected_exit = ExpectedExit::Unsupported;
        fixture.validate().unwrap();
    }

    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d8d7dfe0".into();
    code.length = 4;
    code.comparison_mask_hex = "ffffffff".into();
    fixture.instruction_budget = 2;
    fixture.stop_rip = Some(HexU64(0x1004));
    fixture.expected_exit = ExpectedExit::StopAddress;
    fixture.validate().unwrap();

    fixture.instruction_budget = 1;
    fixture.stop_rip = Some(HexU64(0x1002));
    assert!(fixture.validate().is_err());
    for source in 0u8..=7 {
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = 2;
        code.initial_bytes_hex = format!("d8{:02x}", 0xd8 | source);
        code.comparison_mask_hex = "ffff".into();
        fixture.instruction_budget = 1;
        fixture.stop_rip = Some(HexU64(0x1002));
        fixture.expected_exit = ExpectedExit::StopAddress;
        fixture.validate().unwrap();

        fixture.stop_rip = None;
        fixture.expected_exit = ExpectedExit::Unsupported;
        fixture.validate().unwrap();

        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = 4;
        code.initial_bytes_hex = format!("d8{:02x}dfe0", 0xd8 | source);
        code.comparison_mask_hex = "ffffffff".into();
        fixture.instruction_budget = 2;
        fixture.stop_rip = Some(HexU64(0x1004));
        fixture.expected_exit = ExpectedExit::StopAddress;
        fixture.validate().unwrap();
    }
}

#[test]
fn conversion_is_explicit_and_has_its_own_digest() {
    let raw = include_bytes!("../../../tests/fixtures/x86/s0_09_mov_imm64.case.json");
    let old = crate::case::Fixture::from_json(raw).unwrap();
    let converted = loaded();
    assert_eq!(
        converted.fixture.converted_from_v1_sha256,
        Some(old.input_sha256.clone())
    );
    assert_ne!(converted.input_sha256, old.input_sha256);
    assert_eq!(crate::case::Fixture::from_json(raw).unwrap(), old);
    assert!(crate::case::Fixture::from_json(&converted.fixture.to_json().unwrap()).is_err());
    let mut fixture = converted.fixture;
    fixture.initial_state.gpr[15] = HexU64(u64::MAX);
    fixture.initial_state.rip = HexU64(u64::MAX);
    let roundtrip = Fixture::from_json(&fixture.to_json().unwrap()).unwrap();
    assert_eq!(
        roundtrip.fixture.initial_state.to_cpu().unwrap().gpr[15],
        u64::MAX
    );
    assert_eq!(roundtrip.fixture.initial_state.rip.get(), u64::MAX);
}

#[test]
fn malformed_versions_fields_profiles_and_state_are_rejected() {
    let base = serde_json::to_value(loaded().fixture).unwrap();
    for (path, value) in [
        (vec!["schema_version"], serde_json::json!(3)),
        (
            vec!["profile"],
            serde_json::json!("s0_09_register_subset_v1"),
        ),
        (vec!["initial_state", "mode"], serde_json::json!("real16")),
        (
            vec!["initial_state", "rflags"],
            serde_json::json!("0x0000000000000202"),
        ),
        (
            vec!["initial_state", "rip"],
            serde_json::json!("0x10000000000000000"),
        ),
        (
            vec!["initial_state", "gpr"],
            serde_json::json!(["0x0000000000000000"]),
        ),
        (vec!["unknown"], serde_json::json!(0)),
        (vec!["initial_state", "unknown"], serde_json::json!(0)),
    ] {
        let mut value_root = base.clone();
        let mut field = &mut value_root;
        for key in path {
            field = &mut field[key];
        }
        *field = value;
        assert!(Fixture::from_json(&serde_json::to_vec(&value_root).unwrap()).is_err());
    }
    let mut fixture = loaded().fixture;
    fixture.memory.push(fixture.memory[0].clone());
    assert!(fixture.validate().is_err());
    fixture.memory.pop();
    fixture.memory[0].guest_address = HexU64(u64::MAX);
    assert!(fixture.validate().is_err());
    assert!(Fixture::from_json(&vec![b' '; MAX_FIXTURE_JSON_BYTES + 1]).is_err());
}

#[test]
fn punpckhdq_profile_admits_only_the_literal_register_encoding() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_29_punpckhdq_register.case.json"
    ))
    .unwrap();
    input.fixture.validate().unwrap();

    let mut vex = input.fixture.clone();
    let code = vex
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 4;
    code.initial_bytes_hex = "c5f96ac1".into();
    code.comparison_mask_hex = "ffffffff".into();
    vex.stop_rip = Some(HexU64(vex.initial_state.rip.get() + 4));
    assert!(
        vex.validate().is_err(),
        "VEX encoding must not enter the legacy profile"
    );

    let mut low_dq = input.fixture.clone();
    low_dq
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap()
        .initial_bytes_hex = "66450f62c1".into();
    assert!(
        low_dq.validate().is_err(),
        "PUNPCKLDQ must remain a separate profile"
    );
}

#[test]
fn fs_base_profile_is_required_and_old_wire_shape_stays_unchanged() {
    let raw = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_21g_mfence_store_fence_load.case.json"
    );
    let old = Fixture::from_json(raw).unwrap();
    assert_eq!(
        old.input_sha256,
        "8477721f18892e8115b7dcbfc9260d43446b815bbc528767e331d1293a2e49a4"
    );
    assert_eq!(old.fixture.to_json().unwrap(), raw[..raw.len() - 1]);
    let old_json: serde_json::Value =
        serde_json::from_slice(&old.fixture.to_json().unwrap()).unwrap();
    assert!(old_json["initial_state"].get("fs_base").is_none());
    assert!(old_json["initial_state"].get("fs_selector").is_none());
    assert!(old_json["initial_state"].get("gs_selector").is_none());
    assert!(old_json["initial_state"].get("x87").is_none());
    assert_eq!(old.fixture.profile, Profile::LongIntegerV1);

    let fs_fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22a_fs_base_qword_load_alias.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fs_fixture.profile, Profile::LongFsBaseV1);
    assert!(fs_fixture.initial_state.fs_selector.is_none());
    assert!(fs_fixture.initial_state.gs_selector.is_none());
    assert_eq!(
        fs_fixture
            .initial_state
            .to_cpu_for(fs_fixture.profile)
            .unwrap()
            .fs_base,
        0xffff_8000_0000_0000
    );
    let mut missing = serde_json::to_value(&fs_fixture).unwrap();
    missing["initial_state"]
        .as_object_mut()
        .unwrap()
        .remove("fs_base");
    assert!(Fixture::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut noncanonical = serde_json::to_value(&fs_fixture).unwrap();
    noncanonical["initial_state"]["fs_base"] = serde_json::json!("0x0000800000000000");
    assert!(Fixture::from_json(&serde_json::to_vec(&noncanonical).unwrap()).is_err());

    let mut old_with_fs = serde_json::to_value(&old.fixture).unwrap();
    old_with_fs["initial_state"]["fs_base"] = serde_json::json!("0x0000000000001000");
    assert!(Fixture::from_json(&serde_json::to_vec(&old_with_fs).unwrap()).is_err());
    old_with_fs["initial_state"]["fs_base"] = serde_json::Value::Null;
    assert!(Fixture::from_json(&serde_json::to_vec(&old_with_fs).unwrap()).is_err());
}

#[test]
fn x87_state_profile_roundtrips_raw_empty_register_payloads_and_compares_mutations() {
    let input = x87_fixture();
    assert_eq!(input.fixture.profile, Profile::LongX87StateV1);
    let x87 = input.fixture.initial_state.x87.as_ref().unwrap();
    let old_snapshot_json = serde_json::to_value(x87).unwrap();
    assert!(old_snapshot_json.get("instruction_selector").is_none());
    assert!(old_snapshot_json.get("data_selector").is_none());
    let decoded = x87.to_cpu().unwrap();
    assert_eq!(X87Snapshot::from(&decoded), *x87);
    assert_eq!(
        decoded.registers[0],
        [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x10, 0x80]
    );
    assert_eq!(decoded.tag(7), Some(od_isa::state::X87Tag::Valid));

    let cpu = input
        .fixture
        .initial_state
        .to_cpu_for(Profile::LongX87StateV1)
        .unwrap();
    assert_eq!(cpu.x87.as_ref(), Some(&decoded));
    let returned = IntegerState::from_cpu_for(&cpu, Profile::LongX87StateV1).unwrap();
    assert_eq!(returned.x87.as_ref(), Some(x87));
    assert!(
        IntegerState::from_cpu_for(&cpu, Profile::LongIntegerV1)
            .unwrap()
            .x87
            .is_none()
    );

    let mut incompatible_cpu = cpu.clone();
    incompatible_cpu.x87 = None;
    assert!(
        IntegerState::from_cpu_for(&incompatible_cpu, Profile::LongX87StateV1)
            .unwrap()
            .x87
            .is_none()
    );

    let fixture_json = input.fixture.to_json().unwrap();
    let roundtrip = Fixture::from_json(&fixture_json).unwrap();
    assert_eq!(
        roundtrip.fixture.initial_state.x87,
        input.fixture.initial_state.x87
    );

    let mut missing_x87 = input.fixture.clone();
    missing_x87.initial_state.x87 = None;
    assert!(missing_x87.validate().is_err());
    let mut old_profile_with_x87 = loaded().fixture;
    old_profile_with_x87.initial_state.x87 = Some(x87.clone());
    assert!(old_profile_with_x87.validate().is_err());
    let mut widened = input.fixture.clone();
    widened.memory[0].initial_bytes_hex = "0f0b".into();
    widened.memory[0].length = 2;
    widened.memory[0].comparison_mask_hex = "ffff".into();
    assert!(widened.validate().is_err());

    let mut value: serde_json::Value = serde_json::from_slice(&fixture_json).unwrap();
    value["initial_state"]["x87"]["registers"][0] = serde_json::json!("0123456789abcdef108");
    assert!(Fixture::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&fixture_json).unwrap();
    value["initial_state"]["x87"]["last_opcode"] = serde_json::json!("0x0000000000000800");
    assert!(Fixture::from_json(&serde_json::to_vec(&value).unwrap()).is_err());

    let left = result(&input);
    let result_json = left.to_json().unwrap();
    assert_eq!(RunResult::from_json(&result_json).unwrap(), left);
    let mut right = left.clone();
    let changed = match &mut right.snapshot {
        Observation::Available { value } => value.state.x87.as_mut().unwrap(),
        _ => unreachable!(),
    };
    changed.registers[0].replace_range(0..2, "fe");
    assert!(
        compare(&input, &left, &right)
            .unwrap()
            .iter()
            .any(|field| field == "x87")
    );
}

#[test]
fn x87_state_profile_admits_only_exact_bare_two_byte_operations() {
    let mut fixture = x87_fixture().fixture;
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 2;
    code.initial_bytes_hex = "dbe3".into();
    code.comparison_mask_hex = "ffff".into();
    fixture.stop_rip = Some(HexU64(0x1002));
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());

    for second in 0xd0_u8..=0xdf {
        let mut seeded_store = fixture.clone();
        let code = seeded_store
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dd{second:02x}");
        seeded_store.stop_rip = Some(HexU64(0x1002));
        assert!(
            Fixture::from_json(&seeded_store.to_json().unwrap()).is_ok(),
            "bare DD D0+i / DD D8+i encoding {second:02x}"
        );
    }
    for bytes in ["ddd?", "40ddd1", "fddd1"] {
        let mut outside_register_stores = fixture.clone();
        let code = outside_register_stores
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = (bytes.len() / 2) as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        outside_register_stores.stop_rip = Some(HexU64(0x1000 + (bytes.len() / 2) as u64));
        assert!(
            Fixture::from_json(&serde_json::to_vec(&outside_register_stores).unwrap()).is_err()
        );
    }

    let mut fnstsw = fixture.clone();
    let code = fnstsw
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dfe0".into();
    assert!(Fixture::from_json(&fnstsw.to_json().unwrap()).is_ok());

    let mut fild_m16 = fixture.clone();
    let fild_code = fild_m16
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    fild_code.initial_bytes_hex = "df00".into();
    fild_code.comparison_mask_hex = "ffff".into();
    fild_m16.stop_rip = Some(HexU64(0x1002));
    assert!(Fixture::from_json(&fild_m16.to_json().unwrap()).is_ok());

    let mut fild_m16_fnstsw = fild_m16.clone();
    let fild_code = fild_m16_fnstsw
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    fild_code.length = 4;
    fild_code.initial_bytes_hex = "df00dfe0".into();
    fild_code.comparison_mask_hex = "ffffffff".into();
    fild_m16_fnstsw.instruction_budget = 2;
    fild_m16_fnstsw.stop_rip = Some(HexU64(0x1004));
    assert!(Fixture::from_json(&fild_m16_fnstsw.to_json().unwrap()).is_ok());

    for (bytes, length, stop_rip, budget) in [
        ("db28", 2_usize, 0x1002_u64, 1_u64),
        ("db28dfe0", 4_usize, 0x1004, 2),
    ] {
        let mut fld_m80 = fixture.clone();
        let code = fld_m80
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(length);
        fld_m80.instruction_budget = budget;
        fld_m80.stop_rip = Some(HexU64(stop_rip));
        assert!(Fixture::from_json(&fld_m80.to_json().unwrap()).is_ok());
    }

    for (bytes, length, stop_rip, budget) in [
        ("d900", 2_usize, 0x1002_u64, 1_u64),
        ("d900dfe0", 4_usize, 0x1004, 2),
    ] {
        let mut fld_m32 = fixture.clone();
        let code = fld_m32
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(length);
        fld_m32.instruction_budget = budget;
        fld_m32.stop_rip = Some(HexU64(stop_rip));
        assert!(Fixture::from_json(&fld_m32.to_json().unwrap()).is_ok());
    }
    for (bytes, length, stop_rip, budget) in [
        ("d900dd10", 4_usize, 0x1004_u64, 2_u64),
        ("d900dd18dfe0", 6_usize, 0x1006_u64, 3_u64),
    ] {
        let mut subnormal_store = fixture.clone();
        let code = subnormal_store
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length as u32;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(length);
        subnormal_store.instruction_budget = budget;
        subnormal_store.stop_rip = Some(HexU64(stop_rip));
        assert!(Fixture::from_json(&subnormal_store.to_json().unwrap()).is_ok());
    }
    let mut invalid_fld_m32 = fixture.clone();
    let code = invalid_fld_m32
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d900".into();
    code.comparison_mask_hex = "ffff".into();
    invalid_fld_m32.initial_state.gpr[0] = HexU64(0x0000_0000_0030_0004);
    invalid_fld_m32.memory.push(MemoryRegionSpec {
        region_id: "operand".into(),
        guest_address: HexU64(0x0000_0000_0030_0000),
        length: 8,
        permissions: MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        },
        initial_bytes_hex: "000000000000803f".into(),
        comparison_mask_hex: "ff".repeat(8),
    });
    invalid_fld_m32
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .control_word = HexU64(0x037f);
    invalid_fld_m32
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .status_word = HexU64(0x0040);
    invalid_fld_m32.stop_rip = None;
    invalid_fld_m32.expected_exit = ExpectedExit::Unsupported;
    assert!(Fixture::from_json(&invalid_fld_m32.to_json().unwrap()).is_ok());
    for raw in [
        "000000000000803f",
        "000000000100807f",
        "00000000010080ff",
        "000000000000c07f",
        "000000000000c0ff",
        "000000000100007f",
    ] {
        let mut neighbor = invalid_fld_m32.clone();
        neighbor
            .memory
            .iter_mut()
            .find(|region| region.region_id == "operand")
            .unwrap()
            .initial_bytes_hex = raw.into();
        assert!(
            Fixture::from_json(&serde_json::to_vec(&neighbor).unwrap()).is_ok(),
            "D900 invalid-environment predicate must not depend on source class {raw}"
        );
    }
    for combination in 0..64_u16 {
        let sticky = [0x0001, 0x0002, 0x0004, 0x0008, 0x0010, 0x0020]
            .into_iter()
            .enumerate()
            .fold(0_u16, |flags, (bit, status)| {
                flags
                    | if combination & (1 << bit) != 0 {
                        status
                    } else {
                        0
                    }
            });
        let mut sticky_environment_snan = invalid_fld_m32.clone();
        sticky_environment_snan
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(u64::from(sticky));
        assert!(
            sticky_environment_snan.validate().is_err(),
            "admitted sticky status {sticky:04x} must not admit Unsupported"
        );
    }
    for forbidden_status in [0x0040, 0x0080, 0x8000] {
        let mut forbidden_status_fixture = invalid_fld_m32.clone();
        forbidden_status_fixture
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(forbidden_status);
        assert!(
            Fixture::from_json(&serde_json::to_vec(&forbidden_status_fixture).unwrap()).is_ok(),
            "forbidden status {forbidden_status:04x} must admit pre-read Unsupported"
        );
    }
    let mut ze_only_fld_m32 = invalid_fld_m32.clone();
    ze_only_fld_m32
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .status_word = HexU64(0x0004);
    assert!(ze_only_fld_m32.validate().is_err());
    let mut wrong_fld_m32_control = invalid_fld_m32.clone();
    wrong_fld_m32_control
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .control_word = HexU64(0x027f);
    wrong_fld_m32_control
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .status_word = HexU64(0);
    assert!(Fixture::from_json(&wrong_fld_m32_control.to_json().unwrap()).is_ok());
    let mut wrong_fld_m32_budget = fixture.clone();
    let code = wrong_fld_m32_budget
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 4;
    code.initial_bytes_hex = "d900dfe0".into();
    code.comparison_mask_hex = "ffffffff".into();
    wrong_fld_m32_budget.instruction_budget = 1;
    wrong_fld_m32_budget.stop_rip = Some(HexU64(0x1004));
    assert!(Fixture::from_json(&serde_json::to_vec(&wrong_fld_m32_budget).unwrap()).is_err());

    let mut fld_m64 = fixture.clone();
    let code = fld_m64
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 2;
    code.initial_bytes_hex = "dd00".into();
    code.comparison_mask_hex = "ffff".into();
    fld_m64.initial_state.gpr[0] = HexU64(0x0000_0000_0030_0004);
    fld_m64.memory.push(MemoryRegionSpec {
        region_id: "operand".into(),
        guest_address: HexU64(0x0000_0000_0030_0004),
        length: 8,
        permissions: MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        },
        initial_bytes_hex: "0100000000000000".into(),
        comparison_mask_hex: "ff".repeat(8),
    });
    fld_m64.stop_rip = Some(HexU64(0x1002));
    fld_m64.expected_exit = ExpectedExit::StopAddress;
    assert!(Fixture::from_json(&fld_m64.to_json().unwrap()).is_ok());

    let mut invalid_fld_m64 = fld_m64.clone();
    invalid_fld_m64.stop_rip = None;
    invalid_fld_m64.expected_exit = ExpectedExit::Unsupported;
    invalid_fld_m64
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .control_word = HexU64(0x037f);
    invalid_fld_m64
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .status_word = HexU64(0x40);
    invalid_fld_m64
        .memory
        .iter_mut()
        .find(|region| region.region_id == "operand")
        .unwrap()
        .initial_bytes_hex = "000000000000f03f".into();
    assert!(Fixture::from_json(&invalid_fld_m64.to_json().unwrap()).is_ok());
    for raw in [
        "000000000000f03f",
        "000000000000f87f",
        "000000000000f07f",
        "010000000000f07f",
        "010000000000f0ff",
        "bc9a78563412f0ff",
        "fffffffffffff07f",
    ] {
        let mut neighbor = invalid_fld_m64.clone();
        neighbor
            .memory
            .iter_mut()
            .find(|region| region.region_id == "operand")
            .unwrap()
            .initial_bytes_hex = raw.into();
        assert_eq!(
            Fixture::from_json(&serde_json::to_vec(&neighbor).unwrap()).is_ok(),
            true,
            "DD00 invalid-environment predicate must not inspect source value {raw}"
        );
    }
    for combination in 0..64_u16 {
        let sticky = [0x0001, 0x0002, 0x0004, 0x0008, 0x0010, 0x0020]
            .into_iter()
            .enumerate()
            .fold(0_u16, |flags, (bit, status)| {
                flags
                    | if combination & (1 << bit) != 0 {
                        status
                    } else {
                        0
                    }
            });
        let mut sticky_environment = invalid_fld_m64.clone();
        sticky_environment
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(u64::from(sticky));
        assert!(
            sticky_environment.validate().is_err(),
            "admitted sticky status {sticky:04x} must not admit Unsupported"
        );
    }
    for forbidden_status in [0x0040, 0x0080, 0x8000] {
        let mut forbidden_status_fixture = invalid_fld_m64.clone();
        forbidden_status_fixture
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(forbidden_status);
        assert!(
            Fixture::from_json(&serde_json::to_vec(&forbidden_status_fixture).unwrap()).is_ok(),
            "forbidden status {forbidden_status:04x} must admit pre-read Unsupported"
        );
    }
    let mut ze_only_fld_m64 = invalid_fld_m64.clone();
    ze_only_fld_m64
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .status_word = HexU64(0x0004);
    assert!(ze_only_fld_m64.validate().is_err());

    let mut invalid_fld_m80 = fixture.clone();
    let code = invalid_fld_m80
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "db28".into();
    code.comparison_mask_hex = "ffff".into();
    invalid_fld_m80.initial_state.gpr[0] = HexU64(0x0000_0000_0030_0004);
    invalid_fld_m80.memory.push(MemoryRegionSpec {
        region_id: "operand".into(),
        guest_address: HexU64(0x0000_0000_0030_0004),
        length: 16,
        permissions: MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        },
        initial_bytes_hex: "0000000000000000ff7f000000000000".into(),
        comparison_mask_hex: "ff".repeat(16),
    });
    invalid_fld_m80.stop_rip = None;
    invalid_fld_m80.expected_exit = ExpectedExit::Unsupported;
    assert!(Fixture::from_json(&invalid_fld_m80.to_json().unwrap()).is_ok());
    let mut admitted_status_probe = invalid_fld_m80.clone();
    admitted_status_probe
        .memory
        .iter_mut()
        .find(|region| region.region_id == "operand")
        .unwrap()
        .initial_bytes_hex = "0000000000000080ff3f000000000000".into();
    admitted_status_probe
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .tag_word = HexU64(0xdbe4);
    admitted_status_probe
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .control_word = HexU64(0x037f);

    for historical_status in 0..=0x003f_u64 {
        let mut rejected_with_admitted_history = admitted_status_probe.clone();
        rejected_with_admitted_history
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(0x4500 | historical_status);
        assert!(
            rejected_with_admitted_history.validate().is_err(),
            "DB28 must admit historical exception flags {historical_status:04x}"
        );
    }
    for historical_status in (0..=0x003f_u64).filter(|status| status & 1 != 0) {
        let mut rejected_with_sf_ie = admitted_status_probe.clone();
        rejected_with_sf_ie
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(0x4540 | historical_status);
        assert!(
            rejected_with_sf_ie.validate().is_err(),
            "DB28 must admit historical SF only with IE: {historical_status:04x}"
        );
    }
    for rejected_status in [
        0x0040_u64, 0x0240, 0x0080, 0x00bf, 0x8000, 0x803f, 0x00c1, 0x8041, 0x80c1, 0x82c1,
    ] {
        let mut rejected_environment = admitted_status_probe.clone();
        rejected_environment
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(rejected_status);
        assert!(
            Fixture::from_json(&rejected_environment.to_json().unwrap()).is_ok(),
            "DB28 must permit Unsupported for status gate {rejected_status:04x}"
        );
    }

    for raw in [
        "00000000000000800000",
        "01000000000000800000",
        "ffffffffffffffff0000",
        "00000000000000800080",
        "01000000000000800080",
        "ffffffffffffffff0080",
        "0000000000000080ff7f",
        "00000000000000c0ff7f",
        "00000000000000c0ffff",
        "0100000000000080ffff",
        "0100000000000040ff7f",
        "0100000000000040ffff",
        "0100000000000080ff7f",
        "0100000000000080ffff",
        "5555555555555595ff7f",
        "5555555555555595ffff",
        "ffffffffffffffbfff7f",
        "ffffffffffffffbfffff",
    ] {
        let mut not_the_supported_rejection_class = invalid_fld_m80.clone();
        let x87 = not_the_supported_rejection_class
            .initial_state
            .x87
            .as_mut()
            .unwrap();
        x87.control_word = HexU64(0x037f);
        x87.status_word = HexU64(0x4500);
        x87.tag_word = HexU64(0xdbe4);
        not_the_supported_rejection_class
            .memory
            .iter_mut()
            .find(|region| region.region_id == "operand")
            .unwrap()
            .initial_bytes_hex = format!("{raw}000000000000");
        assert!(
            Fixture::from_json(&serde_json::to_vec(&not_the_supported_rejection_class).unwrap())
                .is_err(),
            "unsupported FLD m80 exception predicate must not admit {raw}"
        );
    }

    // The pseudo-infinity binary80 schema gate must safely decline offsets whose doubled
    // byte index fits usize but leaves no room for the twenty-character slice.
    let mut overflowing_schema_offset = invalid_fld_m80.clone();
    let x87 = overflowing_schema_offset
        .initial_state
        .x87
        .as_mut()
        .unwrap();
    x87.control_word = HexU64(0x037f);
    x87.status_word = HexU64(0x4500);
    x87.tag_word = HexU64(0xdbe4);
    overflowing_schema_offset.initial_state.gpr[0] = HexU64(
        overflowing_schema_offset.memory[0]
            .guest_address
            .get()
            .wrapping_add((usize::MAX / 2) as u64),
    );
    assert!(Fixture::from_json(&serde_json::to_vec(&overflowing_schema_offset).unwrap()).is_err());

    let mut fldcw_old_environment_before_canonical_fault = fixture.clone();
    let code = fldcw_old_environment_before_canonical_fault
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d928".into();
    fldcw_old_environment_before_canonical_fault.stop_rip = None;
    fldcw_old_environment_before_canonical_fault.instruction_budget = 1;
    fldcw_old_environment_before_canonical_fault.expected_exit = ExpectedExit::Unsupported;
    fldcw_old_environment_before_canonical_fault
        .initial_state
        .gpr[0] = HexU64(0x0000_7fff_ffff_ffff);
    fldcw_old_environment_before_canonical_fault
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .control_word = HexU64(0xa55a);
    assert!(
        Fixture::from_json(
            &fldcw_old_environment_before_canonical_fault
                .to_json()
                .unwrap()
        )
        .is_ok()
    );
    fldcw_old_environment_before_canonical_fault.expected_exit = ExpectedExit::Exception;
    assert!(
        Fixture::from_json(
            &serde_json::to_vec(&fldcw_old_environment_before_canonical_fault).unwrap()
        )
        .is_err()
    );

    let mut fistp_m64 = fixture.clone();
    let code = fistp_m64
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "df38".into();
    assert!(Fixture::from_json(&fistp_m64.to_json().unwrap()).is_ok());

    let mut fistp_m64_fnstsw = fistp_m64.clone();
    let code = fistp_m64_fnstsw
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 4;
    code.initial_bytes_hex = "df38dfe0".into();
    code.comparison_mask_hex = "ffffffff".into();
    fistp_m64_fnstsw.stop_rip = Some(HexU64(0x1004));
    fistp_m64_fnstsw.instruction_budget = 2;
    assert!(Fixture::from_json(&fistp_m64_fnstsw.to_json().unwrap()).is_ok());

    for (bytes, length, stop, budget) in [("d918", 2, 0x1002, 1), ("d918dfe0", 4, 0x1004, 2)] {
        let mut fstp_m32fp = fixture.clone();
        let code = fstp_m32fp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(length as usize);
        fstp_m32fp.stop_rip = Some(HexU64(stop));
        fstp_m32fp.instruction_budget = budget;
        assert!(
            Fixture::from_json(&fstp_m32fp.to_json().unwrap()).is_ok(),
            "{bytes}"
        );
    }

    for (bytes, length, stop, budget) in [("d910", 2, 0x1002, 1), ("d910dfe0", 4, 0x1004, 2)] {
        let mut fst_m32fp = fixture.clone();
        let code = fst_m32fp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(length as usize);
        fst_m32fp.stop_rip = Some(HexU64(stop));
        fst_m32fp.instruction_budget = budget;
        assert!(
            Fixture::from_json(&fst_m32fp.to_json().unwrap()).is_ok(),
            "{bytes}"
        );
    }

    let mut fnclex = fixture.clone();
    let code = fnclex
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dbe2".into();
    assert!(Fixture::from_json(&fnclex.to_json().unwrap()).is_ok());

    for source in 0u8..=7 {
        let mut fadd = fixture.clone();
        let code = fadd
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("d8c{source:x}");
        assert!(Fixture::from_json(&fadd.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut fmul = fixture.clone();
        let code = fmul
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dcc{:x}", 8 + destination);
        assert!(Fixture::from_json(&fmul.to_json().unwrap()).is_ok());
    }

    for source in 0u8..=7 {
        let mut fmul = fixture.clone();
        let code = fmul
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("d8c{:x}", 8 + source);
        assert!(Fixture::from_json(&fmul.to_json().unwrap()).is_ok());
    }

    for source in 0u8..=7 {
        let mut fdiv = fixture.clone();
        let code = fdiv
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("d8f{source:x}");
        if source == 1 {
            fdiv.stop_rip = None;
            fdiv.expected_exit = ExpectedExit::Unsupported;
        }
        assert!(Fixture::from_json(&fdiv.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut fdivrp = fixture.clone();
        let code = fdivrp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("def{:x}", destination);
        if destination == 1 {
            fdivrp.stop_rip = None;
            fdivrp.expected_exit = ExpectedExit::Unsupported;
            fdivrp.instruction_budget = 1;
        }
        assert!(Fixture::from_json(&fdivrp.to_json().unwrap()).is_ok());
    }

    let mut fdivrp_inexact = fixture.clone();
    let code = fdivrp_inexact
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "def1".into();
    fdivrp_inexact.stop_rip = None;
    fdivrp_inexact.expected_exit = ExpectedExit::Unsupported;
    fdivrp_inexact.instruction_budget = 1;
    assert!(Fixture::from_json(&fdivrp_inexact.to_json().unwrap()).is_ok());

    for source in 0u8..=7 {
        let mut fsub = fixture.clone();
        let code = fsub
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("d8e{source:x}");
        if source == 0 {
            fsub.stop_rip = None;
            fsub.expected_exit = ExpectedExit::Unsupported;
        }
        assert!(Fixture::from_json(&fsub.to_json().unwrap()).is_ok());
    }

    for source in 0u8..=7 {
        let mut fsubr = fixture.clone();
        let code = fsubr
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("d8e{:x}", 8 + source);
        if source == 0 {
            fsubr.stop_rip = None;
            fsubr.expected_exit = ExpectedExit::Unsupported;
            fsubr.instruction_budget = 1;
        }
        assert!(Fixture::from_json(&fsubr.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut fadd = fixture.clone();
        let code = fadd
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dcc{destination:x}");
        assert!(Fixture::from_json(&fadd.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut faddp = fixture.clone();
        let code = faddp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dec{destination:x}");
        assert!(Fixture::from_json(&faddp.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut fsub = fixture.clone();
        let code = fsub
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dce{:x}", 8 + destination);
        if destination == 0 {
            fsub.stop_rip = None;
            fsub.expected_exit = ExpectedExit::Unsupported;
        }
        assert!(Fixture::from_json(&fsub.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut fsubr = fixture.clone();
        let code = fsubr
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dce{:x}", destination);
        if destination == 0 {
            fsubr.stop_rip = None;
            fsubr.expected_exit = ExpectedExit::Unsupported;
        }
        assert_eq!(fsubr.instruction_budget, 1);
        assert!(
            Fixture::from_json(&fsubr.to_json().unwrap()).is_ok(),
            "DC E{destination:x}"
        );
    }

    for destination in 0u8..=7 {
        let mut fsubp = fixture.clone();
        let code = fsubp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dee{:x}", 8 + destination);
        if destination == 0 {
            fsubp.stop_rip = None;
            fsubp.expected_exit = ExpectedExit::Unsupported;
        }
        assert!(Fixture::from_json(&fsubp.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut fsubrp = fixture.clone();
        let code = fsubrp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("dee{destination:x}");
        if destination == 0 {
            fsubrp.stop_rip = None;
            fsubrp.expected_exit = ExpectedExit::Unsupported;
        }
        assert!(Fixture::from_json(&fsubrp.to_json().unwrap()).is_ok());
    }

    for destination in 0u8..=7 {
        let mut fst = fixture.clone();
        let code = fst
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("ddd{:x}", destination);
        assert!(Fixture::from_json(&fst.to_json().unwrap()).is_ok());

        let mut fstp = fixture.clone();
        let code = fstp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("ddd{:x}", 8 + destination);
        assert!(Fixture::from_json(&fstp.to_json().unwrap()).is_ok());
    }

    let mut fst_bad_source = fixture.clone();
    fst_bad_source
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap()
        .initial_bytes_hex = "ddd1".into();
    fst_bad_source.stop_rip = None;
    fst_bad_source.expected_exit = ExpectedExit::Unsupported;
    fst_bad_source.instruction_budget = 1;
    assert!(Fixture::from_json(&fst_bad_source.to_json().unwrap()).is_ok());

    let mut fstp_bad_source = fixture.clone();
    fstp_bad_source
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap()
        .initial_bytes_hex = "ddd9".into();
    fstp_bad_source.stop_rip = None;
    fstp_bad_source.expected_exit = ExpectedExit::Unsupported;
    assert!(Fixture::from_json(&fstp_bad_source.to_json().unwrap()).is_ok());

    for source in 0u8..=7 {
        let mut fcomp = fixture.clone();
        fcomp.case_id = format!("local_fcomp_st{source}");
        fcomp.instruction_budget = 1;
        fcomp.stop_rip = Some(HexU64(0x1002));
        fcomp.expected_exit = ExpectedExit::StopAddress;
        let fcomp_code = fcomp
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        fcomp_code.length = 2;
        fcomp_code.initial_bytes_hex = format!("d8{:02x}", 0xd8 | source);
        fcomp_code.comparison_mask_hex = "ffff".into();
        assert!(Fixture::from_json(&fcomp.to_json().unwrap()).is_ok());

        let mut fcomp_observation = fcomp.clone();
        fcomp_observation.case_id = format!("local_fcomp_st{source}_fnstsw");
        fcomp_observation
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .initial_bytes_hex = format!("d8{:02x}dfe0", 0xd8 | source);
        fcomp_observation.instruction_budget = 2;
        fcomp_observation.stop_rip = Some(HexU64(0x1004));
        fcomp_observation
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .length = 4;
        fcomp_observation
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap()
            .comparison_mask_hex = "ffffffff".into();
        assert!(Fixture::from_json(&fcomp_observation.to_json().unwrap()).is_ok());

        let mut fcomp_rejected = fcomp;
        fcomp_rejected.case_id = format!("local_fcomp_st{source}_rejected");
        fcomp_rejected.stop_rip = None;
        fcomp_rejected.expected_exit = ExpectedExit::Unsupported;
        assert!(Fixture::from_json(&fcomp_rejected.to_json().unwrap()).is_ok());
    }

    for (bytes, length, stop) in [
        ("db", 1, 0x1001),
        ("9bdbe3", 3, 0x1003),
        ("66dbe3", 3, 0x1003),
        ("dbe0", 2, 0x1002),
        ("dbe4", 2, 0x1002),
        ("dfe1", 2, 0x1002),
        ("9bdfe0", 3, 0x1003),
        ("66dfe0", 3, 0x1003),
        ("9bdbe2", 3, 0x1003),
        ("66dbe2", 3, 0x1003),
        ("f3dbe2", 3, 0x1003),
        ("d8e8", 2, 0x1002),
        ("d8b1", 2, 0x1002),
        ("d800", 2, 0x1002),
        ("66d8c1", 3, 0x1003),
        ("48d8c1", 3, 0x1003),
        ("9bd8c1", 3, 0x1003),
        ("66d8e1", 3, 0x1003),
        ("9bd8e1", 3, 0x1003),
        ("d8", 1, 0x1001),
        ("de00", 2, 0x1002),
        ("66dec1", 3, 0x1003),
        ("9bdec1", 3, 0x1003),
        ("66ddd1", 3, 0x1003),
        ("48ddd1", 3, 0x1003),
        ("9bddd1", 3, 0x1003),
        ("66ddd9", 3, 0x1003),
        ("9bddd9", 3, 0x1003),
        ("dd", 1, 0x1001),
    ] {
        let mut unsupported = fixture.clone();
        let code = unsupported
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.length = length;
        code.initial_bytes_hex = bytes.into();
        code.comparison_mask_hex = "ff".repeat(length as usize);
        unsupported.stop_rip = Some(HexU64(stop));
        assert!(unsupported.validate().is_err(), "{bytes}");
    }
}

#[test]
fn x87_state_profile_admits_only_exact_fabs_zero_composition() {
    let mut fixture = x87_fixture().fixture;
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 8;
    code.initial_bytes_hex = "d900d9e1d910dfe0".into();
    code.comparison_mask_hex = "ffffffffffffffff".into();
    fixture.instruction_budget = 4;
    fixture.stop_rip = Some(HexU64(0x1008));
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());

    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d900d9e2d910dfe0".into();
    assert!(fixture.validate().is_err());
}

#[test]
fn x87_state_profile_admits_exact_ftst_zero_composition_only() {
    let mut fixture = x87_fixture().fixture;
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 6;
    code.initial_bytes_hex = "d900d9e4dfe0".into();
    code.comparison_mask_hex = "ffffffffffff".into();
    fixture.instruction_budget = 3;
    fixture.stop_rip = Some(HexU64(0x1006));
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());

    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d900d9e5dfe0".into();
    assert!(fixture.validate().is_err());
}

#[test]
fn x87_state_profile_admits_both_ftst_canonical_infinity_fixtures_without_schema_changes() {
    for raw in [
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30at_fld_ftst_positive_infinity_fnstsw.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30at_fld_ftst_negative_infinity_fnstsw.case.json"
        )
        .as_slice(),
    ] {
        let fixture = Fixture::from_json(raw).unwrap();
        assert_eq!(fixture.fixture.profile, Profile::LongX87StateV1);
        assert_eq!(fixture.fixture.instruction_budget, 3);
        assert_eq!(fixture.fixture.stop_rip, Some(HexU64(0x0010_0006)));
        assert_eq!(
            fixture
                .fixture
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .unwrap()
                .initial_bytes_hex,
            "d900d9e4dfe0"
        );
    }
}

#[test]
fn x87_state_profile_admits_exact_fst_sti_zero_composition_only() {
    let mut fixture = x87_fixture().fixture;
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 6;
    code.initial_bytes_hex = "d900ddd1dfe0".into();
    code.comparison_mask_hex = "ffffffffffff".into();
    fixture.instruction_budget = 3;
    fixture.stop_rip = Some(HexU64(0x1006));
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());

    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d900ddd2dfe0".into();
    assert!(fixture.validate().is_err());
}

#[test]
fn x87_state_profile_admits_exact_fstp_sti_zero_composition_only() {
    let mut fixture = x87_fixture().fixture;
    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.length = 6;
    code.initial_bytes_hex = "d900ddd9dfe0".into();
    code.comparison_mask_hex = "ffffffffffff".into();
    fixture.instruction_budget = 3;
    fixture.stop_rip = Some(HexU64(0x1006));
    assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());

    let code = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "d900ddd8dfe0".into();
    assert!(fixture.validate().is_err());
}

#[test]
fn x87_fcompp_schema_admits_exact_codes_and_actual_bad_rhs_rejection() {
    let mut success = x87_fixture().fixture;
    let code = success
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "ded9".into();
    code.length = 2;
    code.comparison_mask_hex = "ffff".into();
    success.instruction_budget = 1;
    success.stop_rip = Some(HexU64(success.initial_state.rip.get() + 2));
    assert!(Fixture::from_json(&success.to_json().unwrap()).is_ok());

    let mut observation = success.clone();
    let code = observation
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "ded9dfe0".into();
    code.length = 4;
    code.comparison_mask_hex = "ffffffff".into();
    observation.instruction_budget = 2;
    observation.stop_rip = Some(HexU64(observation.initial_state.rip.get() + 4));
    assert!(Fixture::from_json(&observation.to_json().unwrap()).is_ok());

    let mut rejected = success.clone();
    rejected.stop_rip = None;
    rejected.expected_exit = ExpectedExit::Unsupported;
    let x87 = rejected.initial_state.x87.as_mut().unwrap();
    let top = ((x87.status_word.get() >> 11) & 7) as u8;
    let rhs = (top + 1) & 7;
    x87.tag_word = HexU64(x87.tag_word.get() | (3 << (u32::from(rhs) * 2)));
    assert!(Fixture::from_json(&rejected.to_json().unwrap()).is_ok());

    let mut false_rejection = rejected.clone();
    false_rejection.initial_state.x87.as_mut().unwrap().tag_word = HexU64(0);
    assert!(false_rejection.validate().is_err());
    let mut wrong_stop = rejected.clone();
    wrong_stop.stop_rip = Some(HexU64(wrong_stop.initial_state.rip.get() + 2));
    assert!(wrong_stop.validate().is_err());
    let mut wrong_budget = rejected.clone();
    wrong_budget.instruction_budget = 2;
    assert!(wrong_budget.validate().is_err());
    for bytes in ["66ded9", "9bded9", "ded8", "deda"] {
        let mut unsupported = success.clone();
        let code = unsupported
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = bytes.into();
        code.length = (bytes.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        assert!(unsupported.validate().is_err(), "{bytes}");
    }
}

#[test]
fn x87_fucompp_schema_admits_exact_codes_and_actual_bad_rhs_rejection() {
    let mut success = x87_fixture().fixture;
    let code = success
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dae9".into();
    code.length = 2;
    code.comparison_mask_hex = "ffff".into();
    success.instruction_budget = 1;
    success.stop_rip = Some(HexU64(success.initial_state.rip.get() + 2));
    assert!(Fixture::from_json(&success.to_json().unwrap()).is_ok());

    let mut observation = success.clone();
    let code = observation
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dae9dfe0".into();
    code.length = 4;
    code.comparison_mask_hex = "ffffffff".into();
    observation.instruction_budget = 2;
    observation.stop_rip = Some(HexU64(observation.initial_state.rip.get() + 4));
    assert!(Fixture::from_json(&observation.to_json().unwrap()).is_ok());

    let mut rejected = success.clone();
    rejected.stop_rip = None;
    rejected.expected_exit = ExpectedExit::Unsupported;
    let x87 = rejected.initial_state.x87.as_mut().unwrap();
    let top = ((x87.status_word.get() >> 11) & 7) as u8;
    let rhs = (top + 1) & 7;
    x87.tag_word = HexU64(x87.tag_word.get() | (3 << (u32::from(rhs) * 2)));
    assert!(Fixture::from_json(&rejected.to_json().unwrap()).is_ok());

    let mut false_rejection = rejected.clone();
    false_rejection.initial_state.x87.as_mut().unwrap().tag_word = HexU64(0);
    assert!(false_rejection.validate().is_err());
    let mut wrong_stop = rejected.clone();
    wrong_stop.stop_rip = Some(HexU64(wrong_stop.initial_state.rip.get() + 2));
    assert!(wrong_stop.validate().is_err());
    let mut wrong_budget = rejected.clone();
    wrong_budget.instruction_budget = 2;
    assert!(wrong_budget.validate().is_err());
    for bytes in ["66dae9", "9bdae9", "dae8", "daea", "da00"] {
        let mut unsupported = success.clone();
        let code = unsupported
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = bytes.into();
        code.length = (bytes.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        assert!(unsupported.validate().is_err(), "{bytes}");
    }
}

#[test]
fn x87_fucom_schema_admits_exact_codes_and_actual_bad_rhs_rejection() {
    let sequence = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ag_fucom_equal_fnstsw.case.json"
    ))
    .unwrap();
    let sequence_code = sequence
        .fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .unwrap();
    assert_eq!(sequence_code.initial_bytes_hex, "dde1dfe0");
    assert_eq!(sequence_code.length, 4);
    assert_eq!(sequence.fixture.instruction_budget, 2);
    assert_eq!(
        sequence.fixture.stop_rip.map(HexU64::get),
        Some(sequence.fixture.initial_state.rip.get() + 4)
    );

    let mut bare_success = sequence.fixture.clone();
    bare_success.case_id = "s2_30ag_fucom_equal_bare_schema".into();
    bare_success.instruction_budget = 1;
    bare_success.stop_rip = Some(HexU64(bare_success.initial_state.rip.get() + 2));
    let code = bare_success
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dde1".into();
    code.length = 2;
    code.comparison_mask_hex = "ffff".into();
    assert!(Fixture::from_json(&bare_success.to_json().unwrap()).is_ok());

    let mut false_rejection = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ag_fucom_invalid_rhs_tag.case.json"
    ))
    .unwrap()
    .fixture;
    let rejected_code = false_rejection
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .unwrap();
    assert_eq!(rejected_code.initial_bytes_hex, "dde1");
    assert_eq!(false_rejection.instruction_budget, 1);
    assert_eq!(false_rejection.stop_rip, None);
    false_rejection.initial_state.x87.as_mut().unwrap().tag_word = HexU64(0);
    assert!(false_rejection.validate().is_err());

    for bytes in ["66dde1", "9bdde1", "dde0", "dde2"] {
        let mut unsupported = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ag_fucom_equal_fnstsw.case.json"
        ))
        .unwrap()
        .fixture;
        let code = unsupported
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = bytes.into();
        code.length = (bytes.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        assert!(unsupported.validate().is_err(), "{bytes}");
    }
}

#[test]
fn x87_fucomp_schema_admits_exact_codes_and_actual_bad_rhs_rejection() {
    let sequence = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ah_fucomp_equal_fnstsw.case.json"
    ))
    .unwrap();
    let sequence_code = sequence
        .fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .unwrap();
    assert_eq!(sequence_code.initial_bytes_hex, "dde9dfe0");
    assert_eq!(sequence_code.length, 4);
    assert_eq!(sequence.fixture.instruction_budget, 2);
    assert_eq!(
        sequence.fixture.stop_rip.map(HexU64::get),
        Some(sequence.fixture.initial_state.rip.get() + 4)
    );

    let mut bare_success = sequence.fixture.clone();
    bare_success.case_id = "s2_30ah_fucomp_equal_bare_schema".into();
    bare_success.instruction_budget = 1;
    bare_success.stop_rip = Some(HexU64(bare_success.initial_state.rip.get() + 2));
    let code = bare_success
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dde9".into();
    code.length = 2;
    code.comparison_mask_hex = "ffff".into();
    assert!(Fixture::from_json(&bare_success.to_json().unwrap()).is_ok());

    let rejected = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_30ah_fucomp_invalid_rhs_tag.case.json"
    ))
    .unwrap()
    .fixture;
    let rejected_code = rejected
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .unwrap();
    assert_eq!(rejected_code.initial_bytes_hex, "dde9");
    assert_eq!(rejected.instruction_budget, 1);
    assert_eq!(rejected.stop_rip, None);
    let mut false_rejection = rejected.clone();
    false_rejection.initial_state.x87.as_mut().unwrap().tag_word = HexU64(0);
    assert!(false_rejection.validate().is_err());

    for bytes in [
        "66dde9", "9bdde9", "dde8", "ddea", "ddeb", "ddec", "dded", "ddee", "ddef",
    ] {
        let mut unsupported = sequence.fixture.clone();
        unsupported.case_id = format!("s2_30ah_fucomp_reject_{bytes}");
        unsupported.instruction_budget = 1;
        unsupported.stop_rip = Some(HexU64(unsupported.initial_state.rip.get() + 2));
        let code = unsupported
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = bytes.into();
        code.length = (bytes.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        assert!(unsupported.validate().is_err(), "{bytes}");
    }
}

#[test]
fn x87_fcomi_family_schema_admits_exact_64_programs_and_narrow_actual_source_rejects() {
    let base = x87_fixture().fixture;
    for opcode in [0xdb, 0xdf] {
        for modrm in (0xe8u8..=0xef).chain(0xf0..=0xf7) {
            let bytes = format!("{opcode:02x}{modrm:02x}");
            for program in [bytes.clone(), format!("{bytes}dfe0")] {
                let mut fixture = base.clone();
                fixture.case_id = format!("x87_fcomi_{program}");
                fixture.instruction_budget = if program.len() == 8 { 2 } else { 1 };
                let length = (program.len() / 2) as u32;
                fixture.stop_rip =
                    Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
                let code = fixture
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.initial_bytes_hex = program.clone();
                code.length = length;
                code.comparison_mask_hex = "ff".repeat(length as usize);
                assert!(
                    Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
                    "{program}"
                );
            }
        }
    }

    for (bytes, source) in [("dbf0", 0u8), ("dfef", 7u8)] {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcomi_reject_{bytes}");
        fixture.stop_rip = None;
        fixture.instruction_budget = 1;
        fixture.expected_exit = ExpectedExit::Unsupported;
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        let top = 2u8;
        x87.control_word = HexU64(0x037f);
        x87.status_word = HexU64(u64::from(top) << 11);
        x87.tag_word = HexU64(u16::MAX.into());
        let set_valid = |tag_word: &mut HexU64, physical: u8| {
            tag_word.0 &= !(3u64 << (u32::from(physical) * 2));
        };
        set_valid(&mut x87.tag_word, top);
        set_valid(&mut x87.tag_word, (top + 1) & 7);
        x87.registers[usize::from(top)] = if source == 0 {
            "00000000000000000000".into()
        } else {
            "0000000000000080ff3f".into()
        };
        x87.registers[usize::from((top + 1) & 7)] = "0000000000000080ff3f".into();
        if source != 0 {
            let actual = (top + source) & 7;
            x87.tag_word.0 |= 3u64 << (u32::from(actual) * 2);
        }
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = bytes.into();
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(
            Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
            "{bytes}"
        );

        for near_miss in [
            format!("{bytes}dfe0"),
            format!("66{bytes}"),
            format!("9b{bytes}"),
        ] {
            let mut invalid = fixture.clone();
            let code = invalid
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.initial_bytes_hex = near_miss.clone();
            code.length = (near_miss.len() / 2) as u32;
            code.comparison_mask_hex = "ff".repeat(code.length as usize);
            assert!(invalid.validate().is_err(), "{near_miss}");
        }
    }
}

#[test]
fn x87_fcmovb_schema_admits_only_indexed_exact_streams_and_operand_rejects() {
    let base = x87_fixture().fixture;
    for (program, budget) in [
        ("dac0", 1),
        ("dac1", 1),
        ("dac2", 1),
        ("dac3", 1),
        ("dac4", 1),
        ("dac5", 1),
        ("dac6", 1),
        ("dac7", 1),
        ("dac0dfe0", 2),
        ("dac1dfe0", 2),
        ("dac2dfe0", 2),
        ("dac3dfe0", 2),
        ("dac4dfe0", 2),
        ("dac5dfe0", 2),
        ("dac6dfe0", 2),
        ("dac7dfe0", 2),
        ("dac1dac1dfe0", 3),
        ("d9e8d9e0dbf1dac1dfe0", 6),
        ("d9e8d9e0d9e8dbf1dac1dfe0", 6),
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovb_{program}");
        fixture.instruction_budget = budget;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().expect(program)).is_ok(),
            "{program}"
        );
    }
    for program in [
        "66dac1", "40dac1", "dae9dfe0", "dae8", "dac1dfe1", "dac0dfe1",
    ] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + (program.len() / 2) as u64,
        ));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = (program.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(code.length as usize);
        assert!(fixture.validate().is_err(), "{program}");
    }
    for (tag_word, raw) in [
        (u16::MAX, "0000000000000080ff3f"),
        (0, "00000000000000000000"),
    ] {
        let mut fixture = base.clone();
        fixture.case_id = "x87_fcmovb_bad_operand_cf0".into();
        fixture.stop_rip = None;
        fixture.instruction_budget = 1;
        fixture.expected_exit = ExpectedExit::Unsupported;
        fixture.initial_state.rflags = HexU64(2);
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(0x037f);
        x87.status_word = HexU64(0);
        x87.tag_word = HexU64(u64::from(tag_word));
        x87.registers[0] = raw.into();
        x87.registers[1] = "0000000000000080ff3f".into();
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = "dac1".into();
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());
    }
}

#[test]
fn x87_fcmovnu_schema_admits_exactly_eighteen_streams_and_rejects_bad_inputs() {
    let base = x87_fixture().fixture;
    let programs: Vec<(String, u64)> = (0xd8u8..=0xdf)
        .flat_map(|modrm| {
            let bare = format!("db{modrm:02x}");
            [(bare.clone(), 1), (format!("{bare}dfe0"), 2)]
        })
        .chain([
            ("dbd9dbd9dfe0".into(), 3),
            ("d9e8d9ee4885c0dbd9dfe0".into(), 5),
        ])
        .collect();
    assert_eq!(programs.len(), 18);
    for (program, budget) in &programs {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovnu_{program}");
        fixture.instruction_budget = *budget;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().expect(program)).is_ok(),
            "{program}"
        );
    }
    for program in [
        "66dbd9",
        "40dbd9",
        "dbd9dfe1",
        "dbd9dbd8dfe0",
        "dbd8dfe0dfe0",
        "d9e8d9ee4885c0dbd8dfe0",
    ] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + (program.len() / 2) as u64,
        ));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = (program.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(code.length as usize);
        assert!(fixture.validate().is_err(), "{program}");
    }

    let mut indexed_bad_source = base.clone();
    indexed_bad_source.expected_exit = ExpectedExit::Unsupported;
    indexed_bad_source.stop_rip = None;
    indexed_bad_source.instruction_budget = 1;
    let code = indexed_bad_source
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dbda".into();
    code.length = 2;
    code.comparison_mask_hex = "ffff".into();
    {
        let x87 = indexed_bad_source.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(0x037f);
        x87.status_word = HexU64(0x3800);
        x87.registers.fill("0000000000000080ff3f".into());
        x87.tag_word = HexU64(0);
        let top = 7usize;
        let selected = (top + 2) & 7;
        x87.tag_word.0 &= !(3 << (selected * 2));
        x87.tag_word.0 |= 2 << (selected * 2);
    }
    assert!(
        indexed_bad_source.validate().is_ok(),
        "invalid selected ST(2)"
    );

    let mut unrelated_bad_self = indexed_bad_source.clone();
    let code = unrelated_bad_self
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dbd8".into();
    {
        let x87 = unrelated_bad_self.initial_state.x87.as_mut().unwrap();
        x87.tag_word = HexU64(0);
        let top = 7usize;
        x87.tag_word.0 &= !(3 << (top * 2));
        x87.tag_word.0 |= 0 << (top * 2);
        x87.tag_word.0 &= !3;
        x87.tag_word.0 |= 2;
    }
    assert!(
        unrelated_bad_self.validate().is_err(),
        "self-index must not inspect an unrelated invalid cell"
    );

    let mut invalid_self_top = unrelated_bad_self.clone();
    let x87 = invalid_self_top.initial_state.x87.as_mut().unwrap();
    x87.tag_word.0 &= !(3 << 14);
    x87.tag_word.0 |= 2 << 14;
    assert!(invalid_self_top.validate().is_ok(), "invalid self TOP");
}

#[test]
fn x87_fcmovnb_schema_admits_only_nineteen_exact_streams_and_reject_profiles() {
    let base = x87_fixture().fixture;
    let mut programs: Vec<(String, u64)> = (0xc0u8..=0xc7)
        .flat_map(|modrm| {
            let instruction = format!("db{modrm:02x}");
            [(instruction.clone(), 1), (format!("{instruction}dfe0"), 2)]
        })
        .collect();
    programs.extend(
        [
            ("dbc1dbc1dfe0", 3),
            ("d9e8d9e8d9e0dbf1dbc1dfe0", 6),
            ("d9e8d9e0d9e8dbf1dbc1dfe0", 6),
        ]
        .map(|(program, budget)| (program.to_owned(), budget)),
    );
    assert_eq!(programs.len(), 19);
    for (program, budget) in programs {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovnb_{program}");
        fixture.instruction_budget = budget;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.clone();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().expect(&program)).is_ok(),
            "{program}"
        );
    }
    for program in ["66dbc1", "48dbc1", "9bdbc1", "dbe0", "dbc1dfe1"] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(fixture.validate().is_err(), "{program}");
    }
    for (fcw, fsw) in [
        (0x077f, 0),
        (0x037f, 0x0200),
        (0x037f, 0x8000),
        (0x037f, 0x0080),
    ] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        fixture.stop_rip = None;
        fixture.expected_exit = ExpectedExit::Unsupported;
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(fcw);
        x87.status_word = HexU64(fsw);
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = "dbc1".into();
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());
    }
    for modrm in 0xc0u8..=0xc7 {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        fixture.stop_rip = None;
        fixture.expected_exit = ExpectedExit::Unsupported;
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("db{modrm:02x}");
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(
            Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
            "i={}",
            modrm & 7
        );
        let mut missing = fixture.clone();
        missing.initial_state.x87 = None;
        assert!(missing.validate().is_err(), "missing x87 i={}", modrm & 7);

        let mut rejected = base.clone();
        rejected.instruction_budget = 1;
        rejected.stop_rip = None;
        rejected.expected_exit = ExpectedExit::Unsupported;
        let x87 = rejected.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(0x037f);
        x87.status_word = HexU64(0);
        let top = ((x87.status_word.get() >> 11) & 7) as usize;
        let source = (top + usize::from(modrm & 7)) & 7;
        let mut tag_word = x87.tag_word.get();
        x87.registers[top] = "0000000000000080ff3f".into();
        tag_word &= !(3 << (top * 2));
        let invalid_slot = if modrm == 0xc0 { (top + 1) & 7 } else { source };
        tag_word = (tag_word & !(3 << (invalid_slot * 2))) | (3 << (invalid_slot * 2));
        x87.tag_word = HexU64(tag_word);
        let code = rejected
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("db{modrm:02x}");
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        if modrm == 0xc0 {
            assert!(
                rejected.validate().is_err(),
                "self-index inspected unrelated tag"
            );
        } else {
            assert!(
                Fixture::from_json(&rejected.to_json().unwrap()).is_ok(),
                "i={}",
                modrm & 7
            );
        }
    }
}

#[test]
fn x87_fcmove_schema_admits_only_nineteen_exact_streams_and_reject_profiles() {
    let base = x87_fixture().fixture;
    let mut programs = Vec::new();
    for source in 0..8u8 {
        let opcode = format!("da{:02x}", 0xc8 | source);
        programs.push((opcode.clone(), 1));
        programs.push((format!("{opcode}dfe0"), 2));
    }
    programs.extend(
        [
            ("dac9dac9dfe0", 3),
            ("d9e8d9e8dbf1d9e0dac9dfe0", 6),
            ("d9e8d9e8d9e0dbf1dac9dfe0", 6),
        ]
        .map(|(program, budget)| (program.to_owned(), budget)),
    );
    assert_eq!(programs.len(), 19);
    for (program, budget) in programs {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmove_{program}");
        fixture.instruction_budget = budget;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.clone();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
            "{program}"
        );
    }
    for program in ["66dac9", "48dac9", "9bdac9", "daea", "da", "dac9dfe1"] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(fixture.validate().is_err(), "{program}");
    }
    for source in 0..8u8 {
        let modrm = 0xc8 | source;
        for program in [
            format!("66da{modrm:02x}"),
            format!("48da{modrm:02x}"),
            format!("9bda{modrm:02x}"),
        ] {
            let mut fixture = base.clone();
            fixture.instruction_budget = 1;
            fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + 3));
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.initial_bytes_hex = program.clone();
            code.length = 3;
            code.comparison_mask_hex = "ffffff".into();
            assert!(fixture.validate().is_err(), "{program}");
        }
    }
    for (fcw, fsw) in [
        (0x077f, 0),
        (0x037f, 0x0200),
        (0x037f, 0x8000),
        (0x037f, 0x0080),
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmove_bad_env_{fcw:04x}_{fsw:04x}");
        fixture.instruction_budget = 1;
        fixture.stop_rip = None;
        fixture.expected_exit = ExpectedExit::Unsupported;
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(fcw);
        x87.status_word = HexU64(fsw);
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = "dac9".into();
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());
    }
}

#[test]
fn x87_fcmovne_schema_admits_exact_bare_c9_streams_only() {
    let base = x87_fixture().fixture;
    let mut programs = Vec::new();
    for source in 0..8u8 {
        let instruction = format!("db{:02x}", 0xc8 | source);
        programs.push((instruction.clone(), 1));
        programs.push((format!("{instruction}dfe0"), 2));
    }
    programs.extend(
        [
            ("dbc9dbc9dfe0", 3),
            ("d9e8d9e8dbf1d9e0dbc9dfe0", 6),
            ("d9e8d9e8d9e0dbf1dbc9dfe0", 6),
        ]
        .map(|(program, budget)| (program.to_owned(), budget)),
    );
    assert_eq!(programs.len(), 19);
    for (program, budget) in programs {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovne_{program}");
        fixture.instruction_budget = budget;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.clone();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
            "{program}"
        );
    }
    for program in ["dbe0", "db", "dbc9dfe1"] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(fixture.validate().is_err(), "{program}");
    }
    for source in 0..8u8 {
        let modrm = 0xc8 | source;
        for program in [
            format!("66db{modrm:02x}"),
            format!("48db{modrm:02x}"),
            format!("9bdb{modrm:02x}"),
        ] {
            let mut fixture = base.clone();
            fixture.instruction_budget = 1;
            fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + 3));
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.initial_bytes_hex = program.clone();
            code.length = 3;
            code.comparison_mask_hex = "ffffff".into();
            assert!(fixture.validate().is_err(), "{program}");
        }
    }
}

#[test]
fn x87_fcmovbe_schema_admits_exact_indexed_streams_and_reject_profiles() {
    let base = x87_fixture().fixture;
    let programs = (0..8u8)
        .flat_map(|index| {
            let move_bytes = format!("da{:02x}", 0xd0 | index);
            [(move_bytes.clone(), 1), (format!("{move_bytes}dfe0"), 2)]
        })
        .chain([
            ("dad1dad1dfe0".into(), 3),
            ("d9e8d9e0d9e8dbf1dad1dfe0".into(), 6),
            ("d9e8d9e8dbf1d9e0dad1dfe0".into(), 6),
            ("d9e8d9e8d9e0dbf1dad1dfe0".into(), 6),
        ])
        .collect::<Vec<_>>();
    for (program, budget) in programs {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovbe_{program}");
        fixture.instruction_budget = budget;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.to_owned();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
            "{program}"
        );
    }
    for program in ["daea", "daeb", "dad1dfe1", "66dad1", "48dad1", "9bdad1"] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.to_owned();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(fixture.validate().is_err(), "{program}");
    }

    let mut invalid = base.clone();
    invalid.case_id = "x87_fcmovbe_invalid_participant".into();
    invalid.expected_exit = ExpectedExit::Unsupported;
    invalid.stop_rip = None;
    invalid.instruction_budget = 1;
    let invalid_code = invalid
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    invalid_code.initial_bytes_hex = "dad1".into();
    invalid_code.length = 2;
    invalid_code.comparison_mask_hex = "ffff".into();
    invalid.initial_state.x87.as_mut().unwrap().tag_word = HexU64(0xffff);
    assert!(Fixture::from_json(&invalid.to_json().unwrap()).is_ok());

    for (fcw, fsw) in [
        (0x077f, 0),
        (0x037f, 0x0200),
        (0x037f, 0x8000),
        (0x037f, 0x0080),
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovbe_bad_env_{fcw:04x}_{fsw:04x}");
        fixture.expected_exit = ExpectedExit::Unsupported;
        fixture.stop_rip = None;
        fixture.instruction_budget = 1;
        let x87 = fixture.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(fcw);
        x87.status_word = HexU64(fsw);
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = "dad1".into();
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());
    }
}

#[test]
fn x87_fcmovnbe_schema_admits_exact_indexed_streams_and_reject_profiles() {
    let base = x87_fixture().fixture;
    let mut programs = Vec::new();
    for index in 0..8u8 {
        let instruction = format!("db{:02x}", 0xd0 | index);
        programs.push((instruction.clone(), 1));
        programs.push((format!("{instruction}dfe0"), 2));
    }
    programs.extend(
        [
            ("dbd1dbd1dfe0", 3),
            ("d9e8d9e0d9e8dbf1dbd1dfe0", 6),
            ("d9e8d9e8dbf1d9e0dbd1dfe0", 6),
            ("d9e8d9e8d9e0dbf1dbd1dfe0", 6),
        ]
        .map(|(program, budget)| (program.to_owned(), budget)),
    );
    assert_eq!(programs.len(), 20);
    for (program, budget) in programs {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovnbe_{program}");
        fixture.instruction_budget = budget;
        let length = (program.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.clone();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().expect(&program)).is_ok(),
            "{program}"
        );
    }

    for program in [
        "dbd1dfe1",
        "66dbd1",
        "40dbd1",
        "9bdbd1",
        "dbd1dbd1",
        "d9e8dbf1dbd1dfe0",
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovnbe_reject_{program}");
        fixture.instruction_budget = 1;
        fixture.stop_rip = Some(HexU64(
            fixture.initial_state.rip.get() + (program.len() / 2) as u64,
        ));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = (program.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(code.length as usize);
        assert!(fixture.validate().is_err(), "{program}");
    }
    for modrm in [0xe0, 0xe7] {
        let mut fixture = base.clone();
        fixture.instruction_budget = 1;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + 2));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("db{modrm:02x}");
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(fixture.validate().is_err(), "DB {modrm:02x}");
    }
    for source in 0..8u8 {
        for prefix in ["66", "40", "9b"] {
            let mut fixture = base.clone();
            fixture.instruction_budget = 1;
            fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + 3));
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.initial_bytes_hex = format!("{prefix}db{:02x}", 0xd0 | source);
            code.length = 3;
            code.comparison_mask_hex = "ffffff".into();
            assert!(fixture.validate().is_err(), "{prefix} DB D{source:x}");
        }
    }

    let mut invalid = base.clone();
    invalid.case_id = "x87_fcmovnbe_invalid_untaken_source".into();
    invalid.stop_rip = None;
    invalid.instruction_budget = 1;
    invalid.expected_exit = ExpectedExit::Unsupported;
    invalid.initial_state.rflags = HexU64(2 | 0x41);
    let x87 = invalid.initial_state.x87.as_mut().unwrap();
    x87.control_word = HexU64(0x037f);
    x87.status_word = HexU64(0);
    x87.tag_word = HexU64(u16::MAX.into());
    x87.registers[0] = "0000000000000080ff3f".into();
    x87.registers[1] = "00000000000000000000".into();
    let code = invalid
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dbd1".into();
    code.length = 2;
    code.comparison_mask_hex = "ffff".into();
    assert!(Fixture::from_json(&invalid.to_json().unwrap()).is_ok());

    for source in 0..8u8 {
        for taken in [false, true] {
            let mut invalid = base.clone();
            invalid.case_id = format!("x87_fcmovnbe_invalid_source_{source}_{taken}");
            invalid.stop_rip = None;
            invalid.instruction_budget = 1;
            invalid.expected_exit = ExpectedExit::Unsupported;
            invalid.initial_state.rflags = HexU64(if taken { 2 } else { 2 | 0x41 });
            let x87 = invalid.initial_state.x87.as_mut().unwrap();
            x87.control_word = HexU64(0x037f);
            x87.status_word = HexU64(0);
            x87.tag_word = HexU64(u16::MAX.into());
            x87.registers[0] = "0000000000000080ff3f".into();
            if source == 0 {
                x87.registers[0] = "00000000000000000000".into();
            } else {
                x87.registers[usize::from(source)] = "00000000000000000000".into();
            }
            let top = 0usize;
            let invalid_slot = usize::from(source);
            let mut tags = u16::MAX & !3;
            if invalid_slot == top {
                tags = (tags & !3) | 3;
            } else {
                tags = (tags & !(3 << (invalid_slot * 2))) | (3 << (invalid_slot * 2));
            }
            x87.tag_word = HexU64(u64::from(tags));
            let code = invalid
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.initial_bytes_hex = format!("db{:02x}", 0xd0 | source);
            code.length = 2;
            code.comparison_mask_hex = "ffff".into();
            assert!(Fixture::from_json(&invalid.to_json().unwrap()).is_ok());
        }
    }

    let mut bad_environment = invalid;
    bad_environment.case_id = "x87_fcmovnbe_bad_environment".into();
    bad_environment
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .control_word = HexU64(0x077f);
    bad_environment
        .initial_state
        .x87
        .as_mut()
        .unwrap()
        .registers[1] = "0000000000000080ff3f".into();
    assert!(Fixture::from_json(&bad_environment.to_json().unwrap()).is_ok());

    for source in 0..8u8 {
        let mut bad_environment = base.clone();
        bad_environment.case_id = format!("x87_fcmovnbe_bad_environment_{source}");
        bad_environment.stop_rip = None;
        bad_environment.expected_exit = ExpectedExit::Unsupported;
        bad_environment.instruction_budget = 1;
        let x87 = bad_environment.initial_state.x87.as_mut().unwrap();
        x87.control_word = HexU64(0x077f);
        let code = bad_environment
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = format!("db{:02x}", 0xd0 | source);
        code.length = 2;
        code.comparison_mask_hex = "ffff".into();
        assert!(Fixture::from_json(&bad_environment.to_json().unwrap()).is_ok());
    }
}

#[test]
fn x87_fcmovu_schema_admits_exact_indexed_and_test_pf_producer_streams() {
    let base = x87_fixture().fixture;
    let program = "d9e8d9ee4885c0dad9dfe0";
    for rax in [1, 3] {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovu_test_rax_{rax}");
        fixture.initial_state.gpr[0] = HexU64(rax);
        fixture.instruction_budget = 5;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + 11));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = program.into();
        code.length = 11;
        code.comparison_mask_hex = "ff".repeat(11);
        assert!(Fixture::from_json(&fixture.to_json().unwrap()).is_ok());
    }

    for source in 0..8u8 {
        for (suffix, budget) in [("", 1), ("dfe0", 2)] {
            let stream = format!("da{:02x}{suffix}", 0xd8 | source);
            let mut fixture = base.clone();
            fixture.case_id = format!("x87_fcmovu_index_{source}_{budget}");
            fixture.instruction_budget = budget;
            fixture.stop_rip = Some(HexU64(
                fixture.initial_state.rip.get() + u64::from(budget) * 2,
            ));
            let code = fixture
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.initial_bytes_hex = stream.clone();
            code.length = (stream.len() / 2) as u32;
            code.comparison_mask_hex = "ff".repeat(stream.len() / 2);
            assert!(
                Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
                "{stream}"
            );
        }
    }
    let mut repeat = base.clone();
    repeat.case_id = "x87_fcmovu_repeat".into();
    repeat.instruction_budget = 3;
    repeat.stop_rip = Some(HexU64(repeat.initial_state.rip.get() + 6));
    let code = repeat
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = "dad9dad9dfe0".into();
    code.length = 6;
    code.comparison_mask_hex = "ff".repeat(6);
    assert!(Fixture::from_json(&repeat.to_json().unwrap()).is_ok());

    for malformed in [
        "d9e8d9ee4885c0dad8dfe0",
        "d9e8d9ee4885c0dadadfe0",
        "66d9e8d9ee4885c0dad9dfe0",
        "d9e8d9ee4885c0dad9dfe090",
        "d9e8d9ee4885c0dad9",
    ] {
        let mut fixture = base.clone();
        fixture.case_id = format!("x87_fcmovu_reject_{malformed}");
        fixture.instruction_budget = 5;
        let length = (malformed.len() / 2) as u32;
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + u64::from(length)));
        let code = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.initial_bytes_hex = malformed.into();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(fixture.validate().is_err(), "{malformed}");
    }

    let mut wrong_budget = base.clone();
    wrong_budget.instruction_budget = 4;
    wrong_budget.stop_rip = Some(HexU64(wrong_budget.initial_state.rip.get() + 11));
    let code = wrong_budget
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = program.into();
    code.length = 11;
    code.comparison_mask_hex = "ff".repeat(11);
    assert!(wrong_budget.validate().is_err());

    let mut missing_x87 = base;
    missing_x87.initial_state.x87 = None;
    missing_x87.instruction_budget = 5;
    missing_x87.stop_rip = Some(HexU64(missing_x87.initial_state.rip.get() + 11));
    let code = missing_x87
        .memory
        .iter_mut()
        .find(|region| region.region_id == "code")
        .unwrap();
    code.initial_bytes_hex = program.into();
    code.length = 11;
    code.comparison_mask_hex = "ff".repeat(11);
    assert!(missing_x87.validate().is_err());
}

#[test]
fn result_rejects_observed_fs_base_different_from_initial() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22a_fs_base_qword_load_alias.case.json"
    ))
    .unwrap();
    let left = result(&input);
    let mut right = result(&input);
    snapshot(&mut right).state.fs_base = Some(HexU64(0xffff_8000_0000_0001));
    assert_eq!(
        right.validate_for(&input),
        Err(FormatError::CaseMismatch("state.fs_base"))
    );
    assert_eq!(
        compare(&input, &left, &right),
        Err(FormatError::CaseMismatch("state.fs_base"))
    );
}

#[test]
fn gs_base_profile_is_required_and_result_compares_gs_base() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22b_gs_base_qword_load_alias.case.json"
    ))
    .unwrap();
    assert_eq!(input.fixture.profile, Profile::LongGsBaseV1);
    assert_eq!(
        input.fixture.initial_state.gs_base,
        Some(HexU64(0xffff_8000_0000_0000))
    );
    let mut json: serde_json::Value =
        serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
    json["initial_state"]
        .as_object_mut()
        .unwrap()
        .remove("gs_base");
    assert!(Fixture::from_json(&serde_json::to_vec(&json).unwrap()).is_err());

    let mut right = result(&input);
    snapshot(&mut right).state.gs_base = Some(HexU64(0xffff_8000_0000_0001));
    assert!(matches!(
        right.validate_for(&input),
        Err(FormatError::CaseMismatch("state.gs_base"))
    ));
}

#[test]
fn fs_selector_profile_requires_and_compares_selector_and_base() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22c_fs_selector_to_rax.case.json"
    ))
    .unwrap();
    assert_eq!(input.fixture.profile, Profile::LongFsSelectorV1);
    assert_eq!(
        input.fixture.initial_state.fs_selector,
        Some(HexU64(0xf123))
    );
    let mut result = result(&input);
    snapshot(&mut result).state.gpr[0] = HexU64(0xf123);
    assert!(result.validate_for(&input).is_ok());

    for field in ["fs_selector", "fs_base"] {
        let mut json: serde_json::Value =
            serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
        json["initial_state"].as_object_mut().unwrap().remove(field);
        assert!(Fixture::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    let mut too_wide: serde_json::Value =
        serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
    too_wide["initial_state"]["fs_selector"] = serde_json::json!("0x0000000000010000");
    assert!(Fixture::from_json(&serde_json::to_vec(&too_wide).unwrap()).is_err());
    let mut null_selector: serde_json::Value =
        serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
    null_selector["initial_state"]["fs_selector"] = serde_json::Value::Null;
    assert!(Fixture::from_json(&serde_json::to_vec(&null_selector).unwrap()).is_err());

    for field in ["fs_selector", "fs_base"] {
        let mut changed = result.clone();
        let state = &mut snapshot(&mut changed).state;
        if field == "fs_selector" {
            state.fs_selector = Some(HexU64(0xf122));
        } else {
            state.fs_base = Some(HexU64(0xffff_8000_0000_1001));
        }
        assert!(matches!(
            changed.validate_for(&input),
            Err(FormatError::CaseMismatch(_))
        ));
    }
}

#[test]
fn gs_selector_profile_requires_and_compares_selector_and_base() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22d_gs_selector_to_rax.case.json"
    ))
    .unwrap();
    assert_eq!(input.fixture.profile, Profile::LongGsSelectorV1);
    assert_eq!(
        input.fixture.initial_state.gs_selector,
        Some(HexU64(0xf123))
    );
    let mut result = result(&input);
    snapshot(&mut result).state.gpr[0] = HexU64(0xf123);
    assert!(result.validate_for(&input).is_ok());

    for field in ["gs_selector", "gs_base"] {
        let mut json: serde_json::Value =
            serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
        json["initial_state"].as_object_mut().unwrap().remove(field);
        assert!(Fixture::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
        json["initial_state"][field] = serde_json::Value::Null;
        assert!(Fixture::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    let mut too_wide: serde_json::Value =
        serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
    too_wide["initial_state"]["gs_selector"] = serde_json::json!("0x0000000000010000");
    assert!(Fixture::from_json(&serde_json::to_vec(&too_wide).unwrap()).is_err());
    let mut null_selector: serde_json::Value =
        serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
    null_selector["initial_state"]["gs_selector"] = serde_json::Value::Null;
    assert!(Fixture::from_json(&serde_json::to_vec(&null_selector).unwrap()).is_err());

    for field in ["gs_selector", "gs_base"] {
        let mut changed = result.clone();
        let state = &mut snapshot(&mut changed).state;
        if field == "gs_selector" {
            state.gs_selector = Some(HexU64(0xf122));
        } else {
            state.gs_base = Some(HexU64(0xffff_8000_0000_1001));
        }
        assert!(matches!(
            changed.validate_for(&input),
            Err(FormatError::CaseMismatch(_))
        ));
    }

    let fs_selector = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22c_fs_selector_to_rax.case.json"
    ))
    .unwrap();
    let old_fs_json: serde_json::Value =
        serde_json::from_slice(&fs_selector.fixture.to_json().unwrap()).unwrap();
    assert!(old_fs_json["initial_state"].get("gs_selector").is_none());
}

#[test]
fn verw_gdt_profile_requires_canonical_gdtr_and_compares_both_fields() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22e_verw_writable_dpl3.case.json"
    ))
    .unwrap();
    assert_eq!(input.fixture.profile, Profile::LongVerwGdtV1);
    assert_eq!(
        input.fixture.initial_state.gdtr_base,
        Some(HexU64(0x20_0000))
    );
    assert_eq!(input.fixture.initial_state.gdtr_limit, Some(HexU64(0x0f)));
    let cpu = input
        .fixture
        .initial_state
        .to_cpu_for(Profile::LongVerwGdtV1)
        .unwrap();
    assert_eq!(
        cpu.gdtr,
        Some(od_isa::state::Gdtr {
            base: 0x20_0000,
            limit: 0x0f,
        })
    );

    for field in ["gdtr_base", "gdtr_limit"] {
        let mut json: serde_json::Value =
            serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
        json["initial_state"].as_object_mut().unwrap().remove(field);
        assert!(Fixture::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
        json["initial_state"][field] = serde_json::Value::Null;
        assert!(Fixture::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
    let mut too_wide: serde_json::Value =
        serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
    too_wide["initial_state"]["gdtr_limit"] = serde_json::json!("0x0000000000010000");
    assert!(Fixture::from_json(&serde_json::to_vec(&too_wide).unwrap()).is_err());
    for invalid_base in [0x0000_8000_0000_0000_u64, u64::MAX - 8] {
        let mut invalid: serde_json::Value =
            serde_json::from_slice(&input.fixture.to_json().unwrap()).unwrap();
        invalid["initial_state"]["gdtr_base"] = serde_json::json!(format!("0x{invalid_base:016x}"));
        assert!(Fixture::from_json(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }

    let mut result = result(&input);
    assert!(result.validate_for(&input).is_ok());
    let snapshot = snapshot(&mut result);
    snapshot.state.gdtr_limit = Some(HexU64(0x10));
    assert!(matches!(
        result.validate_for(&input),
        Err(FormatError::CaseMismatch("state.gdtr"))
    ));

    let old = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22d_gs_selector_to_rax.case.json"
    ))
    .unwrap();
    let old_json: serde_json::Value =
        serde_json::from_slice(&old.fixture.to_json().unwrap()).unwrap();
    assert!(old_json["initial_state"].get("gdtr_base").is_none());
    assert!(old_json["initial_state"].get("gdtr_limit").is_none());
}

#[test]
fn fs_load_profile_has_an_isolated_wire_shape_and_observes_hidden_cache_state() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22f_fs_load_writable_g0.case.json"
    ))
    .unwrap();
    assert_eq!(loaded.fixture.profile, Profile::LongFsLoadV1);
    let wire: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    for field in [
        "fs_base",
        "fs_selector",
        "gdtr_base",
        "gdtr_limit",
        "fs_cache_limit",
        "fs_cache_access",
        "fs_cache_flags",
    ] {
        assert!(wire["initial_state"].get(field).is_some(), "{field}");
    }

    let mut result = result(&loaded);
    let initial_snapshot = snapshot(&mut result);
    initial_snapshot.state.fs_base = Some(HexU64(0x8123_4567));
    initial_snapshot.state.fs_selector = Some(HexU64(0x0b));
    initial_snapshot.state.fs_cache_limit = Some(HexU64(0x0000_ffff));
    initial_snapshot.state.fs_cache_access = Some(HexU64(0xf3));
    initial_snapshot.state.fs_cache_flags = Some(HexU64(0));
    assert!(result.validate_for(&loaded).is_ok());

    let baseline = result.clone();
    let mut changed_cache = baseline.clone();
    snapshot(&mut changed_cache).state.fs_cache_limit = Some(HexU64(0x10000));
    assert_eq!(
        compare(&loaded, &baseline, &changed_cache).unwrap(),
        vec!["fs_cache_limit"]
    );

    for field in [
        "fs_base",
        "fs_selector",
        "gdtr_base",
        "gdtr_limit",
        "fs_cache_limit",
        "fs_cache_access",
        "fs_cache_flags",
    ] {
        let mut malformed: serde_json::Value =
            serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
        malformed["initial_state"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err(),
            "missing {field}"
        );
        malformed["initial_state"][field] = serde_json::Value::Null;
        assert!(
            Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err(),
            "null {field}"
        );
    }

    let mut too_wide_cache: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    too_wide_cache["initial_state"]["fs_cache_access"] = serde_json::json!("0x0000000000000100");
    assert!(Fixture::from_json(&serde_json::to_vec(&too_wide_cache).unwrap()).is_err());
    let mut selector_with_ti: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    selector_with_ti["initial_state"]["fs_selector"] = serde_json::json!("0x000000000000000c");
    assert!(Fixture::from_json(&serde_json::to_vec(&selector_with_ti).unwrap()).is_ok());
    selector_with_ti["initial_state"]["fs_selector"] = serde_json::json!("0x0000000000010000");
    assert!(Fixture::from_json(&serde_json::to_vec(&selector_with_ti).unwrap()).is_err());

    let old = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_21g_mfence_store_fence_load.case.json"
    ))
    .unwrap();
    let old_wire: serde_json::Value =
        serde_json::from_slice(&old.fixture.to_json().unwrap()).unwrap();
    for field in ["fs_cache_limit", "fs_cache_access", "fs_cache_flags"] {
        assert!(
            old_wire["initial_state"].get(field).is_none(),
            "legacy field {field}"
        );
    }
}

#[test]
fn gs_load_profile_has_an_isolated_wire_shape_and_observes_hidden_cache_state() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22h_gs_load_writable_g0.case.json"
    ))
    .unwrap();
    assert_eq!(loaded.fixture.profile, Profile::LongGsLoadV1);
    let wire: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    for field in [
        "gs_base",
        "gs_selector",
        "gdtr_base",
        "gdtr_limit",
        "gs_cache_limit",
        "gs_cache_access",
        "gs_cache_flags",
    ] {
        assert!(wire["initial_state"].get(field).is_some(), "{field}");
    }

    let mut result = result(&loaded);
    let initial_snapshot = snapshot(&mut result);
    initial_snapshot.state.gs_base = Some(HexU64(0x8123_4567));
    initial_snapshot.state.gs_selector = Some(HexU64(0x0b));
    initial_snapshot.state.gs_cache_limit = Some(HexU64(0x0000_ffff));
    initial_snapshot.state.gs_cache_access = Some(HexU64(0xf3));
    initial_snapshot.state.gs_cache_flags = Some(HexU64(0));
    assert!(result.validate_for(&loaded).is_ok());

    let baseline = result.clone();
    let mut changed_cache = baseline.clone();
    snapshot(&mut changed_cache).state.gs_cache_limit = Some(HexU64(0x10000));
    assert_eq!(
        compare(&loaded, &baseline, &changed_cache).unwrap(),
        vec!["gs_cache_limit"]
    );

    for field in [
        "gs_base",
        "gs_selector",
        "gdtr_base",
        "gdtr_limit",
        "gs_cache_limit",
        "gs_cache_access",
        "gs_cache_flags",
    ] {
        let mut malformed: serde_json::Value =
            serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
        malformed["initial_state"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err(),
            "missing {field}"
        );
        malformed["initial_state"][field] = serde_json::Value::Null;
        assert!(
            Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err(),
            "null {field}"
        );
    }

    let mut too_wide_cache: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    too_wide_cache["initial_state"]["gs_cache_access"] = serde_json::json!("0x0000000000000100");
    assert!(Fixture::from_json(&serde_json::to_vec(&too_wide_cache).unwrap()).is_err());
    let mut selector_with_ti: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    selector_with_ti["initial_state"]["gs_selector"] = serde_json::json!("0x000000000000000c");
    assert!(Fixture::from_json(&serde_json::to_vec(&selector_with_ti).unwrap()).is_ok());
    selector_with_ti["initial_state"]["gs_selector"] = serde_json::json!("0x0000000000010000");
    assert!(Fixture::from_json(&serde_json::to_vec(&selector_with_ti).unwrap()).is_err());

    let old = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_21g_mfence_store_fence_load.case.json"
    ))
    .unwrap();
    let old_wire: serde_json::Value =
        serde_json::from_slice(&old.fixture.to_json().unwrap()).unwrap();
    for field in ["gs_cache_limit", "gs_cache_access", "gs_cache_flags"] {
        assert!(
            old_wire["initial_state"].get(field).is_none(),
            "legacy field {field}"
        );
    }
}

#[test]
fn null_ds_es_profile_roundtrips_only_its_selector_and_unusable_state() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22k_null_ds_then_es.case.json"
    ))
    .unwrap();
    assert_eq!(loaded.fixture.profile, Profile::LongNullDsEsV1);
    let wire: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    for field in ["ds_selector", "es_selector", "ds_unusable", "es_unusable"] {
        assert!(wire["initial_state"].get(field).is_some(), "{field}");
    }
    for field in [
        "fs_base",
        "gs_base",
        "gdtr_base",
        "gdtr_limit",
        "fs_cache_limit",
    ] {
        assert!(wire["initial_state"].get(field).is_none(), "{field}");
    }
    let cpu = loaded
        .fixture
        .initial_state
        .to_cpu_for(Profile::LongNullDsEsV1)
        .unwrap();
    assert_eq!(cpu.ds_selector, 0x2b);
    assert_eq!(cpu.es_selector, 0x30);
    assert!(!cpu.ds_unusable && !cpu.es_unusable);
    let output = IntegerState::from_cpu_for(&cpu, Profile::LongNullDsEsV1).unwrap();
    assert_eq!(output.ds_selector, Some(HexU64(0x2b)));
    assert_eq!(output.es_selector, Some(HexU64(0x30)));
    assert_eq!(output.ds_unusable, Some(false));
    assert_eq!(output.es_unusable, Some(false));

    for field in ["ds_selector", "es_selector", "ds_unusable", "es_unusable"] {
        let mut malformed: serde_json::Value =
            serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
        malformed["initial_state"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err(),
            "{field}"
        );
        malformed["initial_state"][field] = serde_json::Value::Null;
        assert!(
            Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err(),
            "null {field}"
        );
    }
    let mut too_wide: serde_json::Value =
        serde_json::from_slice(&loaded.fixture.to_json().unwrap()).unwrap();
    too_wide["initial_state"]["ds_selector"] = serde_json::json!("0x0000000000010000");
    assert!(Fixture::from_json(&serde_json::to_vec(&too_wide).unwrap()).is_err());

    let mut out_of_profile = loaded.fixture.clone();
    out_of_profile.memory[0].initial_bytes_hex = "8ed8".into();
    assert!(out_of_profile.validate().is_err());
    out_of_profile = loaded.fixture.clone();
    out_of_profile.memory[0].initial_bytes_hex = "8ed880".into();
    out_of_profile.memory[0].length = 3;
    out_of_profile.memory[0].comparison_mask_hex = "ffffff".into();
    out_of_profile.stop_rip = Some(HexU64(0x0010_0003));
    assert!(out_of_profile.validate().is_err());
}

#[test]
fn fs_load_state_change_is_allowed_after_retirement_independent_of_terminal_outcome() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22f_fs_load_writable_g0.case.json"
    ))
    .unwrap();
    let mut observed = result(&loaded);
    let state = &mut snapshot(&mut observed).state;
    state.fs_base = Some(HexU64(0x89ab_cdef));
    state.fs_selector = Some(HexU64(0x0b));
    state.fs_cache_limit = Some(HexU64(0));
    state.fs_cache_access = Some(HexU64(0xf3));
    state.fs_cache_flags = Some(HexU64(0));

    for outcome in [
        Outcome::Exception {},
        Outcome::Unsupported {
            reason: "later instruction is outside the profile".into(),
        },
        Outcome::HarnessError {
            reason: "later backend operation failed".into(),
        },
    ] {
        let mut after_mov = observed.clone();
        after_mov.outcome = outcome;
        after_mov.instructions_executed = Observation::Available { value: HexU64(1) };
        if matches!(after_mov.outcome, Outcome::Exception {}) {
            let faulting_rip = snapshot(&mut after_mov).state.rip;
            snapshot(&mut after_mov).exception = Observation::Available {
                value: Some(ExceptionSnapshot {
                    vector: 6,
                    error_code: Observation::Unavailable {
                        reason: "exception has no error code".into(),
                    },
                    faulting_rip,
                    fault_address: Observation::Unavailable {
                        reason: "exception has no reported linear address".into(),
                    },
                }),
            };
        }
        assert!(after_mov.validate_for(&loaded).is_ok());
    }

    let mut no_retired_instruction = observed;
    no_retired_instruction.instructions_executed = Observation::Available { value: HexU64(0) };
    assert_eq!(
        no_retired_instruction.validate_for(&loaded),
        Err(FormatError::CaseMismatch("state.fs_base"))
    );
}

#[test]
fn every_gpr_rip_flag_definedness_memory_and_outcome_mismatch_is_observed() {
    let input = loaded();
    let baseline = result(&input);
    assert!(compare(&input, &baseline, &baseline).unwrap().is_empty());
    for i in 0..16 {
        let mut changed = baseline.clone();
        snapshot(&mut changed).state.gpr[i].0 ^= 1;
        assert_eq!(
            compare(&input, &baseline, &changed).unwrap(),
            vec![format!("gpr[{i}]")]
        );
    }
    for kind in ["rip", "rflags", "rflags_defined", "memory"] {
        let mut changed = baseline.clone();
        let state = snapshot(&mut changed);
        match kind {
            "rip" => state.state.rip.0 ^= 1,
            "rflags" => state.state.rflags.0 ^= 1,
            "rflags_defined" => state.rflags_defined.0 ^= 1 << 10,
            _ => state.memory[0].bytes_hex.replace_range(..2, "00"),
        }
        assert!(
            !compare(&input, &baseline, &changed).unwrap().is_empty(),
            "{kind}"
        );
    }
    let mut changed = baseline.clone();
    changed.outcome = Outcome::InstructionBudgetExhausted {};
    assert!(!compare(&input, &baseline, &changed).unwrap().is_empty());
    assert!(
        !compare(&input, &changed, &changed).unwrap().is_empty(),
        "same wrong exit is not success"
    );
    snapshot(&mut changed).rflags_defined = HexU64(0);
    assert!(compare(&input, &baseline, &changed).is_err());
}

#[test]
fn exact_input_and_layout_are_required_and_incomplete_results_never_match() {
    let input = loaded();
    let baseline = result(&input);
    assert_eq!(
        RunResult::from_json(&baseline.to_json().unwrap()).unwrap(),
        baseline
    );
    for kind in ["digest", "case", "address", "length", "missing"] {
        let mut changed = baseline.clone();
        match kind {
            "digest" => changed.input_sha256 = "0".repeat(64),
            "case" => changed.case_id = "different_case".into(),
            "address" => snapshot(&mut changed).memory[0].guest_address.0 += 1,
            "length" => snapshot(&mut changed).memory[0].bytes_hex.push_str("00"),
            _ => snapshot(&mut changed).memory.clear(),
        }
        assert!(compare(&input, &baseline, &changed).is_err(), "{kind}");
    }
    for outcome in [
        Outcome::Unsupported {
            reason: "not implemented".into(),
        },
        Outcome::TimedOut {
            reason: "deadline".into(),
        },
        Outcome::HarnessError {
            reason: "failed".into(),
        },
        Outcome::Signaled {
            signal: 11,
            reason: "host signal".into(),
        },
    ] {
        let mut changed = baseline.clone();
        changed.outcome = outcome;
        changed.snapshot = Observation::Unavailable {
            reason: "not observed".into(),
        };
        assert!(changed.validate().is_ok());
        assert!(compare(&input, &changed, &changed).is_err());
    }
    let mut malformed = serde_json::to_value(&baseline).unwrap();
    malformed["outcome"]["reason"] = serde_json::json!("hidden field");
    assert!(RunResult::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err());
    let mut absent = baseline.clone();
    absent.snapshot = Observation::Unavailable {
        reason: "missing".into(),
    };
    assert!(absent.validate().is_err());
}

#[test]
fn exception_metadata_is_observed_without_substituting_zero_for_absence() {
    let mut input = loaded();
    input.fixture.expected_exit = ExpectedExit::Exception;
    let mut baseline = result(&input);
    baseline.outcome = Outcome::Exception {};
    let rip = snapshot(&mut baseline).state.rip;
    snapshot(&mut baseline).exception = Observation::Available {
        value: Some(ExceptionSnapshot {
            vector: 14,
            error_code: Observation::Available { value: HexU64(5) },
            faulting_rip: rip,
            fault_address: Observation::Available {
                value: HexU64(0x1234),
            },
        }),
    };
    for kind in ["vector", "error_code", "fault_address"] {
        let mut changed = baseline.clone();
        if let Observation::Available { value: Some(fault) } = &mut snapshot(&mut changed).exception
        {
            match kind {
                "vector" => fault.vector = 13,
                "error_code" => fault.error_code = Observation::Available { value: HexU64(0) },
                _ => fault.fault_address = Observation::Available { value: HexU64(0) },
            }
        }
        assert_eq!(
            compare(&input, &baseline, &changed).unwrap(),
            vec!["exception"]
        );
    }
    snapshot(&mut baseline).exception = Observation::Unavailable {
        reason: "host signal is not a guest exception".into(),
    };
    assert!(baseline.validate().is_err());
}

#[test]
fn exception_comparison_ignores_unavailable_explanations_but_requires_architectural_fields() {
    let mut input = loaded();
    input.fixture.expected_exit = ExpectedExit::Exception;
    let mut left = result(&input);
    left.outcome = Outcome::Exception {};
    let rip = snapshot(&mut left).state.rip;
    snapshot(&mut left).exception = Observation::Available {
        value: Some(ExceptionSnapshot {
            vector: 13,
            error_code: Observation::Available { value: HexU64(0) },
            faulting_rip: rip,
            fault_address: Observation::Unavailable {
                reason: "GP has no fault address".into(),
            },
        }),
    };
    let mut right = left.clone();
    if let Observation::Available { value: Some(fault) } = &mut snapshot(&mut right).exception {
        fault.fault_address = Observation::Unavailable {
            reason: "not applicable".into(),
        };
    }
    assert!(compare(&input, &left, &right).unwrap().is_empty());
    if let Observation::Available { value: Some(fault) } = &mut snapshot(&mut right).exception {
        fault.fault_address = Observation::Available { value: HexU64(0) };
    }
    assert_eq!(compare(&input, &left, &right).unwrap(), ["exception"]);
    for vector in [13, 14] {
        let mut missing = left.clone();
        if let Observation::Available { value: Some(fault) } = &mut snapshot(&mut missing).exception
        {
            fault.vector = vector;
            if vector == 13 {
                fault.error_code = Observation::Unavailable {
                    reason: "not observed".into(),
                };
            }
        }
        assert!(compare(&input, &missing, &missing).is_err());
    }
    // An exception-free observation is distinct from an unknown exception observation.
    snapshot(&mut right).exception = Observation::Available { value: None };
    assert!(right.validate().is_err());
    snapshot(&mut right).exception = Observation::Unavailable {
        reason: "unknown".into(),
    };
    assert!(right.validate().is_err());
}

#[test]
fn psubusw_profile_admits_only_its_exact_register_encoding() {
    let raw =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_35_9_psubusw_register.case.json");
    let valid = Fixture::from_json(raw).unwrap();
    assert_eq!(valid.fixture.profile, Profile::LongPsubuswRegV1);
    for bytes in [
        "0fd9c1",       // bare MMX
        "f2660fd9c1",   // other prefix
        "66480fd9c1",   // REX.W
        "66420fd9c1",   // REX.X
        "660fd901",     // memory operand
        "660fd9",       // truncated ModR/M
        "c5f9d9c1",     // VEX
        "c4e179d9c1",   // VEX3
        "62f17d08d9c1", // EVEX
        "66660fd9c1",   // duplicate mandatory prefix
    ] {
        let mut root: serde_json::Value = serde_json::from_slice(raw).unwrap();
        let byte_len = (bytes.len() / 2) as u32;
        root["memory"][0]["length"] = byte_len.into();
        root["memory"][0]["initial_bytes_hex"] = bytes.into();
        root["memory"][0]["comparison_mask_hex"] = "ff".repeat(byte_len as usize).into();
        root["stop_rip"] = format!("0x{:016x}", 0x0010_0000 + byte_len).into();
        let error = Fixture::from_json(&serde_json::to_vec(&root).unwrap()).unwrap_err();
        assert!(
            matches!(
                error,
                FormatError::InvalidField {
                    field: "memory.code",
                    ..
                }
            ),
            "{bytes}: {error}"
        );
    }
}

#[test]
fn psubsb_profile_admits_only_its_exact_register_encoding() {
    let raw =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_35_11_psubsb_register.case.json");
    let valid = Fixture::from_json(raw).unwrap();
    assert_eq!(valid.fixture.profile, Profile::LongPsubsbRegV1);
    assert_eq!(valid.fixture.memory[0].initial_bytes_hex, "66450fe8c1");
    for bytes in [
        "0fe8c1",     // bare MMX
        "f2660fe8c1", // extra prefix
        "f3660fe8c1",
        "67660fe8c1",
        "f0660fe8c1",
        "66480fe8c1",   // REX.W
        "66420fe8c1",   // REX.X
        "6640410fe8c1", // two REX prefixes
        "660fe801",     // memory form
        "660fe8",       // truncated ModR/M
        "c5f9e8c1",     // VEX
        "c4e179e8c1",   // VEX3
        "62f17d08e8c1", // EVEX
        "66660fe8c1",   // duplicate mandatory prefix
    ] {
        let mut invalid = valid.fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = valid.fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
}

#[test]
fn paddsw_profile_admits_only_its_exact_register_encoding() {
    let raw =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_35_12_paddsw_register.case.json");
    let valid = Fixture::from_json(raw).unwrap();
    assert_eq!(valid.fixture.profile, Profile::LongPaddswRegV1);
    assert_eq!(valid.fixture.memory[0].initial_bytes_hex, "66450fedc1");
    for bytes in [
        "0fedc1",     // bare MMX
        "f2660fedc1", // extra prefix
        "f3660fedc1",
        "67660fedc1",
        "f0660fedc1",
        "66480fedc1",   // REX.W
        "66420fedc1",   // REX.X
        "6640410fedc1", // two REX prefixes
        "660fed01",     // memory form
        "660fed",       // truncated ModR/M
        "c5f9edc1",     // VEX
        "c4e179edc1",   // VEX3
        "62f17d08edc1", // EVEX
        "66660fedc1",   // duplicate mandatory prefix
    ] {
        let mut invalid = valid.fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = valid.fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
}

#[test]
fn psubsw_profile_admits_only_its_exact_register_encoding() {
    let raw =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_35_13_psubsw_register.case.json");
    let valid = Fixture::from_json(raw).unwrap();
    assert_eq!(valid.fixture.profile, Profile::LongPsubswRegV1);
    assert_eq!(valid.fixture.memory[0].initial_bytes_hex, "66450fe9c1");
    for bytes in [
        "0fe9c1",     // bare MMX
        "f2660fe9c1", // extra prefix
        "f3660fe9c1",
        "67660fe9c1",
        "f0660fe9c1",
        "66480fe9c1",   // REX.W
        "66420fe9c1",   // REX.X
        "6640410fe9c1", // two REX prefixes
        "660fe901",     // memory form
        "660fe9",       // truncated ModR/M
        "c5f9e9c1",     // VEX
        "c4e179e9c1",   // VEX3
        "62f17d08e9c1", // EVEX
        "66660fe9c1",   // duplicate mandatory prefix
    ] {
        let mut invalid = valid.fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = valid.fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
}

#[test]
fn paddd_profile_admits_only_its_exact_register_encoding() {
    let raw =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_35_14_paddd_register.case.json");
    let valid = Fixture::from_json(raw).unwrap();
    assert_eq!(valid.fixture.profile, Profile::LongPadddRegV1);
    assert_eq!(valid.fixture.memory[0].initial_bytes_hex, "66450ffec1");
    for bytes in [
        "0ffec1",     // bare MMX
        "f2660ffec1", // extra prefix
        "f3660ffec1",
        "67660ffec1",
        "f0660ffec1",
        "66480ffec1",   // REX.W
        "66420ffec1",   // REX.X
        "6640410ffec1", // two REX prefixes
        "660ffe01",     // memory form
        "660ffe",       // truncated ModR/M
        "c5f9fec1",     // VEX
        "c4e179fec1",   // VEX3
        "62f17d08fec1", // EVEX
        "66660ffec1",   // duplicate mandatory prefix
    ] {
        let mut invalid = valid.fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = valid.fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
}

#[test]
fn psubd_profile_admits_only_its_exact_register_encoding() {
    let raw =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_35_15_psubd_register.case.json");
    let valid = Fixture::from_json(raw).unwrap();
    assert_eq!(valid.fixture.profile, Profile::LongPsubdRegV1);
    assert_eq!(valid.fixture.memory[0].initial_bytes_hex, "66450ffac1");
    for bytes in [
        "0ffac1",
        "f2660ffac1",
        "f3660ffac1",
        "67660ffac1",
        "f0660ffac1",
        "66480ffac1",
        "66420ffac1",
        "6640410ffac1",
        "660ffa01",
        "660ffa",
        "c5f9fac1",
        "c4e179fac1",
        "62f17d08fac1",
        "66660ffac1",
    ] {
        let mut invalid = valid.fixture.clone();
        invalid.memory[0].initial_bytes_hex = bytes.into();
        invalid.memory[0].length = (bytes.len() / 2) as u32;
        invalid.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        invalid.stop_rip = Some(HexU64(
            invalid.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(invalid.validate().is_err(), "{bytes}");
    }
    let mut missing_sse = valid.fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());
}

#[test]
fn native_xorpd_profile_requires_sse_only_exact_input_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_40b_xorpd_xmm0_xmm1_asymmetric_a.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongNativeXorpdXmm0Xmm1V1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "660f57c1");
    assert!(fixture.initial_state.x87.is_none());
    assert_eq!(
        fixture.initial_state.sse.as_ref().unwrap().mxcsr.get(),
        0x1f80
    );

    let mut missing_sse = fixture.clone();
    missing_sse.initial_state.sse = None;
    assert!(missing_sse.validate().is_err());

    let json = String::from_utf8(fixture.to_json().unwrap()).unwrap();
    let with_null_x87 = json.replace("\"sse\": {", "\"x87\": null, \"sse\": {");
    assert_ne!(with_null_x87, json);
    assert!(Fixture::from_json(with_null_x87.as_bytes()).is_err());

    let mut with_x87 = fixture.clone();
    with_x87.initial_state.x87 = Some(
        Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_34_17_xorpd_register.case.json"
        ))
        .unwrap()
        .fixture
        .initial_state
        .x87
        .unwrap(),
    );
    assert!(with_x87.validate().is_err());

    let mut wrong_mxcsr = fixture.clone();
    wrong_mxcsr.initial_state.sse.as_mut().unwrap().mxcsr = HexU64(0x1f40);
    assert!(wrong_mxcsr.validate().is_err());

    for bytes in ["0f57c1", "f2660f57c1", "660f57c0", "660f57c2", "660f57c1c0"] {
        let mut wrong = fixture.clone();
        let code = &mut wrong.memory[0];
        code.initial_bytes_hex = bytes.into();
        code.length = (bytes.len() / 2) as u32;
        code.comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        wrong.stop_rip = Some(HexU64(
            wrong.initial_state.rip.get() + (bytes.len() / 2) as u64,
        ));
        assert!(wrong.validate().is_err(), "{bytes}");
    }
}
#[test]
fn cvtsi2sd_r64_independent_oracle_covers_signed_boundaries_ties_and_directed_rounding() {
    use super::independent_i64_to_binary64;
    let vectors = [
        (0, 0, 0x0000_0000_0000_0000, false),
        (1, 0, 0x3ff0_0000_0000_0000, false),
        (-1, 0, 0xbff0_0000_0000_0000, false),
        (i64::MIN, 0, 0xc3e0_0000_0000_0000, false),
        (i64::MIN + 1, 0, 0xc3e0_0000_0000_0000, true),
        (i64::MIN + 1, 1, 0xc3e0_0000_0000_0000, true),
        (i64::MIN + 1, 2, 0xc3df_ffff_ffff_ffff, true),
        (i64::MIN + 1, 3, 0xc3df_ffff_ffff_ffff, true),
        (i64::MAX - 1023, 0, 0x43df_ffff_ffff_ffff, false),
        (i64::MAX, 0, 0x43e0_0000_0000_0000, true),
        (i64::MAX, 1, 0x43df_ffff_ffff_ffff, true),
        (i64::MAX, 2, 0x43e0_0000_0000_0000, true),
        (i64::MAX, 3, 0x43df_ffff_ffff_ffff, true),
        ((1i64 << 53) - 1, 0, 0x433f_ffff_ffff_ffff, false),
        ((1i64 << 53) + 1, 0, 0x4340_0000_0000_0000, true),
        ((1i64 << 53) + 1, 2, 0x4340_0000_0000_0001, true),
        ((1i64 << 53) + 3, 0, 0x4340_0000_0000_0002, true),
        (-((1i64 << 53) + 1), 0, 0xc340_0000_0000_0000, true),
        (-((1i64 << 53) + 1), 1, 0xc340_0000_0000_0001, true),
        (-((1i64 << 53) + 1), 2, 0xc340_0000_0000_0000, true),
        (-((1i64 << 53) + 3), 1, 0xc340_0000_0000_0002, true),
        ((1i64 << 54) - 1, 0, 0x4350_0000_0000_0000, true),
        (-((1i64 << 54) - 1), 0, 0xc350_0000_0000_0000, true),
    ];
    for (value, rc, bits, inexact) in vectors {
        assert_eq!(
            independent_i64_to_binary64(value, rc),
            (bits, inexact),
            "RAX={value}, RC={rc}"
        );
    }
    // Midpoint neighbors around 2^53: below is exact, the two adjacent odd
    // integers are exact halfway cases whose even neighbor depends on parity.
    assert_eq!(
        independent_i64_to_binary64((1i64 << 53) - 1, 0).0,
        0x433f_ffff_ffff_ffff
    );
    assert_eq!(
        independent_i64_to_binary64((1i64 << 53) + 1, 0).0,
        0x4340_0000_0000_0000
    );
    assert_eq!(
        independent_i64_to_binary64((1i64 << 53) + 3, 0).0,
        0x4340_0000_0000_0002
    );
}

#[test]
fn cvtsi2ss_integer_oracle_covers_precision_boundaries_and_double_rounding() {
    let vectors = [
        (0, 0, 0x0000_0000, false),
        (1, 0, 0x3f80_0000, false),
        (-1, 0, 0xbf80_0000, false),
        ((1i64 << 24) - 1, 0, 0x4b7f_ffff, false),
        (-((1i64 << 24) - 1), 0, 0xcb7f_ffff, false),
        (1i64 << 24, 0, 0x4b80_0000, false),
        (-(1i64 << 24), 0, 0xcb80_0000, false),
        ((1i64 << 24) + 1, 0, 0x4b80_0000, true),
        ((1i64 << 24) + 3, 0, 0x4b80_0002, true),
        (-((1i64 << 24) + 1), 0, 0xcb80_0000, true),
        (-((1i64 << 24) + 1), 1, 0xcb80_0001, true),
        (-((1i64 << 24) + 1), 2, 0xcb80_0000, true),
        (-((1i64 << 24) + 1), 3, 0xcb80_0000, true),
        (-((1i64 << 24) + 3), 0, 0xcb80_0002, true),
        (i32::MAX as i64, 0, 0x4f00_0000, true),
        (i32::MIN as i64, 0, 0xcf00_0000, false),
        (i64::MAX, 0, 0x5f00_0000, true),
        (i64::MAX, 1, 0x5eff_ffff, true),
        (i64::MAX, 2, 0x5f00_0000, true),
        (i64::MIN, 0, 0xdf00_0000, false),
        ((1i64 << 55) + (1i64 << 31) + 1, 0, 0x5b00_0001, true),
        ((1i64 << 54) + (1i64 << 30) + 1, 0, 0x5a80_0001, true),
        ((1i64 << 54) + (1i64 << 30) + 1, 1, 0x5a80_0000, true),
        ((1i64 << 54) + (1i64 << 30) + 1, 2, 0x5a80_0001, true),
        ((1i64 << 54) + (1i64 << 30) + 1, 3, 0x5a80_0000, true),
        (-((1i64 << 54) + (1i64 << 30) + 1), 0, 0xda80_0001, true),
        (-((1i64 << 54) + (1i64 << 30) + 1), 1, 0xda80_0001, true),
        (-((1i64 << 54) + (1i64 << 30) + 1), 2, 0xda80_0000, true),
        (-((1i64 << 54) + (1i64 << 30) + 1), 3, 0xda80_0000, true),
    ];
    for (value, rc, bits, inexact) in vectors {
        assert_eq!(
            independent_i64_to_binary32(value, rc),
            (bits, inexact),
            "value={value}, RC={rc}"
        );
    }
}

#[test]
fn cvtsi2ss_parser_covers_all_register_pairs_and_rejects_noncanonical_prefixes() {
    for wide in [false, true] {
        for destination in 0..16u8 {
            for source in 0..16u8 {
                let mut bytes = vec![0xf3];
                let rex = match (wide, destination >= 8, source >= 8) {
                    (false, false, false) => None,
                    (false, true, false) => Some(0x44),
                    (false, false, true) => Some(0x41),
                    (false, true, true) => Some(0x45),
                    (true, false, false) => Some(0x48),
                    (true, false, true) => Some(0x49),
                    (true, true, false) => Some(0x4c),
                    (true, true, true) => Some(0x4d),
                };
                if let Some(rex) = rex {
                    bytes.push(rex);
                }
                bytes.extend_from_slice(&[
                    0x0f,
                    0x2a,
                    0xc0 | ((destination & 7) << 3) | (source & 7),
                ]);
                let text = bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                assert_eq!(
                    cvtsi2ss_register_encoding(&text),
                    Some((wide, destination, source, bytes.len() as u32))
                );
            }
        }
    }
    for invalid in [
        "f3400f2ac0",
        "f4420f2ac0",
        "f3490f2ac0c0",
        "f30f2a00",
        "f3660f2ac0",
        "f20f2ac0",
    ] {
        assert_eq!(cvtsi2ss_register_encoding(invalid), None, "{invalid}");
    }
}

#[test]
fn cvtss2si_integer_oracle_covers_signed_boundaries_rounding_and_event_priority() {
    let cases = [
        (0x0000_0000, 0, false, 0, false, false),
        (0x8000_0000, 0, false, 0, false, false),
        (0x3fc0_0000, 0, false, 2, false, true), // +1.5 nearest even
        (0x3fc0_0000, 1, false, 1, false, true),
        (0x3fc0_0000, 2, false, 2, false, true),
        (0x3fc0_0000, 3, false, 1, false, true),
        (0x3fbf_ffff, 0, false, 1, false, true), // adjacent below +1.5
        (0x3fc0_0001, 0, false, 2, false, true), // adjacent above +1.5
        (0xbfc0_0000, 0, false, u64::MAX - 1, false, true), // -2
        (0xcf00_0000, 0, false, 0xffff_ffff_8000_0000, false, false),
        (0xdf00_0000, 0, true, 0x8000_0000_0000_0000, false, false),
        (0xdf00_0000, 0, false, 0x8000_0000, true, false),
        (0x4f00_0000, 0, false, 0x8000_0000, true, false),
        (0x4eff_ffff, 0, false, 0x0000_0000_7fff_ff80, false, false),
        (0x4f00_0001, 0, false, 0x8000_0000, true, false),
        (0x5eff_ffff, 0, true, 0x7fff_ff80_0000_0000, false, false),
        (0x5f00_0000, 0, true, 0x8000_0000_0000_0000, true, false),
        (0xdf00_0001, 0, true, 0x8000_0000_0000_0000, true, false),
        (0x7f80_0000, 0, true, 0x8000_0000_0000_0000, true, false),
        (0x7fc0_0001, 0, false, 0x8000_0000, true, false),
        (0x7f80_0001, 0, true, 0x8000_0000_0000_0000, true, false),
        (0x0000_0001, 0, true, 0, false, true), // subnormal: inexact only, never DE
    ];
    for (bits, rc, wide, result, invalid, inexact) in cases {
        assert_eq!(
            independent_binary32_to_integer(bits, rc, wide),
            (result, invalid, inexact),
            "bits={bits:08x} RC={rc} width={wide}"
        );
    }
}

#[test]
fn cvtps2dq_raw_lane_oracle_keeps_invalid_and_valid_precision_events_separate() {
    for rc in 0..4 {
        assert_eq!(
            independent_binary32_to_integer(0x4f00_0000, rc, false),
            (0x8000_0000, true, false),
            "+2^31 is invalid without PE"
        );
        assert_eq!(
            independent_binary32_to_integer(0x3fc0_0000, rc, false),
            (if rc == 0 || rc == 2 { 2 } else { 1 }, false, true),
            "valid 1.5 contributes PE under every rounding mode"
        );
        assert_eq!(
            independent_binary32_to_integer(0xcf00_0000, rc, false),
            (0xffff_ffff_8000_0000, false, false),
            "exact -2^31 is valid"
        );
    }
}

#[test]
fn cvttps2dq_raw_lane_oracle_truncates_toward_zero_independent_of_rc() {
    for raw in [
        0x3fc0_0000, // +1.5
        0xbfc0_0000, // -1.5
        0x0000_0001, // positive subnormal
        0x8000_0001, // negative subnormal
        0x4f00_0000, // +2^31, out of range after truncation
        0xcf00_0000, // exact -2^31
        0x7fc0_0001, // QNaN
        0x7f80_0000, // +infinity
    ] {
        let mut expected = None;
        for rc in 0..4 {
            let mxcsr = 0x1f80 | (rc << 13);
            let observed = independent_binary32_to_integer_truncate(raw, mxcsr, false);
            if let Some(expected) = expected {
                assert_eq!(observed, expected, "raw={raw:08x}, RC={rc}");
            } else {
                expected = Some(observed);
            }
            assert_eq!(observed, expected.unwrap(), "raw={raw:08x}, RC={rc}");
        }
    }
    assert_eq!(
        independent_binary32_to_integer_truncate(0x3fc0_0000, 0x1f80, false),
        (1, false, true),
        "+1.5 truncates to +1 and raises PE"
    );
    assert_eq!(
        independent_binary32_to_integer_truncate(0xbfc0_0000, 0x3f80, false),
        (u64::MAX, false, true),
        "-1.5 truncates to -1 and raises PE"
    );
    assert_eq!(
        independent_binary32_to_integer_truncate(0xcf00_0000, 0x7f80, false),
        (0xffff_ffff_8000_0000, false, false),
        "exact -2^31 remains valid"
    );
    assert_eq!(
        independent_binary32_to_integer_truncate(0x4f00_0000, 0x9f80, false),
        (0x8000_0000, true, false),
        "+2^31 raises IE without PE"
    );
}

#[test]
fn cvtpd2dq_independent_oracle_rounds_before_i32_range_check() {
    let cases = [
        (
            0x3ff8_0000_0000_0000,
            [Some(2), Some(1), Some(2), Some(1)],
            [true; 4],
        ), // +1.5
        (
            0xbff8_0000_0000_0000,
            [Some(-2), Some(-2), Some(-1), Some(-1)],
            [true; 4],
        ), // -1.5
        (
            0x41df_ffff_ffe0_0000,
            [None, Some(2_147_483_647), None, Some(2_147_483_647)],
            [false, true, false, true],
        ), // +2147483647.5
        (
            0xc1e0_0000_0001_0000,
            [
                Some(-2_147_483_648),
                None,
                Some(-2_147_483_648),
                Some(-2_147_483_648),
            ],
            [true, false, true, true],
        ), // -2147483648.5
    ];
    for (raw, expected_values, expected_inexact) in cases {
        for rc in 0..4 {
            let (bits, observed_invalid, observed_inexact) =
                independent_binary64_to_integer(raw, rc, false);
            let expected_value = expected_values[rc as usize];
            assert_eq!(
                observed_invalid,
                expected_value.is_none(),
                "raw={raw:016x} RC={rc}"
            );
            assert_eq!(
                observed_inexact, expected_inexact[rc as usize],
                "raw={raw:016x} RC={rc}"
            );
            let expected = expected_value.unwrap_or(i32::MIN) as u32;
            assert_eq!(bits as u32, expected, "raw={raw:016x} RC={rc}");
        }
    }
    for rc in 0..4 {
        assert_eq!(
            independent_binary64_to_integer(0xc1e0_0000_0000_0000, rc, false),
            (0xffff_ffff_8000_0000, false, false),
            "exact -2^31 remains valid"
        );
        assert_eq!(
            independent_binary64_to_integer(0x41e0_0000_0000_0000, rc, false),
            (0x8000_0000, true, false),
            "+2^31 is invalid without PE"
        );
    }
}

#[test]
fn cvtsd2si_m64_r32_independent_oracle_rounds_then_range_checks() {
    let cases = [
        (
            0x41df_ffff_ffe0_0000,
            [None, Some(i32::MAX), None, Some(i32::MAX)],
            [false, true, false, true],
        ),
        (
            0xc1e0_0000_0010_0000,
            [Some(i32::MIN), None, Some(i32::MIN), Some(i32::MIN)],
            [true, false, true, true],
        ),
        (0xc1e0_0000_0000_0000, [Some(i32::MIN); 4], [false; 4]),
        (
            0x3ff8_0000_0000_0000,
            [Some(2), Some(1), Some(2), Some(1)],
            [true; 4],
        ),
        (
            0x0000_0000_0000_0001,
            [Some(0), Some(0), Some(1), Some(0)],
            [true; 4],
        ),
        (
            0x8000_0000_0000_0001,
            [Some(0), Some(-1), Some(0), Some(0)],
            [true; 4],
        ),
    ];
    for (raw, expected_values, expected_inexact) in cases {
        for rc in 0..4 {
            let (bits, invalid, inexact) = independent_binary64_to_integer(raw, rc, false);
            let expected_value = expected_values[rc as usize];
            assert_eq!(invalid, expected_value.is_none(), "raw={raw:016x}, RC={rc}");
            assert_eq!(
                inexact, expected_inexact[rc as usize],
                "raw={raw:016x}, RC={rc}"
            );
            assert_eq!(
                bits as u32,
                expected_value.unwrap_or(i32::MIN) as u32,
                "raw={raw:016x}, RC={rc}"
            );
            let old_ie = rc & 1 != 0;
            let old_pe = rc & 2 != 0;
            let status =
                u32::from(old_ie || invalid) | (u32::from(old_pe || (!invalid && inexact)) << 5);
            assert_eq!(status & 1 != 0, old_ie || invalid);
            assert_eq!(status & 0x20 != 0, old_pe || (!invalid && inexact));
        }
    }
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37cg_cvtsd2si_m64_i32_min.case.json"
    ))
    .unwrap();
    assert_eq!(fixture.fixture.profile, Profile::LongCvtsd2siR32MemoryRaxV1);
}

#[test]
fn cvttsd2si_m64_r32_independent_oracle_truncates_for_every_rc() {
    let cases = [
        (0x3ff8_0000_0000_0000, 1u32, false, true), // +1.5 -> +1, PE
        (0xbff8_0000_0000_0000, 0xffff_ffff, false, true), // -1.5 -> -1, PE
        (0x4004_0000_0000_0000, 2, false, true),    // +2.5 -> +2, PE
        (0xc004_0000_0000_0000, 0xffff_fffe, false, true), // -2.5 -> -2, PE
        (0x0000_0000_0000_0001, 0, false, true),    // tiny positive -> +0, no DE
        (0x8000_0000_0000_0001, 0, false, true),    // tiny negative -> -0, no DE
        (0xc1e0_0000_0000_0000, 0x8000_0000, false, false), // exact i32::MIN
        (0x41df_ffff_ffff_ffff, 0x7fff_ffff, false, true), // just below +2^31
        (0x41e0_0000_0000_0000, 0x8000_0000, true, false), // +2^31 invalid, no PE
        (0xc1e0_0000_0010_0000, 0x8000_0000, false, true), // MIN - 0.5 truncates valid
        (0xc1e0_0000_0020_0000, 0x8000_0000, true, false), // below MIN - 1
        (0x7ff8_0000_0000_0001, 0x8000_0000, true, false), // QNaN
        (0x7ff0_0000_0000_0000, 0x8000_0000, true, false), // +infinity
    ];
    for (raw, expected_bits, expected_invalid, expected_inexact) in cases {
        for rc in 0..4 {
            let (bits, invalid, inexact) = independent_binary64_to_integer(raw, 3, false);
            assert_eq!(bits as u32, expected_bits, "raw={raw:016x}, RC={rc}");
            assert_eq!(invalid, expected_invalid, "raw={raw:016x}, RC={rc}");
            assert_eq!(inexact, expected_inexact, "raw={raw:016x}, RC={rc}");
            let old_ie = rc & 1 != 0;
            let old_pe = rc & 2 != 0;
            let status =
                u32::from(old_ie || invalid) | (u32::from(old_pe || (!invalid && inexact)) << 5);
            assert_eq!(status & 1 != 0, old_ie || invalid);
            assert_eq!(status & 0x20 != 0, old_pe || (!invalid && inexact));
        }
    }
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37cj_cvttsd2si_m64_i32_rc_independence.case.json"
    ))
    .unwrap();
    assert_eq!(
        fixture.fixture.profile,
        Profile::LongCvttsd2siR32MemoryRaxV1
    );
    let mut foreign = fixture.fixture;
    foreign.cvtsd2si_r32_memory_controls = Some(Cvtsd2siControlSnapshot {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
        cr4_osxmmexcpt: false,
    });
    assert!(foreign.validate().is_err());
    assert!(Fixture::from_json(&serde_json::to_vec(&foreign).unwrap()).is_err());
}

#[test]
fn cvtpd2dq_parser_covers_all_register_pairs_and_rejects_neighbor_forms() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let modrm = 0xc0 | ((destination & 7) << 3) | (source & 7);
            let mut bytes = vec![0xf2];
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0xe6, modrm]);
            let code = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                cvtpd2dq_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in [
        "f30fe6c1", // neighboring CVTDQ2PD
        "66f20fe6c1",
        "f2f20fe6c1",
        "41f20fe6c1",
        "f2480fe6c1",
        "f2420fe6c1",
        "f2400fe6c1",
        "f20fe600",   // memory
        "f20fe6c190", // trailing byte
        "c5f9e6c1",
        "62f17d08e6c1",
    ] {
        assert_eq!(cvtpd2dq_register_encoding(code), None, "{code}");
    }
}

#[test]
fn cvttpd2dq_run_result_validate_requires_own_snapshot_controls_only() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37ai_cvttpd2dq_ie_lane0_pe_lane1.case.json"
    ))
    .unwrap();
    let valid = result(&input);
    assert!(valid.validate().is_ok());

    let mut missing = valid.clone();
    snapshot(&mut missing).cvttpd2dq_controls = None;
    assert!(missing.validate().is_err());

    let mut foreign = result(&loaded());
    snapshot(&mut foreign).cvttpd2dq_controls = input.fixture.cvttpd2dq_controls;
    assert!(foreign.validate().is_err());
}

#[test]
fn cvttpd2dq_independent_oracle_is_toward_zero_for_all_mxcsr_rc_values() {
    let cases = [
        (0x3ff8_0000_0000_0000, 1u32, false, true), // +1.5 -> +1, PE
        (0xbff8_0000_0000_0000, 0xffff_ffffu32, false, true), // -1.5 -> -1, PE
        (0xc1e0_0000_0000_0000, 0x8000_0000, false, false), // exact -2^31
        (0x41e0_0000_0000_0000, 0x8000_0000, true, false), // +2^31
        (0x7ff0_0000_0000_0000, 0x8000_0000, true, false), // +infinity
        (0x7ff8_0000_0000_0001, 0x8000_0000, true, false), // qNaN
        (0x0000_0000_0000_0001, 0, false, true),    // tiny positive
    ];
    for (raw, expected, invalid, inexact) in cases {
        for rc in 0..4 {
            assert_eq!(
                independent_binary64_to_integer(raw, 3, false),
                (
                    if invalid {
                        u64::from(expected)
                    } else {
                        i64::from(expected as i32) as u64
                    },
                    invalid,
                    inexact
                ),
                "raw={raw:016x} RC={rc}"
            );
        }
    }
}

#[test]
fn cvttpd2dq_parser_covers_all_register_pairs_and_rejects_neighbor_forms() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let modrm = 0xc0 | ((destination & 7) << 3) | (source & 7);
            let mut bytes = vec![0x66];
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0xe6, modrm]);
            let code = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                cvttpd2dq_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in [
        "f20fe6c1",
        "f30fe6c1",
        "66660fe6c1",
        "41660fe6c1",
        "66480fe6c1",
        "66420fe6c1",
        "66400fe6c1",
        "660fe600",
        "660fe6c190",
        "c5f9e6c1",
        "62f17d08e6c1",
    ] {
        assert_eq!(cvttpd2dq_register_encoding(code), None, "{code}");
    }
}

#[test]
fn sqrtss_parser_and_oracle_cover_register_space_and_special_values() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let mut bytes = vec![0xf3];
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0x51, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let code = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                sqrtss_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in ["f30f51c1", "f3410f51c1", "f3440f51c1", "f3450f51c1"] {
        assert!(sqrtss_register_encoding(code).is_some(), "{code}");
    }
    for code in [
        "f20f51c1",
        "f3660f51c1",
        "66f30f51c1",
        "f3480f51c1",
        "f3420f51c1",
        "f3400f51c1",
        "f30f5100",
        "f30f51c190",
        "c5f87ec1",
        "62f17e0851c1",
    ] {
        assert_eq!(sqrtss_register_encoding(code), None, "{code}");
    }
    assert_eq!(
        independent_sqrtss(0x4080_0000, 0x1f80),
        (0x4000_0000, false, false, false)
    );
    for (rc, expected) in [
        (0u32, 0x3fb5_04f3),
        (1, 0x3fb5_04f3),
        (2, 0x3fb5_04f4),
        (3, 0x3fb5_04f3),
    ] {
        assert_eq!(
            independent_sqrtss(0x4000_0000, 0x1f80 | (rc << 13)),
            (expected, false, false, true)
        );
    }
    assert_eq!(
        independent_sqrtss(0xbf80_0000, 0x1f80),
        (0xffc0_0000, true, false, false)
    );
    assert_eq!(
        independent_sqrtss(0x8000_0000, 0x1f80),
        (0x8000_0000, false, false, false)
    );
    assert_eq!(
        independent_sqrtss(0x7f80_0001, 0x1f80),
        (0x7fc0_0001, true, false, false)
    );
    assert_eq!(
        independent_sqrtss(0x7fc0_1234, 0x1f80),
        (0x7fc0_1234, false, false, false)
    );
    assert_eq!(independent_sqrtss(0x0000_0001, 0x1f80).1, false);
    assert_eq!(independent_sqrtss(0x8000_0001, 0x1f80).1, true);
    assert_eq!(independent_sqrtss(0x8000_0001, 0x1f80).2, false);
}

#[test]
fn sqrtps_parser_and_packed_raw_oracle_aggregate_lane_events() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let mut bytes = Vec::new();
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0x51, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let code = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                sqrtps_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in ["0f51c1", "410f51c1", "440f51c1", "450f51c1"] {
        assert!(sqrtps_register_encoding(code).is_some(), "{code}");
    }
    for code in [
        "f30f51c1",
        "660f51c1",
        "f20f51c1",
        "400f51c1",
        "480f51c1",
        "420f51c1",
        "0f5100",
        "0f51c190",
        "c5f87851c1",
        "62f17c0851c1",
    ] {
        assert_eq!(sqrtps_register_encoding(code), None, "{code}");
    }

    let (values, ie, de, pe) =
        independent_sqrtps([0x8000_0001, 0x0000_0001, 0x4000_0000, 0x4080_0000], 0x1f80);
    assert!(ie && de && pe);
    assert_eq!(values[0], 0xffc0_0000);
    assert_eq!(values[2], 0x3fb5_04f3);
    assert_eq!(values[3], 0x4000_0000);
    let (_, negative_ie, negative_de, _) = independent_sqrtps([0x8000_0001, 0, 0, 0], 0x1f80);
    assert!(negative_ie);
    assert!(!negative_de);
    let (_, snan_ie, subnormal_de, mixed_pe) =
        independent_sqrtps([0x7f80_0001, 0x0000_0001, 0x4000_0000, 0x4080_0000], 0x1f80);
    assert!(snan_ie && subnormal_de && mixed_pe);
    let (special_values, special_ie, special_de, special_pe) =
        independent_sqrtps([0x8000_0000, 0xffc0_1234, 0xff80_0000, 0x7f80_0000], 0x1f80);
    assert_eq!(
        special_values,
        [0x8000_0000, 0xffc0_1234, 0xffc0_0000, 0x7f80_0000]
    );
    assert!(special_ie);
    assert!(!special_de && !special_pe);
    let (snan_values, signaling_ie, _, _) = independent_sqrtps([0x7f80_0001, 0, 0, 0], 0x1f80);
    assert!(signaling_ie);
    assert_eq!(snan_values[0], 0x7fc0_0001);
    for (rc, expected) in [
        (0u32, 0x3fb5_04f3),
        (1, 0x3fb5_04f3),
        (2, 0x3fb5_04f4),
        (3, 0x3fb5_04f3),
    ] {
        assert_eq!(
            independent_sqrtps([0x4000_0000, 0, 0, 0], 0x1f80 | (rc << 13)).0[0],
            expected
        );
    }
}

#[test]
fn sqrtpd_parser_and_independent_binary64_oracle_cover_pairs_and_aggregate_events() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let mut bytes = vec![0x66];
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0x51, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let code = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
            assert_eq!(
                sqrtpd_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in ["660f51c1", "66410f51c1", "66440f51c1", "66450f51c1"] {
        assert!(sqrtpd_register_encoding(code).is_some(), "{code}");
    }
    for code in [
        "0f51c1",
        "f30f51c1",
        "f2660f51c1",
        "66660f51c1",
        "66400f51c1",
        "66480f51c1",
        "66420f51c1",
        "660f5100",
        "660f51c190",
        "c5fd51c1",
        "62f1fd0851c1",
    ] {
        assert_eq!(sqrtpd_register_encoding(code), None, "{code}");
    }
    let (minimum, _, de, pe) = independent_sqrtpd_lane(1, 0x1f80);
    assert_eq!(minimum, 0x1e6000_0000_0000_00);
    assert!(
        de && !pe,
        "sqrt(min subnormal) is exact; DE remains a pre-event"
    );
    let (_, _, de, pe) = independent_sqrtpd_lane(2, 0x1f80);
    assert!(de && pe);
    let (values, ie, de, pe) = independent_sqrtpd([0x7ff0_0000_0000_0001, 2], 0x1f80);
    assert!(ie && de && pe);
    assert_eq!(values[0], 0x7ff8_0000_0000_0001);
    let (values, ie, de, pe) =
        independent_sqrtpd([0xfff8_1234_5678_9abc, 0x4010_0000_0000_0000], 0x1f80);
    assert!(!ie && !de && !pe);
    assert_eq!(values, [0xfff8_1234_5678_9abc, 0x4000_0000_0000_0000]);
    let (values, ie, de, pe) = independent_sqrtpd([0x8000_0000_0000_0001, 0], 0x1f80);
    assert!(ie && !de && !pe);
    assert_eq!(values[0], 0xfff8_0000_0000_0000);
    for (rc, expected) in [
        (0, 0x3ff6_a09e_667f_3bcd),
        (1, 0x3ff6_a09e_667f_3bcc),
        (2, 0x3ff6_a09e_667f_3bcd),
        (3, 0x3ff6_a09e_667f_3bcc),
    ] {
        let (v, _, _, pe) = independent_sqrtpd_lane(0x4000_0000_0000_0000, 0x1f80 | (rc << 13));
        assert_eq!(v, expected);
        assert!(pe);
    }
}

#[test]
fn subpd_parser_and_independent_integer_oracle_cover_masked_range_semantics() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let mut bytes = vec![0x66];
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0x5c, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let code = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
            assert_eq!(
                subpd_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in [
        "0f5cc1",
        "f30f5cc1",
        "f2660f5cc1",
        "66660f5cc1",
        "66400f5cc1",
        "66480f5cc1",
        "66420f5cc1",
        "660f5c00",
        "660f5cc190",
        "c5fd5cc1",
        "62f1fd085cc1",
    ] {
        assert_eq!(subpd_register_encoding(code), None, "{code}");
    }

    let (normal, ie, de, oe, ue, pe) = independent_subpd(
        [0x4008_0000_0000_0000, 0x3ff0_0000_0000_0000],
        [0x3ff0_0000_0000_0000, 0x3c90_0000_0000_0000],
        0x1f80,
    );
    assert_eq!(normal, [0x4000_0000_0000_0000, 0x3ff0_0000_0000_0000]);
    assert!(!ie && !de && !oe && !ue && pe);

    for (rc, expected) in [
        (0, 0x3ff0_0000_0000_0000),
        (1, 0x3fefffffffffffff),
        (2, 0x3ff0_0000_0000_0000),
        (3, 0x3fefffffffffffff),
    ] {
        let (value, tiny, pe, oe) =
            independent_subpd_finite_lane(0x3ff0_0000_0000_0000, 0x3c90_0000_0000_0000, rc);
        assert_eq!(value, expected);
        assert!(!tiny && pe && !oe);
    }

    let (canceled_down, _, _, _, _, _) = independent_subpd(
        [0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000],
        [0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000],
        0x1f80 | (1 << 13),
    );
    assert_eq!(canceled_down, [1 << 63, 1 << 63]);

    let (overflow, ie, de, oe, ue, pe) = independent_subpd(
        [0x7fef_ffff_ffff_ffff, 0],
        [0xffef_ffff_ffff_ffff, 0],
        0x1f80,
    );
    assert_eq!(overflow[0], 0x7ff0_0000_0000_0000);
    let (_, _, exact_significand_inexact, exact_overflow) =
        independent_subpd_finite_lane(0x7fef_ffff_ffff_ffff, 0xffef_ffff_ffff_ffff, 0);
    assert!(exact_overflow && !exact_significand_inexact);
    assert!(!ie && !de && oe && !ue && pe, "masked OM=1 records OE|PE");

    let (tiny_plain, _, _, _, tiny_ue, tiny_pe) = independent_subpd([1, 0], [0, 0], 0x1f80);
    assert_eq!(tiny_plain[0], 1);
    assert!(!tiny_ue && !tiny_pe);
    let (tiny_ftz, _, _, _, tiny_ue, tiny_pe) =
        independent_subpd([1, 0], [0, 0], 0x1f80 | (1 << 15));
    assert_eq!(tiny_ftz[0], 0);
    assert!(tiny_ue && tiny_pe);

    let (nan_lanes, ie, de, _, _, _) =
        independent_subpd([0x7ff8_0000_0000_1234, 1], [1, 0], 0x1f80);
    assert_eq!(nan_lanes[0], 0x7ff8_0000_0000_1234);
    assert!(!ie && de, "QNaN suppresses same-lane DE only");
    let (_, ie, de, _, _, _) = independent_subpd(
        [0x7ff8_0000_0000_1234, 0x3ff0_0000_0000_0000],
        [0x7ff0_0000_0000_0001, 0x3c90_0000_0000_0000],
        0x1f80 & !(1 << 7),
    );
    assert!(ie && !de, "unmasked IE suppresses other-lane post PE");
    let (_, ie, de, _, _, pe) = independent_subpd(
        [0x7ff0_0000_0000_0000, 0x3ff0_0000_0000_0000],
        [0x7ff0_0000_0000_0000, 0x3c90_0000_0000_0000],
        0x1f80 & !(1 << 7),
    );
    assert!(ie && !de && !pe, "invalid pre-event suppresses PE");
}

#[test]
fn subps_parser_and_independent_raw_u32_oracle_cover_four_lanes() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let mut bytes = Vec::new();
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0x5c, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let code = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
            assert_eq!(
                subps_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in [
        "660f5cc1",
        "f30f5cc1",
        "f20f5cc1",
        "400f5cc1",
        "480f5cc1",
        "0f5c",
        "0f5c01",
        "0f5cc190",
        "c5fc5cc1",
        "62f17c085cc1",
    ] {
        assert_eq!(subps_register_encoding(code), None, "{code}");
    }
    let (values, ie, de, oe, ue, pe) = independent_subps(
        [0x40c0_0000, 0x3f80_0000, 0xc040_0000, 0],
        [0x3f80_0000, 0x4000_0000, 0x4040_0000, 0],
        0x1f80,
    );
    assert_eq!(values, [0x40a0_0000, 0xbf80_0000, 0xc0c0_0000, 0]);
    assert!(!ie && !de && !oe && !ue && !pe);
    for (rc, expected) in [
        (0, 0x3f80_0000),
        (1, 0x3f7f_ffff),
        (2, 0x3f80_0000),
        (3, 0x3f7f_ffff),
    ] {
        let (value, tiny, pe, oe) = independent_subps_finite_lane(0x3f80_0000, 0x3300_0000, rc);
        assert_eq!(value, expected);
        assert!(!tiny && pe && !oe);
    }
    let (overflow, ie, de, oe, ue, pe) =
        independent_subps([0x7f7f_ffff, 0, 0, 0], [0xff7f_ffff, 0, 0, 0], 0x1f80);
    assert_eq!(overflow[0], 0x7f80_0000);
    assert!(!ie && !de && oe && !ue && pe);
    let (tiny, _, _, _, ue, pe) = independent_subps([1, 0, 0, 0], [0; 4], 0x1f80 | (1 << 15));
    assert_eq!(tiny[0], 0);
    assert!(ue && pe);
    let (nan, ie, de, _, _, _) =
        independent_subps([0x7fc0_1234, 1, 0, 0], [0x3f80_0000, 0, 0, 0], 0x1f80);
    assert_eq!(nan[0], 0x7fc0_1234);
    assert!(!ie && de);
    let (_, ie, de, _, _, pe) = independent_subps(
        [0x7f80_0001, 0x3f80_0000, 0x7f7f_ffff, 1],
        [0, 0, 0xff7f_ffff, 0],
        0x1f80,
    );
    assert!(ie && de && pe, "masked lane statuses aggregate");
    let (_, ie, de, oe, ue, pe) = independent_subps(
        [0x7f80_0001, 1, 0x7f7f_ffff, 1],
        [0x3f80_0000, 0, 0xff7f_ffff, 0],
        0x1f80 & !(1 << 7),
    );
    assert!(
        ie && de && !oe && !ue && !pe,
        "unmasked IE suppresses post events across packed lanes"
    );
}

#[test]
fn subss_parser_and_independent_raw_u32_oracle_cover_scalar_events() {
    for destination in 0..16u8 {
        for source in 0..16u8 {
            let rex = 0x40 | ((destination >> 1) & 4) | (source >> 3);
            let modrm = 0xc0 | ((destination & 7) << 3) | (source & 7);
            let mut code = vec![0xf3];
            if destination >= 8 || source >= 8 {
                code.push(rex);
            }
            code.extend([0x0f, 0x5c, modrm]);
            let code_hex = code
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                subss_register_encoding(&code_hex),
                Some((destination, source, code.len() as u32))
            );
        }
    }
    for malformed in [
        "f30f5c01",
        "f3660f5cc1",
        "66f30f5cc1",
        "f3400f5cc1",
        "f3420f5cc1",
        "f3480f5cc1",
        "f30f5cc1ff",
        "c5fa5cc1",
        "62f17e085cc1",
    ] {
        assert_eq!(subss_register_encoding(malformed), None, "{malformed}");
    }
    for (rc, expected) in [
        (0, 0x3f80_0000),
        (2, 0x3f80_0000),
        (1, 0x3f7f_ffff),
        (3, 0x3f7f_ffff),
    ] {
        let (value, ie, de, oe, ue, pe) =
            independent_subss(0x3f80_0000, 0x3300_0000, 0x1f80 | (rc << 13));
        assert_eq!(value, expected);
        assert!(!ie && !de && !oe && !ue && pe);
    }
    let (nan, ie, de, _, _, _) = independent_subss(0x7fc0_1234, 0x7f80_0001, 0x1f80);
    assert_eq!(nan, 0x7fc0_1234);
    assert!(ie && !de);
    let (rhs_snan, ie, de, _, _, _) = independent_subss(0x3f80_0000, 0x7f80_0001, 0x1f80);
    assert_eq!(rhs_snan, 0x7fc0_0001, "selected RHS SNaN is quieted");
    assert!(ie && !de);
    let (lhs_snan, ie, de, _, _, _) = independent_subss(0xff80_0123, 0x7fc0_0456, 0x1f80);
    assert_eq!(lhs_snan, 0xffc0_0123, "selected LHS SNaN is quieted");
    assert!(ie && !de);
    let (both_snan, ie, de, _, _, _) = independent_subss(0xff80_0123, 0x7f80_0456, 0x1f80);
    assert_eq!(both_snan, 0xffc0_0123, "first original SNaN is selected");
    assert!(ie && !de);
    let (indefinite, ie, _, _, _, _) = independent_subss(0x7f80_0000, 0x7f80_0000, 0x1f80);
    assert_eq!(indefinite, 0xffc0_0000);
    assert!(ie);
    let (_, _, de, _, _, _) = independent_subss(1, 0, 0x1f80);
    assert!(de);
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/x86/stage2/s2_37aq_subss_xmm0_xmm1.case.json");
    let fixture = Fixture::from_json(&std::fs::read(fixture_path).unwrap())
        .unwrap()
        .fixture;
    for mxcsr in [0x1b80, 0x1780, 0x1fc0, 0x11f80] {
        let mut invalid = fixture.clone();
        invalid.initial_state.sse.as_mut().unwrap().mxcsr = HexU64(mxcsr);
        assert!(invalid.validate().is_err(), "MXCSR={mxcsr:#x}");
    }
}

#[test]
fn sqrtsd_parser_and_scalar_raw_u64_oracle_cover_only_low_lane_semantics() {
    for destination in 0u8..16 {
        for source in 0u8..16 {
            let mut bytes = vec![0xf2];
            if destination >= 8 || source >= 8 {
                bytes.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            bytes.extend([0x0f, 0x51, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let code = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                sqrtsd_register_encoding(&code),
                Some((destination, source, bytes.len() as u32)),
                "{code}"
            );
        }
    }
    for code in ["f20f51c1", "f2410f51c1", "f2440f51c1", "f2450f51c1"] {
        assert!(sqrtsd_register_encoding(code).is_some(), "{code}");
    }
    for code in [
        "660f51c1",
        "f30f51c1",
        "f2660f51c1",
        "66f20f51c1",
        "f2f20f51c1",
        "f2400f51c1",
        "f2480f51c1",
        "f2420f51c1",
        "40f20f51c1",
        "f20f5100",
        "f20f51c190",
        "c5fb51c1",
        "62f1ff0851c1",
    ] {
        assert_eq!(sqrtsd_register_encoding(code), None, "{code}");
    }
    let (minimum, ie, de, pe) = independent_sqrtpd_lane(1, 0x1f80);
    assert_eq!(minimum, 0x1e60_0000_0000_0000);
    assert!(
        !ie && de && !pe,
        "sqrt(min subnormal) is exact with DE only"
    );
    for raw in [2u64, 3] {
        let (_, ie, de, pe) = independent_sqrtpd_lane(raw, 0x1f80);
        assert!(!ie && de && pe, "{raw:#x} has DE and PE");
    }
    assert_eq!(
        independent_sqrtpd_lane(0x8000_0000_0000_0001, 0x1f80),
        (0xfff8_0000_0000_0000, true, false, false)
    );
    assert_eq!(
        independent_sqrtpd_lane(0xfff8_1234_5678_9abc, 0x1f80),
        (0xfff8_1234_5678_9abc, false, false, false)
    );
    assert_eq!(
        independent_sqrtpd_lane(0x7ff0_0000_0000_1234, 0x1f80),
        (0x7ff8_0000_0000_1234, true, false, false)
    );
    assert_eq!(
        independent_sqrtpd_lane(0x8000_0000_0000_0000, 0x1f80).0,
        0x8000_0000_0000_0000
    );
    for (rc, expected) in [
        (0, 0x3ff6_a09e_667f_3bcd),
        (1, 0x3ff6_a09e_667f_3bcc),
        (2, 0x3ff6_a09e_667f_3bcd),
        (3, 0x3ff6_a09e_667f_3bcc),
    ] {
        assert_eq!(
            independent_sqrtpd_lane(0x4000_0000_0000_0000, 0x1f80 | (rc << 13)).0,
            expected
        );
    }
}

#[test]
fn cvtss2si_parser_covers_all_512_pairs_and_rejects_noncanonical_forms() {
    for wide in [false, true] {
        for destination in 0..16u8 {
            for source in 0..16u8 {
                let mut bytes = vec![0xf3];
                let rex = match (wide, destination >= 8, source >= 8) {
                    (false, false, false) => None,
                    (false, true, false) => Some(0x44),
                    (false, false, true) => Some(0x41),
                    (false, true, true) => Some(0x45),
                    (true, false, false) => Some(0x48),
                    (true, false, true) => Some(0x49),
                    (true, true, false) => Some(0x4c),
                    (true, true, true) => Some(0x4d),
                };
                if let Some(rex) = rex {
                    bytes.push(rex);
                }
                bytes.extend_from_slice(&[
                    0x0f,
                    0x2d,
                    0xc0 | ((destination & 7) << 3) | (source & 7),
                ]);
                let text = bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                assert_eq!(
                    cvtss2si_register_encoding(&text),
                    Some((wide, destination, source, bytes.len() as u32))
                );
            }
        }
    }
    for bytes in [
        "f3400f2dc0",
        "f3420f2dc0",
        "f30f2d00",
        "f3660f2dc0",
        "f20f2dc0",
        "f30f2dc0c0",
    ] {
        assert_eq!(cvtss2si_register_encoding(bytes), None, "{bytes}");
    }
}

#[test]
fn cvttss2si_parser_covers_all_512_pairs_and_rejects_noncanonical_forms() {
    for wide in [false, true] {
        for destination in 0..16u8 {
            for source in 0..16u8 {
                let mut bytes = vec![0xf3];
                let rex = match (wide, destination >= 8, source >= 8) {
                    (false, false, false) => None,
                    (false, true, false) => Some(0x44),
                    (false, false, true) => Some(0x41),
                    (false, true, true) => Some(0x45),
                    (true, false, false) => Some(0x48),
                    (true, false, true) => Some(0x49),
                    (true, true, false) => Some(0x4c),
                    (true, true, true) => Some(0x4d),
                };
                if let Some(rex) = rex {
                    bytes.push(rex);
                }
                bytes.extend_from_slice(&[
                    0x0f,
                    0x2c,
                    0xc0 | ((destination & 7) << 3) | (source & 7),
                ]);
                let text = bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                assert_eq!(
                    cvttss2si_register_encoding(&text),
                    Some((wide, destination, source, bytes.len() as u32))
                );
            }
        }
    }
    for bytes in [
        "f3400f2cc0",
        "f3420f2cc0",
        "f30f2c00",
        "f3660f2cc0",
        "f20f2cc0",
        "f30f2cc0c0",
        "f30f2dc0",
    ] {
        assert_eq!(cvttss2si_register_encoding(bytes), None, "{bytes}");
    }
}

#[test]
fn cvttss2si_oracle_always_truncates_toward_zero() {
    for (bits, expected) in [(0x3fc0_0000, 1), (0xbfc0_0000, u64::MAX)] {
        for rc in 0..4 {
            assert_eq!(
                independent_binary32_to_integer(bits, 3, false),
                (expected, false, true),
                "bits={bits:08x} RC={rc}"
            );
        }
    }
    assert_eq!(
        independent_binary32_to_integer(0xcf00_0000, 3, false),
        (0xffff_ffff_8000_0000, false, false)
    );
    assert_eq!(
        independent_binary32_to_integer(0x4f00_0000, 3, false),
        (0x8000_0000, true, false)
    );
}

#[test]
fn cvttss2si_r64_memory_oracle_covers_raw_integer_anchors() {
    let cases = [
        (0x3fc0_0000, 1u64, false, true),
        (0xbfc0_0000, u64::MAX, false, true),
        (0x4020_0000, 2, false, true),
        (0xc020_0000, u64::MAX - 1, false, true),
        (0x0000_0001, 0, false, true),
        (0x8000_0001, 0, false, true),
        (0x5eff_ffff, 0x7fff_ff80_0000_0000, false, false),
        (0x5f00_0000, 0x8000_0000_0000_0000, true, false),
        (0xdeff_ffff, 0x8000_0080_0000_0000, false, false),
        (0xdf00_0000, 0x8000_0000_0000_0000, false, false),
        (0xdf00_0001, 0x8000_0000_0000_0000, true, false),
    ];
    for (raw, expected_bits, expected_invalid, expected_inexact) in cases {
        let (bits, invalid, inexact) = independent_binary32_to_integer(raw, 3, true);
        assert_eq!(bits, expected_bits, "raw={raw:08x}");
        assert_eq!(invalid, expected_invalid, "raw={raw:08x}");
        assert_eq!(inexact, expected_inexact, "raw={raw:08x}");
    }
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37cm_cvttss2si_m32_i64_truncation.case.json"
    ))
    .unwrap();
    assert_eq!(
        fixture.fixture.profile,
        Profile::LongCvttss2siR64MemoryRaxV1
    );
    let mut foreign = fixture.fixture;
    foreign.cvttss2si_r32_memory_controls = Some(Cvttss2siR32MemoryControlSnapshot {
        sse_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
    });
    assert!(foreign.validate().is_err());
    assert!(Fixture::from_json(&serde_json::to_vec(&foreign).unwrap()).is_err());
}

#[test]
fn divpd_independent_integer_oracle_covers_rounding_and_exception_boundaries() {
    use super::independent_divpd_lane;

    let one = 0x3ff0_0000_0000_0000;
    let three = 0x4008_0000_0000_0000;
    for (rc, expected) in [
        (0, 0x3fd5_5555_5555_5555),
        (1, 0x3fd5_5555_5555_5555),
        (2, 0x3fd5_5555_5555_5556),
        (3, 0x3fd5_5555_5555_5555),
    ] {
        let lane = independent_divpd_lane(one, three, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!(
            (lane.1, lane.2, lane.3, lane.4, lane.5, lane.6),
            (false, false, false, false, false, true)
        );
    }

    let min_normal = 0x0010_0000_0000_0000;
    let min_subnormal = 1;
    let two_to_53 = 0x4340_0000_0000_0000;
    for rc in [0, 1, 3] {
        let lane = independent_divpd_lane(min_normal, two_to_53, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, 0);
        assert_eq!(
            (lane.1, lane.2, lane.3, lane.4, lane.5, lane.6),
            (false, false, false, false, true, true)
        );
    }
    let round_up = independent_divpd_lane(min_normal, two_to_53, 0x1f80 | (2 << 13));
    assert_eq!(round_up.0, min_subnormal);
    assert_eq!((round_up.5, round_up.6), (true, true));
    let ftz = independent_divpd_lane(min_normal, two_to_53, 0x9f80);
    assert_eq!(ftz.0, 0);
    assert_eq!((ftz.5, ftz.6), (true, true));

    let max_subnormal = min_normal - 1;
    let half_max_subnormal = independent_divpd_lane(max_subnormal, 0x4000_0000_0000_0000, 0x1f80);
    assert_eq!(half_max_subnormal.0, 0x0008_0000_0000_0000);
    assert!(half_max_subnormal.5 && half_max_subnormal.6);
    for (rc, expected) in [
        (0, max_subnormal),
        (1, max_subnormal),
        (2, min_normal),
        (3, max_subnormal),
    ] {
        let lane = independent_divpd_lane(min_normal, 0x3ff0_0000_0000_0001, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert!(lane.5 && lane.6);
    }
    let exact_subnormal = independent_divpd_lane(min_normal, 0x4000_0000_0000_0000, 0x1f80);
    assert_eq!(exact_subnormal.0, min_normal >> 1);
    assert_eq!((exact_subnormal.5, exact_subnormal.6), (false, false));
    let flushed_exact = independent_divpd_lane(min_normal, 0x4000_0000_0000_0000, 0x9f80);
    assert_eq!(flushed_exact.0, 0);
    assert_eq!((flushed_exact.5, flushed_exact.6), (true, true));

    let subnormal_over_zero = independent_divpd_lane(1, 0, 0x1f80 & !(1 << 8));
    assert_eq!(
        (
            subnormal_over_zero.1,
            subnormal_over_zero.2,
            subnormal_over_zero.3
        ),
        (false, false, true)
    );
    let invalid = independent_divpd_lane(0, 0, 0x1f80);
    assert!(invalid.1);
    assert!(!invalid.3);
    let inf_over_zero = independent_divpd_lane(0x7ff0_0000_0000_0000, 0, 0x1f80);
    assert_eq!(inf_over_zero.3, false);
}

#[test]
fn divsd_independent_integer_oracle_covers_scalar_boundaries() {
    use super::independent_divsd;

    let one = 0x3ff0_0000_0000_0000;
    let three = 0x4008_0000_0000_0000;
    for (rc, expected) in [
        (0, 0x3fd5_5555_5555_5555),
        (1, 0x3fd5_5555_5555_5555),
        (2, 0x3fd5_5555_5555_5556),
        (3, 0x3fd5_5555_5555_5555),
    ] {
        let result = independent_divsd(one, three, 0x1f80 | (rc << 13));
        assert_eq!(result.0, expected);
        assert_eq!(
            (result.1, result.2, result.3, result.4, result.5, result.6),
            (false, false, false, false, false, true)
        );
    }

    let min_normal = 0x0010_0000_0000_0000;
    let min_subnormal = 1;
    let half_midpoint = independent_divsd(0x001f_ffff_ffff_ffff, 0x4000_0000_0000_0000, 0x1f80);
    assert_eq!(half_midpoint.0, min_normal);
    assert_eq!((half_midpoint.5, half_midpoint.6), (true, true));
    for (rc, expected) in [
        (0, min_normal),
        (1, 0x000f_ffff_ffff_ffff),
        (2, min_normal),
        (3, 0x000f_ffff_ffff_ffff),
    ] {
        let rounded = independent_divsd(
            0x001f_ffff_ffff_ffff,
            0x4000_0000_0000_0000,
            0x1f80 | (rc << 13),
        );
        assert_eq!(rounded.0, expected);
        assert_eq!((rounded.5, rounded.6), (true, true));
    }
    for (rc, expected) in [
        (0, 0x000f_ffff_ffff_ffff),
        (1, 0x000f_ffff_ffff_ffff),
        (2, min_normal),
        (3, 0x000f_ffff_ffff_ffff),
    ] {
        let rounded = independent_divsd(min_normal, 0x3ff0_0000_0000_0001, 0x1f80 | (rc << 13));
        assert_eq!(rounded.0, expected);
        assert_eq!((rounded.5, rounded.6), (true, true));
    }
    let exact_tiny = independent_divsd(min_normal, 0x4000_0000_0000_0000, 0x1f80);
    assert_eq!(exact_tiny.0, 0x0008_0000_0000_0000);
    assert_eq!((exact_tiny.5, exact_tiny.6), (false, false));
    let flushed = independent_divsd(min_normal, 0x4000_0000_0000_0000, 0x9f80);
    assert_eq!(flushed.0, 0);
    assert_eq!((flushed.5, flushed.6), (true, true));
    let half_min_subnormal = independent_divsd(min_normal, 0x4340_0000_0000_0000, 0x1f80);
    assert_eq!(half_min_subnormal.0, 0);
    assert_eq!((half_min_subnormal.5, half_min_subnormal.6), (true, true));
    let round_up = independent_divsd(min_normal, 0x4340_0000_0000_0000, 0x5f80);
    assert_eq!(round_up.0, min_subnormal);
    let negative_up = independent_divsd(one | (1 << 63), three, 0x5f80);
    assert_eq!(negative_up.0, 0xbfd5_5555_5555_5555);
    let negative_down = independent_divsd(one | (1 << 63), three, 0x3f80);
    assert_eq!(negative_down.0, 0xbfd5_5555_5555_5556);

    let subnormal_over_zero = independent_divsd(1, 0, 0x1f80 & !(1 << 8));
    assert_eq!(
        (
            subnormal_over_zero.1,
            subnormal_over_zero.2,
            subnormal_over_zero.3
        ),
        (false, false, true)
    );
    let infinity_over_zero = independent_divsd(0x7ff0_0000_0000_0000, 0, 0x1f80 & !(1 << 9));
    assert_eq!(infinity_over_zero.3, false);
}

#[test]
fn divps_independent_integer_oracle_covers_rounding_and_exception_boundaries() {
    use super::independent_divps_lane;

    for (rc, expected) in [
        (0, 0x3eaa_aaab),
        (1, 0x3eaa_aaaa),
        (2, 0x3eaa_aaab),
        (3, 0x3eaa_aaaa),
    ] {
        let lane = independent_divps_lane(0x3f80_0000, 0x4040_0000, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert!(lane.6);
    }

    for (rc, expected) in [(0, 0), (1, 0), (3, 0), (2, 1)] {
        let lane = independent_divps_lane(0x0080_0000, 0x4b80_0000, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!((lane.2, lane.5, lane.6), (false, true, true));
    }
    for (rc, expected) in [
        (0, 0x0080_0000),
        (1, 0x007f_ffff),
        (2, 0x0080_0000),
        (3, 0x007f_ffff),
    ] {
        let lane = independent_divps_lane(0x00ff_ffff, 0x4000_0000, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!((lane.5, lane.6), (true, true));
    }
    for (rc, expected) in [
        (0, 0x007f_ffff),
        (1, 0x007f_ffff),
        (2, 0x0080_0000),
        (3, 0x007f_ffff),
    ] {
        let lane = independent_divps_lane(0x0080_0000, 0x3f80_0001, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!((lane.5, lane.6), (true, true));
    }
    let exact_tiny = independent_divps_lane(0x0080_0000, 0x4000_0000, 0x1f80);
    assert_eq!(exact_tiny.0, 0x0040_0000);
    assert_eq!(
        (exact_tiny.2, exact_tiny.5, exact_tiny.6),
        (false, false, false)
    );
    let ftz_exact_tiny = independent_divps_lane(0x0080_0000, 0x4000_0000, 0x9f80);
    assert_eq!(ftz_exact_tiny.0, 0);
    assert_eq!((ftz_exact_tiny.5, ftz_exact_tiny.6), (true, true));
    let ftz_half_min_subnormal =
        independent_divps_lane(0x0080_0000, 0x4b80_0000, 0x9f80 | (2 << 13));
    assert_eq!(ftz_half_min_subnormal.0, 0);
    assert_eq!(
        (ftz_half_min_subnormal.5, ftz_half_min_subnormal.6),
        (true, true)
    );
    for (rc, expected) in [
        (0, 0x7f80_0000),
        (1, 0x7f7f_ffff),
        (2, 0x7f80_0000),
        (3, 0x7f7f_ffff),
    ] {
        let overflow = independent_divps_lane(0x7f7f_ffff, 0x0080_0000, 0x1f80 | (rc << 13));
        assert_eq!(overflow.0, expected);
        assert_eq!((overflow.4, overflow.6), (true, true));
    }

    let huge_shift_up = independent_divps_lane(1, 0x7f7f_ffff, 0x1f80 | (2 << 13));
    assert_eq!(huge_shift_up.0, 1);
    assert_eq!(
        (huge_shift_up.2, huge_shift_up.5, huge_shift_up.6),
        (true, true, true)
    );
    for (lhs, rhs, expected_flags) in [
        (0, 0, (true, false, false)),
        (0x3f80_0000, 0, (false, false, true)),
        (1, 0, (false, false, true)),
    ] {
        let lane = independent_divps_lane(lhs, rhs, 0x1f80);
        assert_eq!((lane.1, lane.2, lane.3), expected_flags);
    }

    // All pre-rounding exceptions are accumulated across four independent
    // lanes before arithmetic; a first-lane invalid suppresses later PE.
    let (values, ie, de, ze, oe, ue, pe) = super::independent_divps(
        [0, 0x3f80_0000, 1, 0x3f80_0000],
        [0, 0, 0x3f80_0000, 0x4040_0000],
        0x1f80 & !(1 << 7) & !(1 << 8) & !(1 << 9),
    );
    assert_eq!(
        (ie, de, ze, oe, ue, pe),
        (true, true, true, false, false, false)
    );
    assert_eq!(values[3], 0x3eaa_aaab);
    let (_, ie, de, ze, _, _, _) = super::independent_divps(
        [0, 0x3f80_0000, 1, 0x3f80_0000],
        [0, 0, 0x3f80_0000, 0x4040_0000],
        0x1f80,
    );
    assert_eq!((ie, de, ze), (true, true, true));
}

#[test]
fn divss_independent_scalar_oracle_covers_rounding_events_and_scalar_bounds() {
    use super::independent_divss;

    for (rc, expected) in [
        (0, 0x3eaa_aaab),
        (1, 0x3eaa_aaaa),
        (2, 0x3eaa_aaab),
        (3, 0x3eaa_aaaa),
    ] {
        let lane = independent_divss(0x3f80_0000, 0x4040_0000, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!(
            (lane.1, lane.2, lane.3, lane.6),
            (false, false, false, true)
        );
    }
    for (rc, expected) in [(0, 0), (1, 0), (2, 1), (3, 0)] {
        let lane = independent_divss(0x0080_0000, 0x4b80_0000, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!((lane.2, lane.5, lane.6), (false, true, true));
    }
    for (rc, expected) in [
        (0, 0x0080_0000),
        (1, 0x007f_ffff),
        (2, 0x0080_0000),
        (3, 0x007f_ffff),
    ] {
        let lane = independent_divss(0x00ff_ffff, 0x4000_0000, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!((lane.5, lane.6), (true, true));
    }
    for (rc, expected) in [
        (0, 0x007f_ffff),
        (1, 0x007f_ffff),
        (2, 0x0080_0000),
        (3, 0x007f_ffff),
    ] {
        let lane = independent_divss(0x0080_0000, 0x3f80_0001, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, expected);
        assert_eq!((lane.5, lane.6), (true, true));
    }
    let exact_tiny = independent_divss(0x0080_0000, 0x4000_0000, 0x1f80);
    assert_eq!(exact_tiny.0, 0x0040_0000);
    assert_eq!(
        (exact_tiny.2, exact_tiny.5, exact_tiny.6),
        (false, false, false)
    );
    let ftz_tiny = independent_divss(0x0080_0000, 0x4000_0000, 0x9f80);
    assert_eq!(ftz_tiny.0, 0);
    assert_eq!((ftz_tiny.5, ftz_tiny.6), (true, true));
    for rc in 0..4 {
        let lane = independent_divss(0x8000_0000, 0x4040_0000, 0x1f80 | (rc << 13));
        assert_eq!(lane.0, 0x8000_0000);
        assert!(!lane.1 && !lane.2 && !lane.3);
    }
    let huge_shift = independent_divss(1, 0x7f7f_ffff, 0x1f80 | (2 << 13));
    assert_eq!(huge_shift.0, 1);
    assert_eq!(
        (huge_shift.2, huge_shift.5, huge_shift.6),
        (true, true, true)
    );
    let unmasked_de = independent_divss(1, 0x7f7f_ffff, 0x1e80);
    assert_eq!(
        (
            unmasked_de.1,
            unmasked_de.2,
            unmasked_de.4,
            unmasked_de.5,
            unmasked_de.6
        ),
        (false, true, false, false, false)
    );

    let snan = independent_divss(0x7f80_0001, 0x3f80_0000, 0x1f80);
    assert_eq!(
        (snan.0, snan.1, snan.2, snan.3),
        (0x7fc0_0001, true, false, false)
    );
    let zero_over_zero = independent_divss(0x8000_0000, 0, 0x1f80);
    assert_eq!(
        (
            zero_over_zero.0,
            zero_over_zero.1,
            zero_over_zero.2,
            zero_over_zero.3
        ),
        (0xffc0_0000, true, false, false)
    );
    let subnormal_over_zero = independent_divss(1, 0, 0x1f80);
    assert_eq!(
        (
            subnormal_over_zero.1,
            subnormal_over_zero.2,
            subnormal_over_zero.3
        ),
        (false, false, true)
    );
    let infinity_over_zero = independent_divss(0x7f80_0000, 0, 0x1f80);
    assert_eq!(
        (
            infinity_over_zero.0,
            infinity_over_zero.2,
            infinity_over_zero.3
        ),
        (0x7f80_0000, false, false)
    );
    let infinity_over_subnormal = independent_divss(0x7f80_0000, 1, 0x1f80);
    assert_eq!(
        (infinity_over_subnormal.2, infinity_over_subnormal.3),
        (true, false)
    );
}

#[test]
fn addpd_oracle_covers_rounding_tiny_overflow_and_special_events() {
    let nearest = independent_addpd(
        [0x3ff0_0000_0000_0000, 0],
        [0x3ca0_0000_0000_0000, 0],
        0x1f80,
    );
    assert_eq!(nearest.0[0], 0x3ff0_0000_0000_0000);
    assert!(nearest.5);
    let upward = independent_addpd(
        [0x3ff0_0000_0000_0000, 0],
        [0x3ca0_0000_0000_0000, 0],
        0x5f80,
    );
    assert_eq!(upward.0[0], 0x3ff0_0000_0000_0001);
    assert!(upward.5);
    let negative_halfway = independent_addpd(
        [0x3ff0_0000_0000_0000, 0],
        [0xbc90_0000_0000_0000, 0],
        0x3f80,
    );
    assert_eq!(negative_halfway.0[0], 0x3fef_ffff_ffff_ffff);
    assert!(negative_halfway.5);

    let opposite_zeros_nearest = independent_addpd([0, 0], [1 << 63, 0], 0x1f80);
    assert_eq!(opposite_zeros_nearest.0[0], 0);
    let opposite_zeros_down = independent_addpd([0, 0], [1 << 63, 0], 0x3f80);
    assert_eq!(opposite_zeros_down.0[0], 1 << 63);
    let negative_zeros = independent_addpd([1 << 63, 0], [1 << 63, 0], 0x1f80);
    assert_eq!(negative_zeros.0[0], 1 << 63);

    let exact_tiny = independent_addpd(
        [0x0010_0000_0000_0001, 0],
        [0x8010_0000_0000_0000, 0],
        0x1f80,
    );
    assert_eq!(exact_tiny.0[0], 1);
    assert!(!exact_tiny.3 && !exact_tiny.4 && !exact_tiny.5);
    let flushed_tiny = independent_addpd(
        [0x0010_0000_0000_0001, 0],
        [0x8010_0000_0000_0000, 0],
        0x9f80,
    );
    assert_eq!(flushed_tiny.0[0], 0);
    assert!(flushed_tiny.4 && flushed_tiny.5);

    let large_gap_nearest = independent_addpd([0x3ff0_0000_0000_0000, 0], [1, 0], 0x1f80);
    assert_eq!(large_gap_nearest.0[0], 0x3ff0_0000_0000_0000);
    assert!(!large_gap_nearest.1 && large_gap_nearest.2 && large_gap_nearest.5);
    let large_gap_up = independent_addpd([0x3ff0_0000_0000_0000, 0], [1, 0], 0x5f80);
    assert_eq!(large_gap_up.0[0], 0x3ff0_0000_0000_0001);
    assert!(large_gap_up.2 && large_gap_up.5);

    let overflow = independent_addpd(
        [0x7fef_ffff_ffff_ffff, 0],
        [0x7c90_0000_0000_0000, 0],
        0x1f80,
    );
    assert_eq!(overflow.0[0], 0x7ff0_0000_0000_0000);
    assert!(overflow.3 && overflow.5);
    let toward_zero = independent_addpd(
        [0x7fef_ffff_ffff_ffff, 0],
        [0x7c90_0000_0000_0000, 0],
        0x7f80,
    );
    assert_eq!(toward_zero.0[0], 0x7fef_ffff_ffff_ffff);
    assert!(!toward_zero.3 && toward_zero.5);

    // The directed midpoint rounds to maxfinite without overflow, while a
    // much larger exact sum overflows even when the response is maxfinite.
    for (rc, expected) in [
        (0x1f80, 0x7ff0_0000_0000_0000),
        (0x3f80, 0x7fef_ffff_ffff_ffff),
        (0x5f80, 0x7ff0_0000_0000_0000),
        (0x7f80, 0x7fef_ffff_ffff_ffff),
    ] {
        let directed_overflow =
            independent_addpd([0x7fef_ffff_ffff_ffff, 0], [0x7fef_ffff_ffff_ffff, 0], rc);
        assert_eq!(directed_overflow.0[0], expected);
        assert!(directed_overflow.3 && directed_overflow.5);
    }

    let snan = independent_addpd(
        [0x7ff0_0000_0000_0001, 0x7ff0_0000_0000_0000],
        [0x7ff8_0000_0000_0042, 0xfff0_0000_0000_0000],
        0x1f80,
    );
    assert_eq!(snan.0[0], 0x7ff8_0000_0000_0001);
    assert_eq!(snan.0[1], 0xfff8_0000_0000_0000);
    assert!(snan.1);

    let qnan_wins = independent_addpd(
        [0x7ff8_0000_0000_0042, 0],
        [0x7ff0_0000_0000_0001, 0],
        0x1f80,
    );
    assert_eq!(qnan_wins.0[0], 0x7ff8_0000_0000_0042);
    assert!(qnan_wins.1);
    let nan_suppresses_de = independent_addpd([0x7ff8_0000_0000_0042, 0], [1, 0], 0x1f80);
    assert!(!nan_suppresses_de.1 && !nan_suppresses_de.2);

    let infinity_subnormal = independent_addpd([0x7ff0_0000_0000_0000, 0], [1, 0], 0x1f80);
    assert_eq!(infinity_subnormal.0[0], 0x7ff0_0000_0000_0000);
    assert!(!infinity_subnormal.1 && infinity_subnormal.2);
}

#[test]
fn addsd_oracle_covers_scalar_rounding_tiny_overflow_and_special_events() {
    let halfway = independent_addsd(0x3ff0_0000_0000_0000, 0x3ca0_0000_0000_0000, 0x1f80);
    assert_eq!(
        halfway,
        (0x3ff0_0000_0000_0000, false, false, false, false, true)
    );
    let upward = independent_addsd(0x3ff0_0000_0000_0000, 0x3ca0_0000_0000_0000, 0x5f80);
    assert_eq!(
        upward,
        (0x3ff0_0000_0000_0001, false, false, false, false, true)
    );
    let negative_halfway = independent_addsd(0xbff0_0000_0000_0000, 0xbca0_0000_0000_0000, 0x1f80);
    assert_eq!(
        negative_halfway,
        (0xbff0_0000_0000_0000, false, false, false, false, true)
    );
    let negative_down = independent_addsd(0xbff0_0000_0000_0000, 0xbca0_0000_0000_0000, 0x3f80);
    assert_eq!(
        negative_down,
        (0xbff0_0000_0000_0001, false, false, false, false, true)
    );

    let exact_tiny = independent_addsd(0x0010_0000_0000_0001, 0x8010_0000_0000_0000, 0x1f80);
    assert_eq!(exact_tiny, (1, false, false, false, false, false));
    let flushed_tiny = independent_addsd(0x0010_0000_0000_0001, 0x8010_0000_0000_0000, 0x9f80);
    assert_eq!(flushed_tiny, (0, false, false, false, true, true));

    let midpoint = independent_addsd(0x7fef_ffff_ffff_ffff, 0x7c90_0000_0000_0000, 0x1f80);
    assert_eq!(
        midpoint,
        (0x7ff0_0000_0000_0000, false, false, true, false, true)
    );
    let midpoint_down = independent_addsd(0x7fef_ffff_ffff_ffff, 0x7c90_0000_0000_0000, 0x3f80);
    assert_eq!(
        midpoint_down,
        (0x7fef_ffff_ffff_ffff, false, false, false, false, true)
    );
    for rc in [0x1f80, 0x3f80, 0x5f80, 0x7f80] {
        assert_eq!(
            independent_addsd(0x7fef_ffff_ffff_ffff, 0x7fef_ffff_ffff_ffff, rc),
            (
                (if rc == 0x3f80 || rc == 0x7f80 {
                    0x7fef_ffff_ffff_ffff
                } else {
                    0x7ff0_0000_0000_0000
                }),
                false,
                false,
                true,
                false,
                true
            )
        );
    }
    let large_gap_up = independent_addsd(0x3ff0_0000_0000_0000, 1, 0x5f80);
    assert_eq!(
        large_gap_up,
        (0x3ff0_0000_0000_0001, false, true, false, false, true)
    );
    let unmasked_de = independent_addsd(0x3ff0_0000_0000_0000, 1, 0x1e80);
    assert_eq!(
        unmasked_de,
        (0x3ff0_0000_0000_0000, false, true, false, false, false)
    );

    let first_nan = independent_addsd(0x7ff0_0000_0000_0042, 0x7ff8_0000_0000_1234, 0x1f80);
    assert_eq!(
        first_nan,
        (0x7ff8_0000_0000_0042, true, false, false, false, false)
    );
    let opposite_infinities =
        independent_addsd(0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000, 0x1f80);
    assert_eq!(
        opposite_infinities,
        (0xfff8_0000_0000_0000, true, false, false, false, false)
    );
    let infinity_subnormal = independent_addsd(0x7ff0_0000_0000_0000, 1, 0x1f80);
    assert_eq!(
        infinity_subnormal,
        (0x7ff0_0000_0000_0000, false, true, false, false, false)
    );
}

#[test]
fn addps_oracle_covers_binary32_rounding_ftz_overflow_and_special_events() {
    let nearest = independent_addps(
        [0x3f80_0000, 0xbf80_0000, 0x0080_0001, 0x7f7f_ffff],
        [0x3380_0000, 0xb380_0000, 0x8080_0000, 0x7300_0000],
        0x1f80,
    );
    assert_eq!(nearest.0, [0x3f80_0000, 0xbf80_0000, 1, 0x7f80_0000]);
    assert_eq!(
        (nearest.1, nearest.2, nearest.3, nearest.4, nearest.5),
        (false, false, true, false, true)
    );

    let positive = [0x3f80_0000, 0, 0, 0];
    let positive_half = [0x3380_0000, 0, 0, 0];
    for (rc, expected) in [
        (0x0000, 0x3f80_0000),
        (0x2000, 0x3f80_0000),
        (0x4000, 0x3f80_0001),
        (0x6000, 0x3f80_0000),
    ] {
        let outcome = independent_addps(positive, positive_half, 0x1f80 | rc);
        assert_eq!(outcome.0[0], expected, "RC={rc:#x}");
        assert!(outcome.5);
    }
    let negative = independent_addps([0xbf80_0000, 0, 0, 0], [0xb380_0000, 0, 0, 0], 0x3f80);
    assert_eq!(negative.0[0], 0xbf80_0001); // RD
    for rc in [0x1f80, 0x5f80, 0x7f80] {
        assert_eq!(
            independent_addps([0xbf80_0000, 0, 0, 0], [0xb380_0000, 0, 0, 0], rc).0[0],
            0xbf80_0000
        );
    }
    let signed_zero_nearest = independent_addps(
        [0, 0x8000_0000, 0x3f80_0000, 0xbf80_0000],
        [0x8000_0000, 0x8000_0000, 0xbf80_0000, 0x3f80_0000],
        0x1f80,
    );
    assert_eq!(signed_zero_nearest.0, [0, 0x8000_0000, 0, 0]);
    let signed_zero_down = independent_addps(
        [0, 0x8000_0000, 0x3f80_0000, 0xbf80_0000],
        [0x8000_0000, 0x8000_0000, 0xbf80_0000, 0x3f80_0000],
        0x3f80,
    );
    assert_eq!(signed_zero_down.0, [0x8000_0000; 4]);

    let exact_tiny = independent_addps([0x0080_0001, 0, 0, 0], [0x8080_0000, 0, 0, 0], 0x1f80);
    assert_eq!(exact_tiny.0[0], 1);
    assert_eq!(
        (exact_tiny.2, exact_tiny.4, exact_tiny.5),
        (false, false, false)
    );
    let flushed_tiny = independent_addps([0x0080_0001, 0, 0, 0], [0x8080_0000, 0, 0, 0], 0x9f80);
    assert_eq!(flushed_tiny.0[0], 0);
    assert!(flushed_tiny.4 && flushed_tiny.5);

    for (rc, expected, oe, pe) in [
        (0x0000, 0x7f80_0000, true, true),
        (0x2000, 0x7f7f_ffff, false, true),
        (0x4000, 0x7f80_0000, true, true),
        (0x6000, 0x7f7f_ffff, false, true),
    ] {
        let outcome =
            independent_addps([0x7f7f_ffff, 0, 0, 0], [0x7300_0000, 0, 0, 0], 0x1f80 | rc);
        assert_eq!(outcome.0[0], expected);
        assert_eq!((outcome.3, outcome.5), (oe, pe));
    }
    for rc in [0x1f80, 0x3f80, 0x5f80, 0x7f80] {
        let outcome = independent_addps([0x7f7f_ffff, 0, 0, 0], [0x7f7f_ffff, 0, 0, 0], rc);
        assert!(outcome.3 && outcome.5, "RC={rc:#x}");
    }

    let large_gap = independent_addps([0x3f80_0000, 0, 0, 0], [1, 0, 0, 0], 0x1f80);
    assert_eq!(large_gap.0[0], 0x3f80_0000);
    assert!(large_gap.2 && large_gap.5); // DE and PE
    let large_gap_up = independent_addps([0x3f80_0000, 0, 0, 0], [1, 0, 0, 0], 0x5f80);
    assert_eq!(large_gap_up.0[0], 0x3f80_0001);

    let nan = independent_addps([0x7f80_0042, 0, 0, 0], [0x7fc0_1234, 0, 0, 0], 0x1f80);
    assert_eq!(nan.0[0], 0x7fc0_0042);
    assert!(nan.1);
    let qnan_snan = independent_addps([0x7fc0_0042, 0, 0, 0], [0x7f80_0001, 0, 0, 0], 0x1f80);
    assert_eq!(qnan_snan.0[0], 0x7fc0_0042);
    assert!(qnan_snan.1);
    let opposite_inf = independent_addps([0x7f80_0000, 0, 0, 0], [0xff80_0000, 0, 0, 0], 0x1f80);
    assert_eq!(opposite_inf.0[0], 0xffc0_0000);
    assert!(opposite_inf.1);
    let inf_subnormal = independent_addps([0x7f80_0000, 0, 0, 0], [1, 0, 0, 0], 0x1f80);
    assert!(inf_subnormal.2);
    let nan_suppresses_denormal = independent_addps([0x7fc0_0042, 0, 0, 0], [1, 0, 0, 0], 0x1f80);
    assert!(!nan_suppresses_denormal.2);

    // Cross-lane IE/DE pre-events suppress all post-events; sticky old bits do
    // not count as new events.
    let pre = independent_addps(
        [0x7f80_0001, 1, 0x7f7f_ffff, 0x0080_0001],
        [0x3f80_0000, 0x3f80_0000, 0x7f7f_ffff, 0x8080_0000],
        0x1c00,
    );
    assert!(pre.1 && pre.2);
    assert!(!pre.3 && !pre.4 && !pre.5);

    let all_events = independent_addps(
        [0x7f80_0001, 1, 0x7f7f_ffff, 0x0080_0001],
        [0x3f80_0000, 0x3f80_0000, 0x7f7f_ffff, 0x8080_0000],
        0x9f80,
    );
    assert_eq!(all_events.0, [0x7fc0_0001, 0x3f80_0000, 0x7f80_0000, 0]);
    assert_eq!(
        (
            all_events.1,
            all_events.2,
            all_events.3,
            all_events.4,
            all_events.5
        ),
        (true, true, true, true, true)
    );
    let pre_matrix = independent_addps(
        [0x7f80_0001, 1, 0x7f7f_ffff, 0x0080_0001],
        [0x3f80_0000, 0x3f80_0000, 0x7f7f_ffff, 0x8080_0000],
        0x9f80 & !((1 << 7) | (1 << 8)),
    );
    assert!(pre_matrix.1 && pre_matrix.2);
    assert!(!pre_matrix.3 && !pre_matrix.4 && !pre_matrix.5);
}

#[test]
fn addps_profile_admits_all_pairs_and_rejects_alias_encodings() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37ba_addps_xmm0_xmm1.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongAddpsRegistersV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "0f58c1");
    fixture.validate().unwrap();

    for destination in 0..16u8 {
        for source in 0..16u8 {
            let modrm = 0xc0 | ((destination & 7) << 3) | (source & 7);
            let mut code = Vec::new();
            if destination >= 8 || source >= 8 {
                code.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            code.extend([0x0f, 0x58, modrm]);
            let text = code
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                addps_register_encoding(&text),
                Some((destination, source, code.len() as u32)),
                "{destination},{source} {text}"
            );
        }
    }
    for bytes in [
        "660f58c1",     // ADDPD encoding
        "f20f58c1",     // F2 mandatory prefix
        "f30f58c1",     // F3 mandatory prefix
        "420f58c1",     // REX.X
        "480f58c1",     // REX.W
        "400f58c1",     // redundant REX
        "0f5801",       // memory ModR/M
        "0f58c190",     // trailing byte
        "c57c58c1",     // VEX
        "62f17c0858c1", // EVEX
    ] {
        assert_eq!(addps_register_encoding(bytes), None, "{bytes}");
    }

    for (bytes, length, stop) in [
        ("660f58c1", 4, 0x10_0004),
        ("f20f58c1", 4, 0x10_0004),
        ("f30f58c1", 4, 0x10_0004),
        ("0f5801", 3, 0x10_0003),
        ("0f58c190", 4, 0x10_0004),
    ] {
        let mut malformed = fixture.clone();
        malformed.memory[0].initial_bytes_hex = bytes.to_owned();
        malformed.memory[0].length = length;
        malformed.memory[0].comparison_mask_hex = "ff".repeat(length as usize);
        malformed.stop_rip = Some(HexU64(stop));
        assert!(malformed.validate().is_err(), "{bytes}");
    }
    for mxcsr in [0x1fc0, 0x1b80, 0x1780, 0x0001_0000] {
        let mut invalid_mxcsr = fixture.clone();
        invalid_mxcsr.initial_state.sse.as_mut().unwrap().mxcsr = HexU64(mxcsr);
        assert!(invalid_mxcsr.validate().is_err(), "MXCSR={mxcsr:#x}");
    }
}

#[test]
fn independent_addss_oracle_accepts_every_memory_profile_rc_and_sticky_history() {
    for (lhs, rhs) in [
        (0x3f80_0000, 0x3380_0000),
        (0x3f80_0001, 0x3380_0000),
        (0x3f80_0000, 1),
        (0x0000_0001, 0x8000_0000),
        (0x0000_0001, 0x0000_0001),
        (0x7f7f_ffff, 0x7300_0000),
        (0x7f80_0001, 0x7fc0_1234),
        (0x7fc0_1234, 0x3f80_0000),
        (0x7f80_0000, 0xff80_0000),
        (0x7f80_0000, 0x3f80_0000),
        (0x0000_0000, 0x8000_0000),
        (0x8000_0000, 0x8000_0000),
    ] {
        for rc in 0..4u32 {
            let base_mxcsr = 0x1f80 | (rc << 13);
            let expected = independent_addss(lhs, rhs, base_mxcsr);
            for old_sticky in 0..64u32 {
                assert_eq!(
                    independent_addss(lhs, rhs, base_mxcsr | old_sticky),
                    expected,
                    "{lhs:08x}+{rhs:08x} RC={rc} old={old_sticky:02x}"
                );
            }
        }
    }
}

#[test]
fn addss_profile_oracle_and_encoding_cover_scalar_boundaries() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37bc_addss_xmm0_xmm1.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(fixture.profile, Profile::LongAddssRegistersV1);
    assert_eq!(fixture.memory[0].initial_bytes_hex, "f30f58c1");
    fixture.validate().unwrap();

    for destination in 0..16u8 {
        for source in 0..16u8 {
            let modrm = 0xc0 | ((destination & 7) << 3) | (source & 7);
            let mut code = vec![0xf3];
            if destination >= 8 || source >= 8 {
                code.push(0x40 | ((destination >> 1) & 4) | (source >> 3));
            }
            code.extend([0x0f, 0x58, modrm]);
            let text = code
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(
                addss_register_encoding(&text),
                Some((destination, source, code.len() as u32)),
                "{destination},{source} {text}"
            );
        }
    }
    for bytes in [
        "660f58c1",     // ADDPD encoding
        "f20f58c1",     // ADDSD encoding
        "f3400f58c1",   // redundant REX
        "f3420f58c1",   // REX.X
        "f3480f58c1",   // REX.W
        "40f30f58c1",   // reordered REX
        "f3660f58c1",   // repeated mandatory prefix
        "f30f5801",     // memory ModR/M
        "f30f58c190",   // trailing byte
        "c5fa58c1",     // VEX
        "62f17e0858c1", // EVEX
    ] {
        assert_eq!(addss_register_encoding(bytes), None, "{bytes}");
    }
    for (rc, expected) in [
        (0, 0x3f80_0000),
        (1, 0x3f80_0000),
        (2, 0x3f80_0001),
        (3, 0x3f80_0000),
    ] {
        let (value, ie, de, oe, ue, pe) =
            independent_addss(0x3f80_0000, 0x3380_0000, 0x1f80 | (rc << 13));
        assert_eq!(value, expected);
        assert!(!ie && !de && !oe && !ue && pe);
    }
    let exact_tiny = independent_addss(0x0080_0001, 0x8080_0000, 0x1f80);
    assert_eq!(exact_tiny, (1, false, false, false, false, false));
    let ftz_tiny = independent_addss(0x0080_0001, 0x8080_0000, 0x9f80);
    assert_eq!(ftz_tiny, (0, false, false, false, true, true));
    let midpoint = independent_addss(0x7f7f_ffff, 0x7300_0000, 0x3f80);
    assert_eq!(midpoint, (0x7f7f_ffff, false, false, false, false, true));
    let large_gap = independent_addss(0x3f80_0000, 1, 0x5f80);
    assert_eq!(large_gap, (0x3f80_0001, false, true, false, false, true));
    let nan = independent_addss(0x7f80_0001, 0x7fc0_1234, 0x1f80);
    assert_eq!(nan.0, 0x7fc0_0001);
    assert!(nan.1);
    assert!(!nan.2 && !nan.3 && !nan.4 && !nan.5);
}

#[test]
fn independent_sqrtps_raw_oracle_covers_event_products_masks_and_rc_stickies() {
    let categories = [
        0x4080_0000u32, // exact neutral
        0xbf80_0000,    // IE
        0x0000_0002,    // DE only
        0x4000_0000,    // PE only
        0x0000_0001,    // DE + PE
    ];
    for product in 0u32..625 {
        let mut value = product;
        let mut lanes = [0u32; 4];
        let mut expected_ie = false;
        let mut expected_de = false;
        let mut expected_pe = false;
        for lane in &mut lanes {
            let category = (value % 5) as usize;
            *lane = categories[category];
            expected_ie |= category == 1;
            expected_de |= category == 2 || category == 4;
            expected_pe |= category == 3 || category == 4;
            value /= 5;
        }
        for masks in 0u32..8 {
            let mxcsr = 0x1f80 & !(1 << 7) & !(1 << 8) & !(1 << 12)
                | (u64::from(masks & 1) << 7)
                | (u64::from((masks >> 1) & 1) << 8)
                | (u64::from((masks >> 2) & 1) << 12);
            let (_, ie, de, pe) = super::independent_sqrtps(lanes, mxcsr as u32);
            assert_eq!((ie, de, pe), (expected_ie, expected_de, expected_pe));
            let im = masks & 1 != 0;
            let dm = masks & 2 != 0;
            let pm = masks & 4 != 0;
            let pre_unmasked = ie && !im || de && !dm;
            for osxmmexcpt in [false, true] {
                let vector = if pre_unmasked || !pre_unmasked && pe && !pm {
                    Some(if osxmmexcpt { 19 } else { 6 })
                } else {
                    None
                };
                let expected_vector = if (expected_ie && !im) || (expected_de && !dm) {
                    Some(if osxmmexcpt { 19 } else { 6 })
                } else if expected_pe && !pm {
                    Some(if osxmmexcpt { 19 } else { 6 })
                } else {
                    None
                };
                assert_eq!(vector, expected_vector, "product={product}, masks={masks}");
                let committed = if pre_unmasked {
                    u32::from(ie) | (u32::from(de) << 1)
                } else {
                    u32::from(ie) | (u32::from(de) << 1) | (u32::from(pe) << 5)
                };
                assert_eq!(
                    committed & 0x23,
                    committed,
                    "product={product}, masks={masks}"
                );
                assert_eq!(vector.is_some(), pre_unmasked || pe && !pm);
            }
        }
    }
    for rc in 0u32..4 {
        for sticky in 0u32..64 {
            let mxcsr = 0x1f80 | (rc << 13) | sticky;
            let (values, ie, de, pe) = super::independent_sqrtps(
                [0x4000_0000, 0x4080_0000, 0x4110_0000, 0x4180_0000],
                mxcsr,
            );
            assert_eq!(values[0], if rc == 2 { 0x3fb5_04f4 } else { 0x3fb5_04f3 });
            assert!(!ie && !de && pe);
            assert_eq!(sticky | (u32::from(pe) << 5), sticky | (1 << 5));
        }
    }
    for lane in 0..4 {
        for (raw, signaling) in [
            (0x7fc1_2345u32, false),
            (0xffc1_2345u32, false),
            (0x7f81_2345u32, true),
            (0xff81_2345u32, true),
        ] {
            let lanes = std::array::from_fn(|index| if index == lane { raw } else { 0x4080_0000 });
            for im in [false, true] {
                let mxcsr = if im { 0x1f80 } else { 0x1f80 & !(1 << 7) };
                let (_, ie, de, pe) = super::independent_sqrtps(lanes, mxcsr);
                assert_eq!(ie, signaling, "lane={lane}, raw={raw:#x}, IM={im}");
                assert!(!de && !pe, "lane={lane}, raw={raw:#x}, IM={im}");
                for osxmmexcpt in [false, true] {
                    let fault = signaling && !im;
                    let vector = fault.then_some(if osxmmexcpt { 19 } else { 6 });
                    assert_eq!(vector.is_some(), signaling && !im);
                }
            }
        }
    }
}

#[test]
fn subsd_m64_unmasked_matrix_matches_independent_raw_u64_oracle() {
    use od_isa::execute::{
        RunConfig, RunExit, SubsdMemoryRaxUnmaskedControls,
        run_with_subsd_memory_rax_unmasked_controls,
    };
    use od_isa::memory::{AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError};
    use od_isa::{LongModeState, Register};

    struct MatrixMemory {
        rhs: [u8; 8],
        reads: usize,
    }
    impl GuestMemory for MatrixMemory {
        fn fetch(&mut self, address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
            const CODE: [u8; 4] = [0xf2, 0x0f, 0x5c, 0x00];
            CODE.get(address.checked_sub(0x1000_0000).unwrap_or(u64::MAX) as usize)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(
            &mut self,
            address: u64,
            _context: AccessContext,
            destination: &mut [u8],
        ) -> Result<(), MemoryError> {
            if address != 0x2001 || destination.len() != 8 {
                return Err(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                });
            }
            destination.copy_from_slice(&self.rhs);
            self.reads += 1;
            Ok(())
        }
        fn write(
            &mut self,
            address: u64,
            _context: AccessContext,
            _source: &[u8],
        ) -> Result<(), MemoryError> {
            Err(MemoryError::PermissionDenied {
                address,
                access: AccessKind::Write,
            })
        }
        fn atomic_exchange_integer(
            &mut self,
            address: u64,
            _context: AccessContext,
            _size: od_isa::OperandSize,
            _replacement: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }
        fn atomic_fetch_add_integer(
            &mut self,
            address: u64,
            _context: AccessContext,
            _size: od_isa::OperandSize,
            _addend: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }
        fn atomic_compare_exchange_integer(
            &mut self,
            address: u64,
            _context: AccessContext,
            _size: od_isa::OperandSize,
            _expected: u64,
            _replacement: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }
        fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    let vectors: [(u64, u64); 22] = [
        (0x4008_0000_0000_0000, 0x3ff0_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x3fb9_9999_9999_999a),
        (0x3ff0_0000_0000_0000, 0x3ca0_0000_0000_0000),
        (0x0000_0000_0000_0000, 0x8000_0000_0000_0000),
        (0x0000_0000_0000_0001, 0x3ff0_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x0000_0000_0000_0001),
        (0x7ff8_0000_0000_1234, 0x0000_0000_0000_0001),
        (0xfff0_0000_0000_0001, 0x0000_0000_0000_0001),
        (0x7ff8_0000_0000_1234, 0xfff0_0000_0000_0001),
        (0xfff8_0000_0000_1234, 0x3ff0_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x7ff8_0000_0000_1234),
        (0x3ff0_0000_0000_0000, 0xfff8_0000_0000_1234),
        (0x7ff0_0000_0000_0001, 0x3ff0_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x7ff0_0000_0000_0001),
        (0x3ff0_0000_0000_0000, 0xfff0_0000_0000_0001),
        (0xfff0_0000_0000_0001, 0x7ff8_0000_0000_1234),
        (0x7ff0_0000_0000_0000, 0x7ff0_0000_0000_0000),
        (0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000),
        (0xfff0_0000_0000_0000, 0x3ff0_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x7ff0_0000_0000_0000),
        (0x7fe0_0000_0000_0000, 0xff7f_ffff_ffff_ffff),
        (0x0000_0000_0000_0001, 0x0000_0000_0000_0000),
    ];
    for (lhs, rhs) in vectors {
        for rc in 0..4u32 {
            for sticky in 0..64u32 {
                for masks in 0..16u32 {
                    for osxmmexcpt in [false, true] {
                        let sticky_bits =
                            (0..6).fold(0u32, |bits, bit| bits | (((sticky >> bit) & 1) << bit));
                        let mut mxcsr = 0x1f80 | sticky_bits | (rc << 13);
                        for (selector, bit) in [(1, 7), (2, 8), (4, 9), (8, 12)] {
                            if masks & selector == 0 {
                                mxcsr &= !(1 << bit);
                            }
                        }
                        let (expected_low, ie, de, oe, ue, pe) = independent_subsd(lhs, rhs, mxcsr);
                        let pre_unmasked =
                            (ie && mxcsr & (1 << 7) == 0) || (de && mxcsr & (1 << 8) == 0);
                        let post_unmasked = !pre_unmasked && pe && mxcsr & (1 << 12) == 0;
                        let fault = pre_unmasked || post_unmasked;
                        let expected_mxcsr = mxcsr
                            | u32::from(ie)
                            | (u32::from(de) << 1)
                            | (u32::from(oe) << 3)
                            | (u32::from(ue) << 4)
                            | (u32::from(pe) << 5);

                        let mut state = LongModeState::default();
                        state.rip = 0x1000_0000;
                        state.gpr[Register::Rax as usize] = 0x2001;
                        state.rflags = (1 << 1) | (1 << 11);
                        state.sse = Some(od_isa::state::SseState {
                            xmm: [[0x5a; 16]; 16],
                            mxcsr,
                        });
                        state.sse.as_mut().unwrap().xmm[0][..8].copy_from_slice(&lhs.to_le_bytes());
                        let before_xmm = state.sse.as_ref().unwrap().xmm[0];
                        let mut memory = MatrixMemory {
                            rhs: rhs.to_le_bytes(),
                            reads: 0,
                        };
                        let result = run_with_subsd_memory_rax_unmasked_controls(
                            &mut state,
                            &mut memory,
                            RunConfig {
                                instruction_budget: 1,
                                stop_rip: Some(0x1000_0004),
                                context: AccessContext::default(),
                            },
                            SubsdMemoryRaxUnmaskedControls {
                                sse2_available: true,
                                cr0_em: false,
                                cr0_ts: false,
                                cr4_osfxsr: true,
                                cr4_osxmmexcpt: osxmmexcpt,
                            },
                            || false,
                        );
                        if fault {
                            assert_eq!(
                                result.exit,
                                RunExit::Fault(ArchitecturalFault {
                                    vector: if osxmmexcpt { 19 } else { 6 },
                                    error_code: None,
                                    linear_address: None,
                                })
                            );
                            assert_eq!(result.instructions_executed, 0);
                            assert_eq!(state.rip, 0x1000_0000);
                            assert_eq!(state.sse.as_ref().unwrap().xmm[0], before_xmm);
                        } else {
                            assert_eq!(result.exit, RunExit::StopAddress);
                            assert_eq!(result.instructions_executed, 1);
                            assert_eq!(state.rip, 0x1000_0004);
                            assert_eq!(
                                u64::from_le_bytes(
                                    state.sse.as_ref().unwrap().xmm[0][..8].try_into().unwrap()
                                ),
                                expected_low
                            );
                        }
                        assert_eq!(state.sse.as_ref().unwrap().mxcsr, expected_mxcsr);
                        assert_eq!(&state.sse.as_ref().unwrap().xmm[0][8..], &before_xmm[8..]);
                        assert_eq!(state.rflags, (1 << 1) | (1 << 11));
                        assert_eq!(memory.reads, 1);
                    }
                }
            }
        }
    }
}

#[test]
fn subsd_memory_write_permission_preserves_masked_admission_only() {
    let mut masked = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_37dl_subsd_m64_halfway.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(masked.profile, Profile::LongSubsdMemoryRaxV1);
    masked.memory[1].permissions.write = true;
    assert!(masked.validate().is_ok());

    let mut unmasked = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38af_subsd_unmasked_success.case.json"
    ))
    .unwrap()
    .fixture;
    assert_eq!(unmasked.profile, Profile::LongSubsdMemoryRaxUnmaskedV1);
    unmasked.memory[1].permissions.write = true;
    assert!(unmasked.validate().is_err());
}

#[test]
fn mulss_memory_unmasked_schema_oracle_matrix() {
    let seed = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38ag_mulss_memory_unmasked_wpr_min_normal.case.json"
    ))
    .unwrap()
    .fixture;
    let operands: [(u32, u32); 13] = [
        (0x7fc0_0001, 0x3f80_0000), // QNaN in destination.
        (0x3f80_0000, 0xffc1_2345), // signed QNaN in memory.
        (0x7f80_0001, 0x3f80_0000), // SNaN in destination.
        (0x3f80_0000, 0xff80_0001), // signed SNaN in memory.
        (0x7fc0_0001, 0x0000_0001), // QNaN suppresses denormal.
        (0x0000_0000, 0xff80_0000), // zero times negative infinity.
        (0x8000_0000, 0xc040_0000), // signed zero times negative finite.
        (0x7f80_0000, 0xc000_0000), // signed infinity product.
        (0x4000_0000, 0x3f00_0000), // exact product.
        (0x3f8c_cccd, 0x3f8c_cccd), // inexact product.
        (0x0000_0001, 0x3f00_0000), // tiny inexact product.
        (0x7f7f_ffff, 0x4000_0000), // overflow.
        (0x007f_ffff, 0x3f80_0001), // WPR rounds to minimum normal.
    ];
    let mut comparisons = 0usize;
    for (vector_index, (lhs, rhs)) in operands.into_iter().enumerate() {
        for rc in 0..4u32 {
            for sticky in 0..64u32 {
                for masks in 0..16u32 {
                    for osxmmexcpt in [false, true] {
                        let mask_bits = ((masks & 1) << 7)
                            | (((masks >> 1) & 1) << 8)
                            | (((masks >> 2) & 1) << 9)
                            | (((masks >> 3) & 1) << 12)
                            | (1 << 10)
                            | (1 << 11);
                        let mxcsr = (rc << 13) | sticky | mask_bits;
                        let mut fixture = seed.clone();
                        fixture.case_id = format!(
                            "s2_38ag_matrix_{vector_index}_{rc}_{sticky}_{masks}_{osxmmexcpt}"
                        );
                        fixture.initial_state.sse.as_mut().unwrap().xmm[0].replace_range(
                            0..8,
                            &lhs.to_le_bytes()
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect::<String>(),
                        );
                        fixture.memory[1].initial_bytes_hex = rhs
                            .to_le_bytes()
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect();
                        fixture.initial_state.sse.as_mut().unwrap().mxcsr =
                            HexU64(u64::from(mxcsr));
                        fixture
                            .mulss_memory_rax_unmasked_controls
                            .as_mut()
                            .unwrap()
                            .cr4_osxmmexcpt = osxmmexcpt;
                        let (value, ie, de, oe, ue, pe) = independent_mulss(lhs, rhs, mxcsr);
                        let pre = ie && mxcsr & (1 << 7) == 0 || de && mxcsr & (1 << 8) == 0;
                        let post = !pre && pe && mxcsr & (1 << 12) == 0;
                        let faults = pre || post;
                        fixture.expected_exit = if faults {
                            ExpectedExit::Exception
                        } else {
                            ExpectedExit::StopAddress
                        };
                        fixture
                            .validate()
                            .unwrap_or_else(|error| panic!("fixture: {error:?}"));
                        let loaded = LoadedFixture {
                            fixture,
                            input_sha256: "0".repeat(64),
                        };
                        let mut actual = result(&loaded);
                        let snapshot = match &mut actual.snapshot {
                            Observation::Available { value } => value,
                            _ => unreachable!(),
                        };
                        let status = HexU64(u64::from(
                            mxcsr
                                | u32::from(ie)
                                | (u32::from(de) << 1)
                                | (u32::from(oe) << 3)
                                | (u32::from(ue) << 4)
                                | (u32::from(pe) << 5),
                        ));
                        snapshot.state.sse.as_mut().unwrap().mxcsr = status;
                        if faults {
                            let vector = if osxmmexcpt { 19 } else { 6 };
                            actual.outcome = Outcome::Exception {};
                            actual.instructions_executed =
                                Observation::Available { value: HexU64(0) };
                            snapshot.state = loaded.fixture.initial_state.clone();
                            snapshot.state.sse.as_mut().unwrap().mxcsr = status;
                            snapshot.exception = Observation::Available {
                                value: Some(crate::result::ExceptionSnapshot {
                                    vector,
                                    error_code: Observation::Unavailable {
                                        reason: "exception has no architectural error code".into(),
                                    },
                                    faulting_rip: loaded.fixture.initial_state.rip,
                                    fault_address: Observation::Unavailable {
                                        reason: "not applicable to SIMD exception".into(),
                                    },
                                }),
                            };
                        } else {
                            snapshot.state.sse.as_mut().unwrap().xmm[0].replace_range(
                                0..8,
                                &value
                                    .to_le_bytes()
                                    .iter()
                                    .map(|byte| format!("{byte:02x}"))
                                    .collect::<String>(),
                            );
                        }
                        actual.validate_for(&loaded).unwrap_or_else(|error| {
                            panic!("vector={vector_index} RC={rc} sticky={sticky:#x} masks={masks:#x} OSXMMEXCPT={osxmmexcpt}: {error:?}")
                        });
                        comparisons += 1;
                    }
                }
            }
        }
    }
    assert_eq!(comparisons, 13 * 4 * 64 * 16 * 2);
}

#[test]
fn mulss_memory_unmasked_executor_matches_raw_u32_oracle_matrix() {
    use crate::memory::FlatMemory;
    use od_isa::execute::{
        MulssMemoryRaxUnmaskedControls, RunConfig, RunExit,
        run_with_mulss_memory_rax_unmasked_controls,
    };
    use od_isa::memory::AccessContext;

    let seed = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_38ag_mulss_memory_unmasked_wpr_min_normal.case.json"
    ))
    .unwrap()
    .fixture;
    // The runtime product mirrors every schema/oracle event vector, then adds
    // an exact-tiny result so all arithmetic classes reach the interpreter.
    let operands = [
        (0x7fc0_0001, 0x3f80_0000), // QNaN in destination.
        (0x3f80_0000, 0xffc1_2345), // Signed QNaN in memory.
        (0x7f80_0001, 0x3f80_0000), // SNaN in destination.
        (0x3f80_0000, 0xff80_0001), // Signed SNaN in memory.
        (0x7fc0_0001, 0x0000_0001), // QNaN suppresses the subnormal operand's DE.
        (0x0000_0000, 0xff80_0000), // Zero times negative infinity.
        (0x8000_0000, 0xc040_0000), // Signed zero times negative finite.
        (0x7f80_0000, 0xc000_0000), // Signed infinity product.
        (0x4000_0000, 0x3f00_0000), // Exact product.
        (0x3f8c_cccd, 0x3f8c_cccd), // Inexact product.
        (0x0000_0001, 0x3f00_0000), // Tiny inexact product with DE.
        (0x7f7f_ffff, 0x4000_0000), // Overflow.
        (0x007f_ffff, 0x3f80_0001), // WPR rounds to minimum normal.
        (0x0080_0000, 0x3f00_0000), // Exact tiny product: minimum normal * 0.5.
    ];
    let mut executions = 0usize;
    for (vector_index, (lhs, rhs)) in operands.into_iter().enumerate() {
        for rc in 0..4u32 {
            for sticky in 0..64u32 {
                for masks in 0..16u32 {
                    for osxmmexcpt in [false, true] {
                        let mask_bits = ((masks & 1) << 7)
                            | (((masks >> 1) & 1) << 8)
                            | (((masks >> 2) & 1) << 9)
                            | (((masks >> 3) & 1) << 12)
                            | (1 << 10)
                            | (1 << 11);
                        let mxcsr = (rc << 13) | sticky | mask_bits;
                        let (expected_value, ie, de, oe, ue, pe) =
                            independent_mulss(lhs, rhs, mxcsr);
                        let status = mxcsr
                            | u32::from(ie)
                            | (u32::from(de) << 1)
                            | (u32::from(oe) << 3)
                            | (u32::from(ue) << 4)
                            | (u32::from(pe) << 5);
                        let pre = (ie && mxcsr & (1 << 7) == 0) || (de && mxcsr & (1 << 8) == 0);
                        let post = !pre && pe && mxcsr & (1 << 12) == 0;

                        let mut fixture = seed.clone();
                        fixture.case_id = format!(
                            "s2_38ag_executor_{vector_index}_{rc}_{sticky}_{masks}_{osxmmexcpt}"
                        );
                        fixture.initial_state.sse.as_mut().unwrap().xmm[0].replace_range(
                            0..8,
                            &lhs.to_le_bytes()
                                .iter()
                                .map(|byte| format!("{byte:02x}"))
                                .collect::<String>(),
                        );
                        fixture.memory[1].initial_bytes_hex = rhs
                            .to_le_bytes()
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect();
                        fixture.initial_state.sse.as_mut().unwrap().mxcsr =
                            HexU64(u64::from(mxcsr));
                        fixture
                            .mulss_memory_rax_unmasked_controls
                            .as_mut()
                            .unwrap()
                            .cr4_osxmmexcpt = osxmmexcpt;
                        fixture.expected_exit = if pre || post {
                            ExpectedExit::Exception
                        } else {
                            ExpectedExit::StopAddress
                        };
                        fixture.validate().unwrap_or_else(|error| {
                            panic!("vector={vector_index} RC={rc} sticky={sticky:#x} masks={masks:#x} OSXMMEXCPT={osxmmexcpt}: {error:?}")
                        });

                        let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
                        state.sse = Some(
                            fixture
                                .initial_state
                                .sse
                                .as_ref()
                                .unwrap()
                                .to_cpu()
                                .unwrap(),
                        );
                        let original_xmm0 = state.sse.as_ref().unwrap().xmm[0];
                        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
                        let controls = MulssMemoryRaxUnmaskedControls {
                            sse_available: true,
                            cr0_em: false,
                            cr0_ts: false,
                            cr4_osfxsr: true,
                            cr4_osxmmexcpt: osxmmexcpt,
                        };
                        let result = run_with_mulss_memory_rax_unmasked_controls(
                            &mut state,
                            &mut memory,
                            RunConfig {
                                instruction_budget: 1,
                                stop_rip: Some(fixture.stop_rip.unwrap().get()),
                                context: AccessContext::default(),
                            },
                            controls,
                            || false,
                        );

                        let expected_fault = pre || post;
                        if expected_fault {
                            assert!(
                                matches!(&result.exit, RunExit::Fault(fault)
                                    if fault.vector == if osxmmexcpt { 19 } else { 6 }),
                                "vector={vector_index} RC={rc} sticky={sticky:#x} masks={masks:#x} OSXMMEXCPT={osxmmexcpt}: {:?}",
                                result.exit
                            );
                            assert_eq!(result.instructions_executed, 0);
                            assert_eq!(state.rip, fixture.initial_state.rip.get());
                            assert_eq!(state.sse.as_ref().unwrap().xmm[0], original_xmm0);
                        } else {
                            assert_eq!(result.exit, RunExit::StopAddress);
                            assert_eq!(result.instructions_executed, 1);
                            assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
                            assert_eq!(
                                u32::from_le_bytes(
                                    state.sse.as_ref().unwrap().xmm[0][..4].try_into().unwrap()
                                ),
                                expected_value,
                                "vector={vector_index} RC={rc} sticky={sticky:#x} masks={masks:#x} OSXMMEXCPT={osxmmexcpt}"
                            );
                        }
                        assert_eq!(state.sse.as_ref().unwrap().mxcsr, status);
                        executions += 1;
                    }
                }
            }
        }
    }
    assert_eq!(executions, 14 * 4 * 64 * 16 * 2);
}
