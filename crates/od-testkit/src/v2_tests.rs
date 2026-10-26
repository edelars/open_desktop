use super::*;

fn loaded() -> LoadedFixture {
    let old = crate::case::Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
    ))
    .unwrap();
    let converted = Fixture::convert_v1(&old).unwrap();
    Fixture::from_json(&converted.to_json().unwrap()).unwrap()
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
fn fld_m32_m64_budget_one_normal_unsupported_rejects_oe_only_status() {
    for (width, opcode, normal_bytes) in [(4, "d900", "0000803f"), (8, "dd00", "000000000000f03f")]
    {
        let mut fixture = x87_fixture().fixture;
        fixture.case_id = format!("local_fld_m{width}_normal_ze_rejection");
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

        fixture.initial_state.x87.as_mut().unwrap().status_word = HexU64(0x0004);
        fixture.initial_state.x87.as_mut().unwrap().control_word = HexU64(0x037f);
        fixture.initial_state.x87.as_mut().unwrap().tag_word = HexU64(u16::MAX.into());
        assert!(
            fixture.validate().is_ok(),
            "{opcode} ZE Unsupported fixture"
        );

        fixture.initial_state.x87.as_mut().unwrap().status_word = HexU64(0x0008);
        assert!(
            fixture.validate().is_err(),
            "{opcode} OE-only must not reject"
        );
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
    let returned = IntegerState::from_cpu_for(&cpu, Profile::LongX87StateV1);
    assert_eq!(returned.x87.as_ref(), Some(x87));
    assert!(
        IntegerState::from_cpu_for(&cpu, Profile::LongIntegerV1)
            .x87
            .is_none()
    );

    let mut incompatible_cpu = cpu.clone();
    incompatible_cpu.x87 = None;
    assert!(
        IntegerState::from_cpu_for(&incompatible_cpu, Profile::LongX87StateV1)
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
        .status_word = HexU64(0x0004);
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
    for combination in 0..8_u16 {
        let sticky = (combination & 3) | if combination & 4 != 0 { 0x20 } else { 0 };
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
    for forbidden_status in [0x0004, 0x0040, 0x0080, 0x8000] {
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
        .status_word = HexU64(0x04);
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
    for combination in 0..8_u16 {
        let sticky = (combination & 3) | if combination & 4 != 0 { 0x20 } else { 0 };
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
    for forbidden_status in [0x0004, 0x0040, 0x0080, 0x8000] {
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
        "66dac1", "40dac1", "dae9dfe0", "dac8", "dac1dfe1", "dac0dfe1",
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
    for program in ["66dbc1", "48dbc1", "9bdbc1", "dbc8", "dbd0", "dbc1dfe1"] {
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
fn x87_fcmove_schema_admits_only_five_exact_streams_and_reject_profiles() {
    let base = x87_fixture().fixture;
    for (program, budget) in [
        ("dac9", 1),
        ("dac9dfe0", 2),
        ("dac9dac9dfe0", 3),
        ("d9e8d9e8dbf1d9e0dac9dfe0", 6),
        ("d9e8d9e8d9e0dbf1dac9dfe0", 6),
    ] {
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
        code.initial_bytes_hex = program.into();
        code.length = length;
        code.comparison_mask_hex = "ff".repeat(length as usize);
        assert!(
            Fixture::from_json(&fixture.to_json().unwrap()).is_ok(),
            "{program}"
        );
    }
    for program in ["66dac9", "48dac9", "9bdac9", "dac8", "daca", "dac9dfe1"] {
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
