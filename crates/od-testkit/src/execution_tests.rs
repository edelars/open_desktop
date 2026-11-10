//! Cross-component CPU/memory checks. Byte programs are hand-authored, not encoder output.
use crate::HexU64;
use crate::case::MemoryPermissions;
use crate::memory::{FlatMemory, encode_bytes};
use crate::v2::{ExpectedExit, Fixture, decode_bytes};
use od_isa::execute::{
    Cvtdq2psControls, Cvtpd2dqControls, Cvtps2dqControls, Cvttpd2dqControls, Cvttps2dqControls,
    Fxrstor64Cr0Controls, Fxsave64Cr0Controls, REP_ITERATION_WORK_LIMIT, RunConfig, RunExit,
    StepOutcome, run, run_with_cvtdq2ps_register_controls, run_with_cvtpd2dq_register_controls,
    run_with_cvtps2dq_register_controls, run_with_cvttpd2dq_register_controls,
    run_with_cvttps2dq_register_controls, run_with_fxrstor64_cr0_controls,
    run_with_fxsave64_cr0_controls, run_with_rep_continuation, run_with_rflags_defined, step,
};
use od_isa::memory::{
    AccessContext, AccessKind, ArchitecturalFault, GuestMemory, MemoryError, PrivilegeLevel,
};
use od_isa::state::SseState;
use od_isa::{LongModeState, OperandSize, Register, RegisterView};

fn memory(code: &[u8], base: u64) -> FlatMemory {
    let source = crate::case::Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
    ))
    .unwrap();
    let mut input = Fixture::convert_v1(&source).unwrap();
    input.memory[0].guest_address = HexU64(base);
    input.memory[0].length = code.len() as u32;
    input.memory[0].initial_bytes_hex = encode_bytes(code);
    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    input.memory[0].permissions = MemoryPermissions {
        read: false,
        write: false,
        execute: true,
    };
    FlatMemory::from_fixture(&input).unwrap()
}

fn ud2_memory_regions(regions: &[(&str, u64, &[u8])]) -> FlatMemory {
    let source = crate::case::Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
    ))
    .unwrap();
    let mut fixture = Fixture::convert_v1(&source).unwrap();
    fixture.memory = regions
        .iter()
        .map(|(id, address, bytes)| crate::case::MemoryRegionSpec {
            region_id: (*id).into(),
            guest_address: HexU64(*address),
            length: bytes.len() as u32,
            permissions: MemoryPermissions {
                read: false,
                write: false,
                execute: true,
            },
            initial_bytes_hex: encode_bytes(bytes),
            comparison_mask_hex: "ff".repeat(bytes.len()),
        })
        .collect();
    FlatMemory::from_fixture(&fixture).unwrap()
}

#[test]
fn fmul_sti_st0_runner_checks_every_top_and_destination() {
    fn normal(significand: u64, exponent: u16) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&exponent.to_le_bytes());
        bytes
    }
    let one_and_half = normal(0xc000_0000_0000_0000, 0x3fff);
    let two = normal(0x8000_0000_0000_0000, 0x4000);
    let three = normal(0xc000_0000_0000_0000, 0x4000);
    let two_and_quarter = normal(0x9000_0000_0000_0000, 0x4000);
    for top in 0u8..8 {
        for destination in 0u8..8 {
            let physical_top = usize::from(top);
            let physical_destination = usize::from((top + destination) & 7);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | (1 << 9);
            x87.tag_word = u16::MAX;
            x87.registers[physical_top] = one_and_half;
            x87.set_tag(top, od_isa::state::X87Tag::Valid);
            if destination != 0 {
                x87.registers[physical_destination] = two;
                x87.set_tag(physical_destination as u8, od_isa::state::X87Tag::Valid);
            }
            state.x87 = Some(x87);
            let mut code = memory(&[0xdc, 0xc8 | destination], state.rip);
            let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
            assert_eq!(
                result.exit,
                RunExit::StopAddress,
                "TOP={top}, i={destination}"
            );
            assert_eq!(result.instructions_executed, 1);
            let after = state.x87.as_ref().unwrap();
            assert_eq!(
                after.registers[physical_destination],
                if destination == 0 {
                    two_and_quarter
                } else {
                    three
                }
            );
            assert_eq!(after.top(), top);
            assert_eq!(after.status_word & (1 << 9), 0);
            assert_eq!(state.rip, 0x1002);
        }
    }
}

#[test]
fn fdiv_st0_sti_runner_checks_every_top_and_source_and_inexact_rounding() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }
    let fifteen = normal(0xf000_0000_0000_0000, 0x4002, false);
    let three = normal(0xc000_0000_0000_0000, 0x4000, false);
    let five = normal(0xa000_0000_0000_0000, 0x4001, false);
    let one = normal(0x8000_0000_0000_0000, 0x3fff, false);

    for top in 0u8..8 {
        for source in 0u8..8 {
            let old_top = usize::from(top);
            let physical_source = usize::from((top + source) & 7);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | (1 << 9);
            x87.tag_word = u16::MAX;
            x87.registers = std::array::from_fn(|physical| {
                std::array::from_fn(|byte| (0x80 + physical * 7 + byte) as u8)
            });
            x87.registers[old_top] = if source == 0 {
                normal(0xd123_4567_89ab_cdef, 0x4000, top & 1 != 0)
            } else {
                fifteen
            };
            x87.set_tag(top, od_isa::state::X87Tag::Valid);
            if source != 0 {
                x87.registers[physical_source] = three;
                x87.set_tag(physical_source as u8, od_isa::state::X87Tag::Valid);
            }
            state.x87 = Some(x87);
            let before = state.x87.as_ref().unwrap().clone();
            let mut code = memory(&[0xd8, 0xf0 | source], state.rip);
            let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
            assert_eq!(result.exit, RunExit::StopAddress, "TOP={top}, i={source}");
            assert_eq!(result.instructions_executed, 1);
            let after = state.x87.as_ref().unwrap();
            assert_eq!(
                after.registers[old_top],
                if source == 0 { one } else { five }
            );
            assert_eq!(after.top(), top);
            assert_eq!(after.tag_word, before.tag_word);
            assert_eq!(after.status_word & (1 << 9), 0);
            assert_eq!(after.instruction_pointer, 0x1000);
            for physical in 0..8 {
                if physical != old_top {
                    assert_eq!(after.registers[physical], before.registers[physical]);
                }
            }
        }
    }

    let mut state = LongModeState::default();
    state.rip = 0x1000;
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 14);
    x87.tag_word = 0xfff0;
    x87.registers[0] = normal(0x8000_0000_0000_0000, 0x3ffe, false); // 0.5
    x87.registers[1] = normal(0xc000_0000_0000_0000, 0x3fff, false); // 1.5
    state.x87 = Some(x87);
    let before = state.clone();
    let mut code = memory(&[0xd8, 0xf1], state.rip);
    let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let mut expected = before;
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = normal(0xaaaa_aaaa_aaaa_aaab, 0x3ffd, false);
    expected_x87.status_word =
        (expected_x87.status_word & !((1 << 5) | (1 << 9))) | (1 << 5) | (1 << 9);
    expected_x87.instruction_pointer = 0x1000;
    expected.rip = 0x1002;
    assert_eq!(state, expected);

    let mut state = LongModeState::default();
    state.rip = 0x1200;
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0;
    x87.tag_word = 0xfff0;
    x87.registers[0] = three;
    x87.registers[1] = fifteen;
    state.x87 = Some(x87);
    for (run_config, expected_exit) in [
        (config(1, Some(state.rip)), RunExit::StopAddress),
        (config(0, None), RunExit::InstructionBudgetExhausted),
    ] {
        let before = state.clone();
        let mut code = memory(&[0xd8, 0xf9], state.rip);
        let result = run(&mut state, &mut code, run_config, || false);
        assert_eq!(result.exit, expected_exit);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }
}

#[test]
fn fdivrp_runner_checks_every_top_and_index_and_atomic_overflow_rejection() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }
    let one = normal(0x8000_0000_0000_0000, 0x3fff, false);
    let fifteen = normal(0xf000_0000_0000_0000, 0x4002, false);
    let three = normal(0xc000_0000_0000_0000, 0x4000, false);
    let five = normal(0xa000_0000_0000_0000, 0x4001, false);

    for top in 0u8..8 {
        for index in 0u8..8 {
            let old_top = usize::from(top);
            let destination = usize::from((top + index) & 7);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | (1 << 9);
            x87.tag_word = u16::MAX;
            x87.registers[old_top] = if index == 0 { one } else { fifteen };
            x87.set_tag(top, od_isa::state::X87Tag::Valid);
            if index != 0 {
                x87.registers[destination] = three;
                x87.set_tag(destination as u8, od_isa::state::X87Tag::Valid);
            }
            state.x87 = Some(x87);
            let mut code = memory(&[0xde, 0xf0 | index], state.rip);
            let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
            assert_eq!(result.exit, RunExit::StopAddress, "TOP={top}, i={index}");
            assert_eq!(result.instructions_executed, 1);
            let after = state.x87.as_ref().unwrap();
            assert_eq!(
                after.registers[destination],
                if index == 0 { one } else { five }
            );
            assert_eq!(after.tag(top), Some(od_isa::state::X87Tag::Empty));
            assert_eq!(after.top(), top.wrapping_add(1) & 7);
            assert_eq!(after.status_word & (1 << 9), 0);
            assert_eq!(state.rip, 0x1002);
        }
    }

    let mut state = LongModeState::default();
    state.rip = 0x1100;
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0;
    x87.tag_word = 0;
    // ST(0)/ST(i) overflows the bounded normal-result domain. Inexact finite
    // quotients are now admitted by the precision profile.
    x87.registers[0] = normal(u64::MAX, 0x7ffe, false);
    x87.registers[1] = normal(0x8000_0000_0000_0000, 0x3ffe, false);
    state.x87 = Some(x87);
    let before = state.clone();
    let mut code = memory(&[0xde, 0xf1], state.rip);
    let result = run(&mut state, &mut code, config(1, None), || false);
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xde });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn fdivr_runner_checks_every_top_and_index_and_inexact_rounding() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }
    let one = normal(0x8000_0000_0000_0000, 0x3fff, false);
    let fifteen = normal(0xf000_0000_0000_0000, 0x4002, false);
    let three = normal(0xc000_0000_0000_0000, 0x4000, false);
    let five = normal(0xa000_0000_0000_0000, 0x4001, false);

    for top in 0u8..8 {
        for index in 0u8..8 {
            let old_top = usize::from(top);
            let source = usize::from((top + index) & 7);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | (1 << 9);
            x87.tag_word = u16::MAX;
            x87.registers[old_top] = if index == 0 { one } else { three };
            x87.set_tag(top, od_isa::state::X87Tag::Valid);
            if index != 0 {
                x87.registers[source] = fifteen;
                x87.set_tag(source as u8, od_isa::state::X87Tag::Valid);
            }
            state.x87 = Some(x87);
            let before = state.clone();
            let mut code = memory(&[0xd8, 0xf8 | index], state.rip);
            let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
            assert_eq!(result.exit, RunExit::StopAddress, "TOP={top}, i={index}");
            assert_eq!(result.instructions_executed, 1);
            let after = state.x87.as_ref().unwrap();
            assert_eq!(
                after.registers[old_top],
                if index == 0 { one } else { five }
            );
            assert_eq!(after.top(), top);
            assert_eq!(after.tag_word, before.x87.as_ref().unwrap().tag_word);
            assert_eq!(after.status_word & (1 << 9), 0);
            assert_eq!(after.instruction_pointer, 0x1000);
            assert_eq!(state.rip, 0x1002);
        }
    }

    let mut state = LongModeState::default();
    state.rip = 0x1100;
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0;
    x87.tag_word = 0xfff0;
    x87.registers[0] = three;
    x87.registers[1] = one;
    state.x87 = Some(x87);
    let before = state.clone();
    let mut code = memory(&[0xd8, 0xf9], state.rip);
    let result = run(&mut state, &mut code, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    let mut expected = before;
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[0] = normal(0xaaaa_aaaa_aaaa_aaab, 0x3ffd, false);
    expected_x87.status_word =
        (expected_x87.status_word & !((1 << 5) | (1 << 9))) | (1 << 5) | (1 << 9);
    expected_x87.instruction_pointer = 0x1100;
    expected.rip = 0x1102;
    assert_eq!(state, expected);
}

#[test]
fn fdivr_sti_st0_runner_checks_every_top_and_index_and_inexact_rounding() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }
    let one = normal(0x8000_0000_0000_0000, 0x3fff, false);
    let six = normal(0xc000_0000_0000_0000, 0x4001, false);
    let three = normal(0xc000_0000_0000_0000, 0x4000, false);
    let two = normal(0x8000_0000_0000_0000, 0x4000, false);

    for top in 0u8..8 {
        for index in 0u8..8 {
            let physical_top = usize::from(top);
            let physical_destination = usize::from((top + index) & 7);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0700 | (1 << 9);
            x87.tag_word = u16::MAX;
            x87.registers = std::array::from_fn(|physical| {
                std::array::from_fn(|byte| (0x60 + physical * 7 + byte) as u8)
            });
            x87.registers[physical_top] = if index == 0 { one } else { six };
            x87.set_tag(top, od_isa::state::X87Tag::Valid);
            if index != 0 {
                x87.registers[physical_destination] = three;
                x87.set_tag(physical_destination as u8, od_isa::state::X87Tag::Valid);
            }
            x87.instruction_pointer = 0x1234;
            x87.data_pointer = 0x5678;
            x87.last_opcode = 0x05aa;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut code = memory(&[0xdc, 0xf0 | index], state.rip);
            let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
            assert_eq!(result.exit, RunExit::StopAddress, "TOP={top}, i={index}");
            assert_eq!(result.instructions_executed, 1);
            let mut expected = before.clone();
            let expected_x87 = expected.x87.as_mut().unwrap();
            expected_x87.registers[physical_destination] = if index == 0 { one } else { two };
            expected_x87.status_word &= !(1 << 9);
            expected_x87.instruction_pointer = 0x1000;
            expected.rip = 0x1002;
            assert_eq!(state, expected, "full state TOP={top}, i={index}");
        }
    }

    // Rounded 1 / 3 commits the literal p64 result and precision diagnostics.
    let mut state = LongModeState::default();
    state.rip = 0x1100;
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0;
    x87.tag_word = 0xfff0;
    x87.registers[0] = one;
    x87.registers[1] = three;
    state.x87 = Some(x87);
    let before = state.clone();
    let mut code = memory(&[0xdc, 0xf1], state.rip);
    let result = run(&mut state, &mut code, config(1, Some(0x1102)), || false);
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let mut expected = before;
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[1] = normal(0xaaaa_aaaa_aaaa_aaab, 0x3ffd, false);
    expected_x87.status_word |= (1 << 5) | (1 << 9);
    expected_x87.instruction_pointer = 0x1100;
    expected.rip = 0x1102;
    assert_eq!(state, expected);
}

#[test]
fn fdiv_sti_st0_runner_checks_every_top_and_index_and_inexact_rounding() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }
    let one = normal(0x8000_0000_0000_0000, 0x3fff, false);
    let fifteen = normal(0xf000_0000_0000_0000, 0x4002, false);
    let three = normal(0xc000_0000_0000_0000, 0x4000, false);
    let five = normal(0xa000_0000_0000_0000, 0x4001, false);

    for top in 0u8..8 {
        for index in 0u8..8 {
            let physical_top = usize::from(top);
            let physical_destination = usize::from((top + index) & 7);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0700 | (1 << 9);
            x87.tag_word = u16::MAX;
            x87.registers = std::array::from_fn(|physical| {
                std::array::from_fn(|byte| (0x60 + physical * 7 + byte) as u8)
            });
            x87.registers[physical_top] = if index == 0 { one } else { three };
            x87.set_tag(top, od_isa::state::X87Tag::Valid);
            if index != 0 {
                x87.registers[physical_destination] = fifteen;
                x87.set_tag(physical_destination as u8, od_isa::state::X87Tag::Valid);
            }
            x87.instruction_pointer = 0x1234;
            x87.data_pointer = 0x5678;
            x87.last_opcode = 0x05aa;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut code = memory(&[0xdc, 0xf8 | index], state.rip);
            let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
            assert_eq!(result.exit, RunExit::StopAddress, "TOP={top}, i={index}");
            assert_eq!(result.instructions_executed, 1);
            let mut expected = before.clone();
            let expected_x87 = expected.x87.as_mut().unwrap();
            expected_x87.registers[physical_destination] = if index == 0 { one } else { five };
            expected_x87.status_word &= !(1 << 9);
            expected_x87.instruction_pointer = 0x1000;
            expected.rip = 0x1002;
            assert_eq!(state, expected, "full state TOP={top}, i={index}");
        }
    }

    // ST(1) / ST(0) = 1/3 is inexact. The low four tag bits are zero,
    // so both physical input registers have Valid tags.
    let mut state = LongModeState::default();
    state.rip = 0x1100;
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0;
    x87.tag_word = 0xfff0;
    x87.registers[0] = three;
    x87.registers[1] = one;
    state.x87 = Some(x87);
    let before = state.clone();
    let mut code = memory(&[0xdc, 0xf9], state.rip);
    let result = run(&mut state, &mut code, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    let mut expected = before;
    let expected_x87 = expected.x87.as_mut().unwrap();
    expected_x87.registers[1] = normal(0xaaaa_aaaa_aaaa_aaab, 0x3ffd, false);
    expected_x87.status_word =
        (expected_x87.status_word & !((1 << 5) | (1 << 9))) | (1 << 5) | (1 << 9);
    expected_x87.instruction_pointer = 0x1100;
    expected.rip = 0x1102;
    assert_eq!(state, expected);
}

#[test]
fn fsubp_runner_literal_outputs_follow_sti_minus_st0_and_pop() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }

    for (destination, destination_value, source_value, expected) in [
        (
            1u8,
            normal(0xc000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3ffe, false),
        ),
        (
            2u8,
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0xc000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3ffe, true),
        ),
        (
            3u8,
            normal(0xc000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x3ffe, true),
        ),
        (
            4u8,
            normal(0x8000_0000_0000_0000, 0x3fff, true),
            normal(0xc000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x3ffe, false),
        ),
        (
            5u8,
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x4000, false),
        ),
        (
            6u8,
            normal(0x8000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x4000, true),
        ),
        (
            7u8,
            normal(0x8000_0000_0000_0001, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3fc0, false),
        ),
    ] {
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 1 << 9;
        x87.tag_word = u16::MAX & !3 & !(3 << (usize::from(destination) * 2));
        x87.registers[0] = source_value;
        x87.registers[usize::from(destination)] = destination_value;
        state.x87 = Some(x87);
        let mut code = memory(&[0xde, 0xe8 | destination], 0x1000);
        let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
        assert_eq!(result.exit, RunExit::StopAddress, "i={destination}");
        assert_eq!(result.instructions_executed, 1, "i={destination}");
        let after = state.x87.as_ref().unwrap();
        assert_eq!(
            after.registers[usize::from(destination)],
            expected,
            "i={destination}"
        );
        assert_eq!(after.top(), 1, "i={destination}");
        assert_eq!(after.tag_word & 3, 3, "old TOP is empty, i={destination}");
        assert_eq!(
            after.status_word & (1 << 9),
            0,
            "C1 clears, i={destination}"
        );
        assert_eq!(state.rip, 0x1002, "i={destination}");
    }
}

#[test]
fn fsubr_sti_st0_runner_outputs_use_st0_minus_sti_without_pop() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }
    for (st0, sti, expected) in [
        (
            normal(0xc000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3ffe, false),
        ),
        (
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0xc000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3ffe, true),
        ),
    ] {
        for top in 0u8..8 {
            let destination = usize::from((top + 3) & 7);
            let source = usize::from(top);
            let mut state = LongModeState::default();
            state.rip = 0x7000;
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = u16::from(top) << 11;
            x87.tag_word = 0;
            x87.registers[source] = st0;
            x87.registers[destination] = sti;
            state.x87 = Some(x87);
            let mut memory = memory(&[0xdc, 0xe3], 0x7000);
            let outcome = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(outcome.instructions_executed, 1);
            assert_eq!(state.rip, 0x7002);
            let after = state.x87.unwrap();
            assert_eq!(after.registers[destination], expected);
            assert_eq!(after.registers[source], st0);
            assert_eq!(after.top(), top);
            assert_eq!(after.tag_word, 0);
            assert_eq!(after.status_word, u16::from(top) << 11);
        }
    }
}

#[test]
fn fsubr_runner_literal_outputs_follow_sti_minus_st0_and_alias_is_atomic() {
    fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        bytes[8..].copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
        bytes
    }

    for (index, top, source, expected) in [
        (
            1u8,
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0xc000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3ffe, false),
        ),
        (
            2,
            normal(0xc000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0x8000_0000_0000_0000, 0x3ffe, true),
        ),
        (
            3,
            normal(0xc000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x3ffe, false),
        ),
        (
            4,
            normal(0x8000_0000_0000_0000, 0x3fff, true),
            normal(0xc000_0000_0000_0000, 0x3fff, true),
            normal(0x8000_0000_0000_0000, 0x3ffe, true),
        ),
        (
            5,
            normal(0x8000_0000_0000_0000, 0x3fff, true),
            normal(0xc000_0000_0000_0000, 0x3fff, false),
            normal(0xa000_0000_0000_0000, 0x4000, false),
        ),
        (
            6,
            normal(0x8000_0000_0000_0000, 0x3fff, false),
            normal(0xc000_0000_0000_0000, 0x3fff, true),
            normal(0xa000_0000_0000_0000, 0x4000, true),
        ),
    ] {
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 1 << 9;
        x87.tag_word = u16::MAX & !3 & !(3 << (usize::from(index) * 2));
        x87.registers[0] = top;
        x87.registers[usize::from(index)] = source;
        state.x87 = Some(x87);
        let before = state.clone();
        let mut code = memory(&[0xd8, 0xe8 | index], 0x1000);
        let result = run(&mut state, &mut code, config(1, Some(0x1002)), || false);
        assert_eq!(result.exit, RunExit::StopAddress, "i={index}");
        assert_eq!(result.instructions_executed, 1, "i={index}");
        let after = state.x87.as_ref().unwrap();
        assert_eq!(after.registers[0], expected, "i={index}");
        assert_eq!(
            after.registers[usize::from(index)],
            before.x87.as_ref().unwrap().registers[usize::from(index)]
        );
        assert_eq!(after.top(), 0, "no pop, i={index}");
        assert_eq!(after.tag_word, before.x87.as_ref().unwrap().tag_word);
        assert_eq!(
            after.status_word,
            before.x87.as_ref().unwrap().status_word & !(1 << 9)
        );
        assert_eq!(after.instruction_pointer, 0x1000);
        assert_eq!(state.rip, 0x1002);
    }

    let mut state = LongModeState::default();
    state.rip = 0x1000;
    let mut x87 = od_isa::state::X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.tag_word = u16::MAX & !3;
    x87.registers[0] = normal(0x8000_0000_0000_0000, 0x3fff, false);
    state.x87 = Some(x87);
    let before = state.clone();
    let mut code = memory(&[0xd8, 0xe8], 0x1000);
    let result = run(&mut state, &mut code, config(1, None), || false);
    assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xd8 });
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

fn config(budget: u64, stop: Option<u64>) -> RunConfig {
    RunConfig {
        instruction_budget: budget,
        stop_rip: stop,
        context: AccessContext::default(),
    }
}

#[test]
fn ftst_infinity_project_fixtures_match_literal_full_state_and_memory() {
    let directory = arithmetic_fixture_directory();
    for (case_id, negative, input_bytes, expected_status) in [
        (
            "s2_30at_fld_ftst_positive_infinity_fnstsw",
            false,
            "0000807f",
            0x3800,
        ),
        (
            "s2_30at_fld_ftst_negative_infinity_fnstsw",
            true,
            "000080ff",
            0x3900,
        ),
    ] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let memory_before = memory.snapshot();
        let mut state = input
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );

        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        assert_eq!(result.instructions_executed, 3, "{case_id}");
        assert_eq!(input.instruction_budget, 3, "{case_id}");
        assert_eq!(input.stop_rip, Some(HexU64(0x0010_0006)), "{case_id}");
        let stack = input
            .memory
            .iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(stack.initial_bytes_hex, input_bytes, "{case_id}");

        let mut expected = before;
        expected.rip = 0x0010_0006;
        expected.gpr[0] = (expected.gpr[0] & !0xffff) | u64::from(expected_status);
        let x87 = expected.x87.as_mut().unwrap();
        x87.status_word = expected_status;
        x87.registers[7] = [
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0x80,
            0xff,
            if negative { 0xff } else { 0x7f },
        ];
        x87.set_tag(7, od_isa::state::X87Tag::Special);
        x87.instruction_pointer = 0x0010_0002;
        x87.data_pointer = 0x0030_0004;
        assert_eq!(state, expected, "{case_id}");
        assert_eq!(memory.snapshot(), memory_before, "{case_id}");
    }
}

#[test]
fn fnstcw_literal_fixtures_check_raw_store_snapshot_and_fault_boundaries() {
    let directory = arithmetic_fixture_directory();
    for case_id in [
        "s2_31g_fnstcw_m16int_rc0",
        "s2_31g_fnstcw_m16int_rc1",
        "s2_31g_fnstcw_m16int_rc2",
        "s2_31g_fnstcw_m16int_rc3",
        "s2_31g_fnstcw_m16int_arbitrary_state",
        "s2_31g_fnstcw_m16int_fnstsw",
        "s2_31g_fnstcw_m16int_noncanonical_crossing",
    ] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before_memory = memory.snapshot();
        let mut state = input
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );

        match input.expected_exit {
            ExpectedExit::StopAddress => {
                assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
                let composition = case_id.ends_with("fnstsw");
                assert_eq!(
                    result.instructions_executed,
                    if composition { 2 } else { 1 }
                );
                let mut expected_state = before.clone();
                expected_state.rip += if composition { 4 } else { 2 };
                if composition {
                    let status_word = before.x87.as_ref().unwrap().status_word;
                    expected_state.gpr[Register::Rax as usize] =
                        (expected_state.gpr[Register::Rax as usize] & !0xffff)
                            | u64::from(status_word);
                }
                assert_eq!(state, expected_state, "{case_id}");
                assert_eq!(state.x87, before.x87, "{case_id}");
                let after_memory = memory.snapshot();
                let left = after_memory
                    .iter()
                    .find(|region| region.region_id == "data_left")
                    .unwrap();
                let right = after_memory
                    .iter()
                    .find(|region| region.region_id == "data_right")
                    .unwrap();
                let control_word = input.initial_state.x87.as_ref().unwrap().control_word.get();
                assert_eq!(
                    left.bytes_hex,
                    format!("{:02x}", control_word & 0xff),
                    "{case_id}"
                );
                assert!(
                    right
                        .bytes_hex
                        .starts_with(&format!("{:02x}", control_word >> 8)),
                    "{case_id}: {right:?}"
                );
                assert!(right.bytes_hex.ends_with("ccddeeff11223344556677889900"));
            }
            ExpectedExit::Exception => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    }),
                    "{case_id}"
                );
                assert_eq!(result.instructions_executed, 0, "{case_id}");
                assert_eq!(state, before, "{case_id}");
                assert_eq!(memory.snapshot(), before_memory, "{case_id}");
            }
            other => panic!("unexpected exit in {case_id}: {other:?}"),
        }
    }
}

#[test]
fn fild_m32_literal_fixtures_push_independent_binary80_and_observe_status() {
    let directory = arithmetic_fixture_directory();
    for case_id in ["s2_31n_fild_m32int_success", "s2_31n_fild_m32int_fnstsw"] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before_memory = memory.snapshot();
        let mut state = input
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let mut expected = state.clone();
        let composition = case_id.ends_with("fnstsw");
        let expected_rip = input.initial_state.rip.get() + if composition { 4 } else { 2 };
        expected.rip = expected_rip;
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
        expected_x87.tag_word = 0x3ffc;
        expected_x87.status_word = 0x7d00;
        expected_x87.instruction_pointer = input.initial_state.rip.get();
        expected_x87.data_pointer = input.initial_state.gpr[Register::Rax as usize].get();
        if composition {
            expected.gpr[Register::Rax as usize] = (expected.gpr[Register::Rax as usize] & !0xffff)
                | u64::from(expected_x87.status_word);
        }

        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        assert_eq!(
            result.instructions_executed,
            if composition { 2 } else { 1 }
        );
        assert_eq!(state, expected, "{case_id}");
        assert_eq!(
            state.x87.as_ref().unwrap().registers[7],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]
        );
        assert_eq!(memory.snapshot(), before_memory, "FILD must not write RAM");
    }
}

#[test]
fn fild_m16_literal_fixtures_push_independent_binary80_and_observe_status() {
    let directory = arithmetic_fixture_directory();
    for case_id in ["s2_31o_fild_m16int_success", "s2_31o_fild_m16int_fnstsw"] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before_memory = memory.snapshot();
        let mut state = input
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let mut expected = state.clone();
        let composition = case_id.ends_with("fnstsw");
        expected.rip = input.initial_state.rip.get() + if composition { 4 } else { 2 };
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
        expected_x87.tag_word = 0x3ffc;
        expected_x87.status_word = 0x7d00;
        expected_x87.instruction_pointer = input.initial_state.rip.get();
        expected_x87.data_pointer = input.initial_state.gpr[Register::Rax as usize].get();
        if composition {
            expected.gpr[Register::Rax as usize] = (expected.gpr[Register::Rax as usize] & !0xffff)
                | u64::from(expected_x87.status_word);
        }
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        assert_eq!(
            result.instructions_executed,
            if composition { 2 } else { 1 }
        );
        assert_eq!(state, expected, "{case_id}");
        assert_eq!(
            state.x87.as_ref().unwrap().registers[7],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]
        );
        assert_eq!(memory.snapshot(), before_memory, "FILD must not write RAM");
    }
}

#[test]
fn fild_m64_literal_fixtures_accept_exact_bytes_and_observe_pushed_status() {
    let directory = arithmetic_fixture_directory();
    for case_id in ["s2_31p_fild_m64int_success", "s2_31p_fild_m64int_fnstsw"] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let code = input
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .unwrap();
        assert_eq!(
            code.initial_bytes_hex,
            if case_id.ends_with("fnstsw") {
                "df28dfe0"
            } else {
                "df28"
            }
        );
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before_memory = memory.snapshot();
        let mut state = input
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let mut expected = state.clone();
        let composition = case_id.ends_with("fnstsw");
        expected.rip = input.initial_state.rip.get() + if composition { 4 } else { 2 };
        let expected_x87 = expected.x87.as_mut().unwrap();
        expected_x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
        expected_x87.tag_word = 0x3ffc;
        expected_x87.status_word = 0x7d00;
        expected_x87.instruction_pointer = input.initial_state.rip.get();
        expected_x87.data_pointer = input.initial_state.gpr[Register::Rax as usize].get();
        if composition {
            expected.gpr[Register::Rax as usize] = (expected.gpr[Register::Rax as usize] & !0xffff)
                | u64::from(expected_x87.status_word);
        }
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        assert_eq!(
            result.instructions_executed,
            if composition { 2 } else { 1 }
        );
        assert_eq!(state, expected, "{case_id}");
        assert_eq!(memory.snapshot(), before_memory, "FILD must not write RAM");
    }
}

#[test]
fn fnstsw_literal_fixtures_check_raw_store_snapshot_composition_and_faults() {
    let directory = arithmetic_fixture_directory();
    for case_id in [
        "s2_31h_fnstsw_m16_status_0000",
        "s2_31h_fnstsw_m16_mixed_state",
        "s2_31h_fnstsw_m16_status_ffff_fnstsw_ax",
        "s2_31h_fnstsw_m16_noncanonical_crossing",
    ] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before_memory = memory.snapshot();
        let mut state = input
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );

        match input.expected_exit {
            ExpectedExit::StopAddress => {
                assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
                let composition = case_id.ends_with("fnstsw_ax");
                assert_eq!(
                    result.instructions_executed,
                    if composition { 2 } else { 1 }
                );
                let mut expected_state = before.clone();
                expected_state.rip += if composition { 4 } else { 2 };
                if composition {
                    let status_word = before.x87.as_ref().unwrap().status_word;
                    expected_state.gpr[Register::Rax as usize] =
                        (expected_state.gpr[Register::Rax as usize] & !0xffff)
                            | u64::from(status_word);
                }
                assert_eq!(state, expected_state, "{case_id}");
                assert_eq!(state.x87, before.x87, "{case_id}");
                let after_memory = memory.snapshot();
                let left = after_memory
                    .iter()
                    .find(|region| region.region_id == "data_left")
                    .unwrap();
                let right = after_memory
                    .iter()
                    .find(|region| region.region_id == "data_right")
                    .unwrap();
                let status_word = before.x87.as_ref().unwrap().status_word;
                assert_eq!(left.bytes_hex, format!("{:02x}", status_word & 0xff));
                assert!(
                    right
                        .bytes_hex
                        .starts_with(&format!("{:02x}", status_word >> 8))
                );
                assert!(right.bytes_hex.ends_with("ccddeeff11223344556677889900"));
            }
            ExpectedExit::Exception => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    }),
                    "{case_id}"
                );
                assert_eq!(result.instructions_executed, 0, "{case_id}");
                assert_eq!(state, before, "{case_id}");
                assert_eq!(memory.snapshot(), before_memory, "{case_id}");
            }
            other => panic!("unexpected exit in {case_id}: {other:?}"),
        }
    }
}

#[test]
fn fnstcw_second_byte_permission_failure_is_atomic_across_fixture_regions() {
    let path =
        arithmetic_fixture_directory().join("s2_31g_fnstcw_m16int_arbitrary_state.case.json");
    let raw = std::fs::read(path).unwrap();
    let mut input = Fixture::from_json(&raw).unwrap().fixture;
    input
        .memory
        .iter_mut()
        .find(|region| region.region_id == "data_right")
        .unwrap()
        .permissions
        .write = false;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let before_memory = memory.snapshot();
    let mut state = input
        .initial_state
        .to_cpu_for(crate::v2::Profile::LongX87StateV1)
        .unwrap();
    let before = state.clone();

    let result = run(
        &mut state,
        &mut memory,
        config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::PermissionDenied {
            address: 0x0000_0000_0030_0002,
            access: AccessKind::Write,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(memory.snapshot(), before_memory);
}

struct AccessTraceMemory {
    inner: FlatMemory,
    accesses: Vec<(AccessKind, u64, usize, AccessContext)>,
    fetch_limit: Option<u64>,
}

impl GuestMemory for AccessTraceMemory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.accesses.push((AccessKind::Fetch, address, 1, context));
        if self.fetch_limit.is_some_and(|limit| address >= limit) {
            return Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            });
        }
        self.inner.fetch(address, context)
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.accesses
            .push((AccessKind::Read, address, destination.len(), context));
        self.inner.read(address, context, destination)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.accesses
            .push((AccessKind::Write, address, source.len(), context));
        self.inner.write(address, context, source)
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_exchange_integer(address, context, size, replacement)
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_fetch_add_integer(address, context, size, addend)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_compare_exchange_integer(address, context, size, expected, replacement)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.inner.mfence(context)
    }
}

fn execute_data_transfer_fixture(raw: &[u8], changes: &[(usize, u64)], retired: u64) -> FlatMemory {
    let fixture = Fixture::from_json(raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let mut expected = state.gpr;
    for &(register, value) in changes {
        expected[register] = value;
    }
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress, "{}", fixture.case_id);
    assert_eq!(result.instructions_executed, retired, "{}", fixture.case_id);
    assert_eq!(state.gpr, expected, "{}", fixture.case_id);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    assert_eq!(
        state.rflags, 0xcd7,
        "all modeled input flags must be preserved"
    );
    for region in memory.snapshot() {
        if region.region_id == "code" || region.region_id == "stack" {
            let initial = fixture
                .memory
                .iter()
                .find(|r| r.region_id == region.region_id)
                .unwrap();
            assert_eq!(region.bytes_hex, initial.initial_bytes_hex);
        }
    }
    memory
}

#[test]
fn c6_c7_and_moffs_have_hand_calculated_registers_and_actual_memory() {
    // Expected values/bytes were written independently of the decoder/executor.
    let memory = execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_immediate_moffs.case.json"),
        &[
            (0, 0xffff_ffff_8000_0001),
            (1, 0xaabb_ccdd_eeff_8000),
            (8, 0x8000_0000),
            (9, 0xffff_ffff_8000_0001),
        ],
        34,
    );
    let values: [&[u8]; 4] = [
        &[0x80],
        &[1, 0x80],
        &[1, 0, 0, 0x80],
        &[1, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff],
    ];
    let mut data = vec![0x5a; 80];
    for (slot, bytes) in values.iter().enumerate() {
        data[slot * 16..slot * 16 + bytes.len()].copy_from_slice(bytes);
    }
    data[64..68].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
    assert_region(&memory, "data", &data);
    let mut output = vec![0xa5; 128];
    for slot in 0..8 {
        let bytes = values[slot % 4];
        output[slot * 16..slot * 16 + bytes.len()].copy_from_slice(bytes);
    }
    assert_region(&memory, "moffs_output", &output);
    assert_words(
        &memory,
        &[
            0x1122_3344_5566_7f88,
            0x1122_3344_5566_7f80,
            0x1122_3344_5566_8001,
            0x8000_0001,
            0xffff_ffff_8000_0001,
            0xffff_ffff_8000_0080,
            0xffff_ffff_8000_8001,
            0x8000_0001,
            0xffff_ffff_8000_0001,
        ],
    );
}

fn assert_region(memory: &FlatMemory, id: &str, expected: &[u8]) {
    let region = memory
        .snapshot()
        .into_iter()
        .find(|r| r.region_id == id)
        .unwrap();
    assert_eq!(region.bytes_hex, encode_bytes(expected), "{id}");
}

fn assert_words(memory: &FlatMemory, expected: &[u64]) {
    let bytes: Vec<u8> = expected
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    assert_region(memory, "results", &bytes);
}

const EXTENSION_CHANGES: &[(usize, u64)] = &[
    (0, 0x1122_3344_5566_ff80),
    (1, 0xffff_ffff),
    (2, 0xffff_8001),
    (6, 0xffff_ffff),
    (7, 0xf8),
    (8, 0x80),
    (9, 0x8001),
    (10, 0x8000),
    (11, 0xffff_ffff_ffff_ff80),
    (12, 0xffff_ffff_ffff_8000),
    (13, 0xffff_ffff_8000_0000),
    (14, 0x7788_99aa_bbcc_8000),
    (15, 0x80),
];
const EXTENSION_RESULTS: &[u64] = &[
    0x77,
    0xff,
    0x1122_3344_5566_0080,
    0x80,
    0x8001,
    0x8000,
    0x1122_3344_5566_ff80,
    0xffff_ffff,
    0xffff_ffff_ffff_ff80,
    0xffff_8001,
    0xffff_ffff_ffff_8000,
    0xffff_ffff_8000_0000,
    0x8000_0000,
    0x7788_99aa_bbcc_8000,
    0xffff_ffff,
    0xf8,
    0x80,
];

#[test]
fn mov_extensions_cover_high_bytes_rex_aliases_signedness_and_destination_widths() {
    let memory = execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_extend.case.json"),
        EXTENSION_CHANGES,
        34,
    );
    let data = memory
        .snapshot()
        .into_iter()
        .find(|r| r.region_id == "data")
        .unwrap();
    assert_eq!(data.bytes_hex, "80ff0180008000000080ffff");
    assert_words(&memory, EXTENSION_RESULTS);
}

#[test]
fn addr32_transfers_truncate_start_address_and_keep_each_result_observable() {
    let memory = execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_addr32_transfers.case.json"),
        EXTENSION_CHANGES,
        38,
    );
    assert_words(&memory, EXTENSION_RESULTS);
    let mut data = vec![0x5a; 96];
    data[..12].copy_from_slice(&[0x80, 0xff, 1, 0x80, 0, 0x80, 0, 0, 0, 0x80, 0xff, 0xff]);
    data[32] = 0x80;
    data[48..50].copy_from_slice(&[1, 0x80]);
    data[64..68].copy_from_slice(&[1, 0, 0, 0x80]);
    data[80..88].copy_from_slice(&[1, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff]);
    assert_region(&memory, "data", &data);
}

#[test]
fn lea_produces_unmapped_noncanonical_offsets_without_reading_or_faulting() {
    execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_lea_no_read.case.json"),
        &[
            (0, 0x0000_8000_0000_0000),
            (2, 0xffff_ffff),
            (6, 0x0123_4567_89ab_0012),
            (7, 0x13),
            (8, 0x0000_8000_0000_0005),
            (9, 0xffff_ffff),
            (10, 0x101020),
            (11, 0xaabb_ccdd_eeff_ffff),
            (12, 0x0000_8000_0000_0000),
            (13, 0),
            (14, 0x7788_99aa_bbcc_0023),
            (15, 0x0000_8000_0000_0000),
        ],
        12,
    );
}

#[test]
fn register_xchg_aliases_use_old_values_and_nop_is_not_eax_zero_extension() {
    execute_data_transfer_fixture(
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_10_xchg_aliases.case.json"),
        &[
            (0, 0x89ab_cdef),
            (1, 0xaabb_ccdd_eeff_ba98),
            (2, 0x5566_0011),
            (4, 0x300077),
            (8, 0x7654_3210),
            (9, 0x0123_4567_89ab_cdef),
            (10, 0x1122_3344_5566_7788),
            (11, 0x89ab_cdef),
            (12, 0xfedc_88f8),
            (13, 0xfedc_ba98_7654_3210),
        ],
        18,
    );
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ArithmeticExpected {
    gpr: [HexU64; 16],
    rflags: HexU64,
    #[serde(default)]
    rflags_defined: Option<HexU64>,
    memory: std::collections::BTreeMap<String, String>,
}

fn arithmetic_fixture_directory() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86/stage2")
}

#[test]
fn arithmetic_fixtures_match_independent_literal_answers_for_every_observable_output() {
    assert_literal_fixture_answers("s2_11_expected.json", 313);
}

#[test]
fn logic_fixtures_match_defined_flags_and_independent_literal_answers() {
    assert_literal_fixture_answers("s2_12_expected.json", 197);
}

#[test]
fn shift_rotate_fixtures_match_literal_boundary_answers() {
    assert_literal_fixture_answers("s2_13_expected.json", 144);
}

#[test]
fn sign_extension_fixtures_match_literal_full_state_answers() {
    assert_literal_fixture_answers("s2_15_sign_extension_expected.json", 6);
}

#[test]
fn bswap_fixtures_match_literal_full_state_answers() {
    assert_literal_fixture_answers("s2_15_bswap_expected.json", 7);
}

#[test]
fn bit_operation_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_expected.json", 12);
}

#[test]
fn bit_immediate_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_immediate_expected.json", 48);
}

#[test]
fn bit_immediate_memory_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_immediate_memory_expected.json", 48);
}

#[test]
fn bit_register_memory_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_bitops_register_memory_expected.json", 72);
}

#[test]
fn bit_scan_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_15_scans_expected.json", 23);
}

#[test]
fn direct_control_flow_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_16a_control_flow_expected.json", 68);
}

#[test]
fn call_ret_fixtures_match_literal_state_nested_targets_faults_and_budget() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17c_call_ret_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 6);
    for (case_id, answer) in answers {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if answer.exception.is_some() {
            assert_eq!(state, before, "{case_id}: no architectural commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn call_and_ret_stack_faults_leave_state_and_memory_uncommitted() {
    let directory = arithmetic_fixture_directory();
    let input = Fixture::from_json(
        &std::fs::read(directory.join("s2_17c_call_forward.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let stack_address = 0x3000f8;
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(6),
        linear_address: Some(stack_address),
    };
    for (rip, access, error) in [
        (
            input.initial_state.rip.get(),
            AccessKind::Write,
            MemoryError::Architectural(fault.clone()),
        ),
        (
            input.initial_state.rip.get() + 10,
            AccessKind::Read,
            MemoryError::Backend("injected RET stack read failure".into()),
        ),
    ] {
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        memory.inject_once(stack_address, access, error.clone());
        let mut state = input.initial_state.to_cpu().unwrap();
        state.rip = rip;
        if access == AccessKind::Read {
            state.gpr[4] = 0x3000f8;
        }
        let before = state.clone();
        let before_memory = memory.snapshot();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{access:?}");
        assert_eq!(memory.snapshot(), before_memory, "{access:?}");
        match error {
            MemoryError::Architectural(fault) => assert_eq!(result.exit, RunExit::Fault(fault)),
            error => assert_eq!(result.exit, RunExit::MemoryError(error)),
        }
    }
}

#[test]
fn indirect_call_fixtures_match_literal_targets_faults_and_budget() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17d_indirect_call_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if answer.exception.is_some() {
            assert_eq!(state, before, "{case_id}: no architectural commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn indirect_memory_call_fixtures_match_literal_full_state_and_fault_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17e_indirect_call_memory_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 9);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_17e_call_mem_read_fault" {
            memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if let Some(expected) = &answer.exception {
            assert_eq!(expected.fault_rip.get(), state.rip, "{case_id}");
            assert_eq!(state, initial, "{case_id}: no CPU commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn ret_imm16_fixtures_match_literal_adjustment_nested_cleanup_and_faults() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17f_ret_imm16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 9);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_17f_ret_imm_read_fault" {
            memory.inject_once(
                0x0030_0100,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0030_0100),
                }),
            );
        }
        if case_id == "s2_17f_ret_imm_truncated" {
            memory.inject_once(
                0x0010_0002,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0002),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if let Some(expected) = &answer.exception {
            assert_eq!(expected.fault_rip.get(), state.rip, "{case_id}");
            assert_eq!(state, initial, "{case_id}: no CPU commit");
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IndirectJumpExpected {
    gpr: [HexU64; 16],
    rip: HexU64,
    rflags: HexU64,
    rflags_defined: HexU64,
    memory: std::collections::BTreeMap<String, String>,
    exit: String,
    exception: Option<IndirectJumpException>,
    instructions_executed: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IndirectJumpException {
    vector: u8,
    error_code: u32,
    #[serde(default)]
    linear_address: Option<HexU64>,
    fault_rip: HexU64,
}

#[test]
fn indirect_jmp_fixtures_reach_new_decode_address_and_preserve_noncanonical_fault_state() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_16d_indirect_jmp_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 3);
    for (case_id, answer) in answers {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(
            state.rflags & input.rflags_mask.get(),
            answer.rflags.get() & input.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if let Some(expected) = &answer.exception {
            assert_eq!(expected.fault_rip.get(), state.rip, "{case_id}");
            assert_eq!(
                state,
                input.initial_state.to_cpu().unwrap(),
                "{case_id}: no commit"
            );
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn indirect_memory_jmp_fixtures_read_qword_before_fault_or_target_decode() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_16e_indirect_jmp_memory_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 7);
    for (case_id, answer) in answers {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_16e_jmp_mem_read_fault" {
            memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(
            state.rflags & input.rflags_mask.get(),
            answer.rflags.get() & input.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            actual.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                }),
                "{case_id}"
            ),
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        if answer.exception.is_some() {
            assert_eq!(
                state, initial,
                "{case_id}: source/target fault must not commit"
            );
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn setcc_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_16b_setcc_expected.json", 79);
}

#[test]
fn cmovcc_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_16c_cmovcc_expected.json", 70);
}

#[test]
fn push_pop_fixtures_match_literal_full_state_and_definedness_answers() {
    assert_literal_fixture_answers("s2_17a_push_pop_expected.json", 8);
}

#[test]
fn push_immediate_fixtures_match_literal_signed_boundary_answers() {
    assert_literal_fixture_answers("s2_17b_push_immediate_expected.json", 8);
}

#[test]
fn push_pop16_fixtures_match_literal_state_alias_and_fault_answers() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17g_push_pop16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 10);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        match case_id.as_str() {
            "s2_17g_push_fault_bottom" => memory.inject_once(
                0x002f_fffe,
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(0x002f_fffe),
                }),
            ),
            "s2_17g_pop_fault_top" => memory.inject_once(
                0x0030_0100,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0030_0100),
                }),
            ),
            "s2_17g_truncated_fetch" => memory.inject_once(
                0x0010_0001,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0001),
                }),
            ),
            _ => {}
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, before, "{case_id}: no CPU commit");
                assert_eq!(state.rip, expected.fault_rip.get(), "{case_id}");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn register_stack_rexw_fixtures_match_literal_width_and_prefix_order_answers() {
    assert_literal_fixture_answers("s2_17n_register_stack_rexw_expected.json", 6);
}

#[test]
fn popfq_fixtures_match_literal_stack_flags_and_definedness_answers() {
    assert_literal_fixture_answers("s2_17o_popfq_expected.json", 2);
}

#[test]
fn popfw_fixtures_match_literal_word_stack_flags_and_answers() {
    assert_literal_fixture_answers("s2_17r_popfw_expected.json", 1);
}

#[test]
fn popfw_preserves_unmasked_rflags_bits_locally() {
    let directory = arithmetic_fixture_directory();
    let raw =
        std::fs::read(directory.join("s2_17r_popfw_updates_allowed_flags.case.json")).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let memory_before = memory.snapshot();
    // The v2 fixture mask does not compare IF, reserved bit 3, ID, or upper
    // RFLAGS bits. Keep IOPL=0 for the admitted profile and assert those bits
    // directly here. The stack source itself carries IOPL=3, which POPFW must
    // leave privileged and unchanged.
    let initial_flags = 2 | (1 << 3) | (1 << 9) | (1 << 21) | (1 << 63);
    state.rflags = initial_flags;
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );

    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let modeled = 0x0cd5;
    assert_eq!(state.rflags, (initial_flags & !modeled) | modeled);
    assert_eq!(
        state.gpr[Register::Rsp as usize],
        fixture.initial_state.gpr[4].get() + 2
    );
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn popfq_out_of_profile_fixture_inputs_return_unsupported_without_commit() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17o_loads_status_flags_and_df.case.json")).unwrap();
    let base = Fixture::from_json(&raw).unwrap().fixture;

    for event_flag in [1_u64 << 8, 1_u64 << 18] {
        let mut fixture = base.clone();
        let stack = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "stack")
            .unwrap();
        let mut bytes = decode_bytes(&stack.initial_bytes_hex).unwrap();
        let offset = (fixture.initial_state.gpr[4].get() - stack.guest_address.get()) as usize;
        bytes[offset..offset + 8].copy_from_slice(&event_flag.to_le_bytes());
        stack.initial_bytes_hex = encode_bytes(&bytes);

        // Round-trip through the public fixture parser to exercise the same input contract.
        let raw = serde_json::to_vec(&fixture).unwrap();
        let fixture = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let memory_before = memory.snapshot();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let state_before = state.clone();
        let actual = run(&mut state, &mut memory, config(1, None), || false);

        assert!(matches!(actual.exit, RunExit::Unsupported { opcode: 0x9d }));
        assert_eq!(state, state_before);
        assert_eq!(memory.snapshot(), memory_before);
    }
}

fn assert_popfw_unsupported_without_commit(stack_flags: u16, initial_flags: u64) {
    let directory = arithmetic_fixture_directory();
    let raw =
        std::fs::read(directory.join("s2_17r_popfw_updates_allowed_flags.case.json")).unwrap();
    let mut fixture = Fixture::from_json(&raw).unwrap().fixture;
    let stack = fixture
        .memory
        .iter_mut()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let mut bytes = decode_bytes(&stack.initial_bytes_hex).unwrap();
    let offset = (fixture.initial_state.gpr[4].get() - stack.guest_address.get()) as usize;
    bytes[offset..offset + 2].copy_from_slice(&stack_flags.to_le_bytes());
    stack.initial_bytes_hex = encode_bytes(&bytes);

    let raw = serde_json::to_vec(&fixture).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    state.rflags = initial_flags;
    let state_before = state.clone();
    let actual = run(&mut state, &mut memory, config(1, None), || false);

    assert!(matches!(actual.exit, RunExit::Unsupported { opcode: 0x9d }));
    assert_eq!(state, state_before);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn popfw_tf_source_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(1 << 8, 2);
}

#[test]
fn popfw_initial_tf_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(0, 2 | (1 << 8));
}

#[test]
fn popfw_nt_source_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(1 << 14, 2);
}

#[test]
fn popfw_initial_nt_returns_unsupported_without_cpu_or_memory_commit() {
    assert_popfw_unsupported_without_commit(0, 2 | (1 << 14));
}

#[test]
fn popfw_nonzero_current_iopl_is_unsupported_before_stack_access() {
    let directory = arithmetic_fixture_directory();
    let raw =
        std::fs::read(directory.join("s2_17r_popfw_updates_allowed_flags.case.json")).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    state.rflags = 2 | (3 << 12);
    let state_before = state.clone();
    let stack_address = state.gpr[Register::Rsp as usize];
    memory.inject_once(
        stack_address,
        AccessKind::Read,
        MemoryError::Unmapped {
            address: stack_address,
            access: AccessKind::Read,
        },
    );
    let result = run(&mut state, &mut memory, config(1, None), || false);

    assert!(matches!(result.exit, RunExit::Unsupported { opcode: 0x9d }));
    assert_eq!(state, state_before);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn pushfq_fixture_matches_literal_masked_rflags_image() {
    assert_literal_fixture_answers("s2_17p_pushfq_expected.json", 1);
}

#[test]
fn pushf_rexw_overrides_66_fixture_matches_qword_round_trip() {
    assert_literal_fixture_answers("s2_17s_pushf_rexw_expected.json", 1);
}

#[test]
fn pushf_rexw_round_trip_preserves_unmasked_rflags_and_full_qword_stack_image() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_17s_pushf_rexw_overrides_66_qword_roundtrip.case.json"
    ))
    .unwrap()
    .fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    // Exercise an allowed POPFQ source state: TF/NT/AC, RF, and IOPL are clear.
    // IF and ID remain outside the fixture mask and must round-trip unchanged.
    state.rflags |= (1 << 9) | (1 << 21);
    let original_flags = state.rflags;
    let original_rsp = state.gpr[Register::Rsp as usize];

    let result = run(
        &mut state,
        &mut memory,
        config(2, fixture.stop_rip.map(HexU64::get)),
        || false,
    );

    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    assert_eq!(state.gpr[Register::Rsp as usize], original_rsp);
    assert_eq!(state.rflags, original_flags);
    let stack_spec = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let stack = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let offset = (original_rsp - 8 - stack_spec.guest_address.get()) as usize;
    let expected_image = (original_flags & 0x00fc_ffff)
        .to_le_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let start = offset * 2;
    assert_eq!(&stack.bytes_hex[start..start + 16], expected_image);
    assert_eq!(&stack.bytes_hex[start - 2..start], "a5");
    assert_eq!(&stack.bytes_hex[start + 16..start + 18], "a5");
}

#[test]
fn pushfw_fixture_matches_literal_low_flags_word() {
    assert_literal_fixture_answers("s2_17q_pushfw_expected.json", 1);
}

#[test]
fn fs_load_accessed_byte_write_spans_adjacent_gdt_regions_without_full_descriptor_writeback() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22f_fs_load_writable_g0.case.json"
    ))
    .unwrap()
    .fixture;
    let gdt = fixture.memory[1].clone();
    let mut bytes = gdt.initial_bytes_hex.clone();
    bytes.replace_range(26..28, "f2");
    let left = crate::case::MemoryRegionSpec {
        region_id: "gdt_before_access_byte".into(),
        guest_address: gdt.guest_address,
        length: 13,
        permissions: MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        },
        initial_bytes_hex: bytes[..26].into(),
        comparison_mask_hex: "ff".repeat(13),
    };
    let right = crate::case::MemoryRegionSpec {
        region_id: "gdt_access_byte".into(),
        guest_address: HexU64(gdt.guest_address.get() + 13),
        length: 1,
        permissions: MemoryPermissions {
            read: true,
            write: true,
            execute: false,
        },
        initial_bytes_hex: bytes[26..28].into(),
        comparison_mask_hex: "ff".into(),
    };
    let tail = crate::case::MemoryRegionSpec {
        region_id: "gdt_after_access_byte".into(),
        guest_address: HexU64(gdt.guest_address.get() + 14),
        length: 2,
        permissions: MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        },
        initial_bytes_hex: bytes[28..].into(),
        comparison_mask_hex: "ff".repeat(2),
    };
    fixture.memory = vec![fixture.memory[0].clone(), left, right, tail];

    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    let snapshot = memory.snapshot();
    assert_eq!(snapshot[1].bytes_hex, bytes[..26]);
    assert_eq!(snapshot[2].bytes_hex, "f3");
    assert_eq!(snapshot[3].bytes_hex, bytes[28..]);

    let mut read_only_fixture = fixture;
    read_only_fixture.memory[2].permissions.write = false;
    let mut denied_memory = FlatMemory::from_fixture(&read_only_fixture).unwrap();
    let mut denied_state = read_only_fixture
        .initial_state
        .to_cpu_for(read_only_fixture.profile)
        .unwrap();
    let before_state = denied_state.clone();
    let before_memory = denied_memory.snapshot();
    let denied = run(
        &mut denied_state,
        &mut denied_memory,
        config(1, read_only_fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(
        denied.exit,
        RunExit::MemoryError(MemoryError::PermissionDenied {
            address: read_only_fixture.initial_state.gdtr_base.unwrap().get() + 13,
            access: AccessKind::Write,
        })
    );
    assert_eq!(denied.instructions_executed, 0);
    assert_eq!(denied_state, before_state);
    assert_eq!(denied_memory.snapshot(), before_memory);
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MovsbExpected {
    gpr: [HexU64; 16],
    rflags: HexU64,
    rflags_defined: HexU64,
    rip: HexU64,
    memory: std::collections::BTreeMap<String, String>,
    fault: Option<MovsbFault>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MovsbFault {
    access: String,
    address: HexU64,
}

struct MovsbAccessLog {
    inner: FlatMemory,
    accesses: Vec<(AccessKind, u64, usize)>,
    mfences: Vec<AccessContext>,
    mfence_error: Option<MemoryError>,
}

impl GuestMemory for MovsbAccessLog {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.accesses.push((AccessKind::Fetch, address, 1));
        self.inner.fetch(address, context)
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.accesses
            .push((AccessKind::Read, address, destination.len()));
        self.inner.read(address, context, destination)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.accesses
            .push((AccessKind::Write, address, source.len()));
        self.inner.write(address, context, source)
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_exchange_integer(address, context, size, replacement)
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_fetch_add_integer(address, context, size, addend)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: od_isa::OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_compare_exchange_integer(address, context, size, expected, replacement)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.mfences.push(context);
        if let Some(error) = &self.mfence_error {
            return Err(error.clone());
        }
        self.inner.mfence(context)
    }
}

#[test]
fn gs_load_accessed_byte_write_spans_adjacent_gdt_regions_without_full_descriptor_writeback() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22h_gs_load_writable_g0.case.json"
    ))
    .unwrap()
    .fixture;
    let gdt = fixture.memory[1].clone();
    let mut bytes = gdt.initial_bytes_hex.clone();
    bytes.replace_range(26..28, "f2");
    let left = crate::case::MemoryRegionSpec {
        region_id: "gdt_before_access_byte".into(),
        guest_address: gdt.guest_address,
        length: 13,
        permissions: MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        },
        initial_bytes_hex: bytes[..26].into(),
        comparison_mask_hex: "ff".repeat(13),
    };
    let right = crate::case::MemoryRegionSpec {
        region_id: "gdt_access_byte".into(),
        guest_address: HexU64(gdt.guest_address.get() + 13),
        length: 1,
        permissions: MemoryPermissions {
            read: true,
            write: true,
            execute: false,
        },
        initial_bytes_hex: bytes[26..28].into(),
        comparison_mask_hex: "ff".into(),
    };
    let tail = crate::case::MemoryRegionSpec {
        region_id: "gdt_after_access_byte".into(),
        guest_address: HexU64(gdt.guest_address.get() + 14),
        length: 2,
        permissions: MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        },
        initial_bytes_hex: bytes[28..].into(),
        comparison_mask_hex: "ff".repeat(2),
    };
    fixture.memory = vec![fixture.memory[0].clone(), left, right, tail];

    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    let snapshot = memory.snapshot();
    assert_eq!(snapshot[1].bytes_hex, bytes[..26]);
    assert_eq!(snapshot[2].bytes_hex, "f3");
    assert_eq!(snapshot[3].bytes_hex, bytes[28..]);

    let mut read_only_fixture = fixture;
    read_only_fixture.memory[2].permissions.write = false;
    let mut denied_memory = FlatMemory::from_fixture(&read_only_fixture).unwrap();
    let mut denied_state = read_only_fixture
        .initial_state
        .to_cpu_for(read_only_fixture.profile)
        .unwrap();
    let before_state = denied_state.clone();
    let before_memory = denied_memory.snapshot();
    let denied = run(
        &mut denied_state,
        &mut denied_memory,
        config(1, read_only_fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(
        denied.exit,
        RunExit::MemoryError(MemoryError::PermissionDenied {
            address: read_only_fixture.initial_state.gdtr_base.unwrap().get() + 13,
            access: AccessKind::Write,
        })
    );
    assert_eq!(denied.instructions_executed, 0);
    assert_eq!(denied_state, before_state);
    assert_eq!(denied_memory.snapshot(), before_memory);
}

#[test]
fn s2_22i_loaded_gs_base_composes_with_exact_qword_load() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22i_gs_base_composition.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = AccessTraceMemory {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        fetch_limit: None,
    };
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );

    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, 0x0010_0006);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_7788);
    for register in 1..state.gpr.len() {
        assert_eq!(
            state.gpr[register],
            fixture.initial_state.gpr[register].get(),
            "non-destination GPR {register} is preserved"
        );
    }
    assert_eq!(state.rflags, fixture.initial_state.rflags.get());
    assert_eq!(
        state.gdtr,
        Some(od_isa::state::Gdtr {
            base: 0x0020_0000,
            limit: 15
        })
    );
    assert_eq!(state.gs_selector, 0x0b);
    assert_eq!(state.gs_base, 0x8000_0000);
    assert_eq!(
        state.gs_cache,
        Some(od_isa::state::FsDescriptorCache {
            limit: 0,
            access: 0xf3,
            flags: 0,
        })
    );
    let observed = memory.inner.snapshot();
    assert_eq!(observed[1].bytes_hex, "00000000000000000000000000f30080");
    assert_eq!(
        observed[2].bytes_hex,
        "0000000000000000000000deadc0decafef00d"
    );
    assert_eq!(
        observed[3].bytes_hex,
        "00000000000000000000008877665544332211"
    );

    let descriptor_context = AccessContext::default().descriptor_table();
    assert_eq!(
        memory
            .accesses
            .iter()
            .copied()
            .filter(|(kind, _, _, _)| *kind != AccessKind::Fetch)
            .collect::<Vec<_>>(),
        vec![
            (AccessKind::Read, 0x0020_0008, 8, descriptor_context),
            (AccessKind::Write, 0x0020_000d, 1, descriptor_context),
            (AccessKind::Read, 0x8000_000b, 8, AccessContext::default()),
        ]
    );
    for (_, _, _, context) in memory
        .accesses
        .iter()
        .filter(|(kind, _, _, _)| *kind != AccessKind::Fetch)
    {
        assert_eq!(context.privilege, od_isa::memory::PrivilegeLevel::Ring3);
    }
    assert_eq!(
        descriptor_context.page_privilege(),
        od_isa::memory::PrivilegeLevel::Ring0
    );
}

#[test]
fn s2_22i_later_gs_data_fault_preserves_load_and_canonicality_fault_skips_read() {
    let source =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22i_gs_base_composition.case.json");
    let mut canonical = Fixture::from_json(source).unwrap().fixture;
    canonical.initial_state.gpr[Register::Rax as usize] = HexU64(0x0000_7fff_8000_000b);

    for (fixture, data_fault) in [
        (canonical, false),
        (Fixture::from_json(source).unwrap().fixture, true),
    ] {
        let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
        let mut inner = FlatMemory::from_fixture(&fixture).unwrap();
        if data_fault {
            inner.inject_once(
                0x8000_000b,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(0x8000_000b),
                }),
            );
        }
        let mut memory = AccessTraceMemory {
            inner,
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let original_rax = state.gpr[Register::Rax as usize];
        let result = run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: if data_fault { 14 } else { 13 },
                error_code: Some(0),
                linear_address: if data_fault { Some(0x8000_000b) } else { None },
            })
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 0x0010_0002);
        assert_eq!(state.gpr[Register::Rax as usize], original_rax);
        assert_eq!(state.gs_selector, 0x0b);
        assert_eq!(state.gs_base, 0x8000_0000);
        assert_eq!(state.gs_cache.unwrap().access, 0xf3);
        assert_eq!(
            memory.inner.snapshot()[1].bytes_hex,
            "00000000000000000000000000f30080"
        );
        assert_eq!(
            memory
                .accesses
                .iter()
                .filter(|(kind, _, _, _)| *kind == AccessKind::Read)
                .count(),
            if data_fault { 2 } else { 1 },
            "only descriptor read plus optional explicit data read are allowed"
        );
        if !data_fault {
            assert!(!memory.accesses.iter().any(|(kind, address, _, context)| {
                *kind == AccessKind::Read
                    && *address == 0x0000_8000_0000_000b
                    && context.origin == od_isa::memory::AccessOrigin::Explicit
            }));
        }
    }
}

#[test]
fn s2_22j_loaded_fs_base_composes_with_exact_qword_load() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22j_fs_base_composition.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = AccessTraceMemory {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        fetch_limit: None,
    };
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );

    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.rip, 0x0010_0006);
    assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_7788);
    for register in 1..state.gpr.len() {
        assert_eq!(
            state.gpr[register],
            fixture.initial_state.gpr[register].get(),
            "non-destination GPR {register} is preserved"
        );
    }
    assert_eq!(state.rflags, fixture.initial_state.rflags.get());
    assert_eq!(
        state.gdtr,
        Some(od_isa::state::Gdtr {
            base: 0x0020_0000,
            limit: 15
        })
    );
    assert_eq!(state.fs_selector, 0x0b);
    assert_eq!(state.fs_base, 0x8000_0000);
    assert_eq!(
        state.fs_cache,
        Some(od_isa::state::FsDescriptorCache {
            limit: 0,
            access: 0xf3,
            flags: 0,
        })
    );
    let observed = memory.inner.snapshot();
    assert_eq!(observed[1].bytes_hex, "00000000000000000000000000f30080");
    assert_eq!(
        observed[2].bytes_hex,
        "0000000000000000000000deadc0decafef00d"
    );
    assert_eq!(
        observed[3].bytes_hex,
        "00000000000000000000008877665544332211"
    );

    let descriptor_context = AccessContext::default().descriptor_table();
    assert_eq!(
        memory
            .accesses
            .iter()
            .copied()
            .filter(|(kind, _, _, _)| *kind != AccessKind::Fetch)
            .collect::<Vec<_>>(),
        vec![
            (AccessKind::Read, 0x0020_0008, 8, descriptor_context),
            (AccessKind::Write, 0x0020_000d, 1, descriptor_context),
            (AccessKind::Read, 0x8000_000b, 8, AccessContext::default()),
        ]
    );
    for (_, _, _, context) in memory
        .accesses
        .iter()
        .filter(|(kind, _, _, _)| *kind != AccessKind::Fetch)
    {
        assert_eq!(context.privilege, od_isa::memory::PrivilegeLevel::Ring3);
    }
    assert_eq!(
        descriptor_context.page_privilege(),
        od_isa::memory::PrivilegeLevel::Ring0
    );
}

#[test]
fn s2_22j_later_fs_data_fault_preserves_load_and_canonicality_fault_skips_read() {
    let source =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_22j_fs_base_composition.case.json");
    let mut canonical = Fixture::from_json(source).unwrap().fixture;
    canonical.initial_state.gpr[Register::Rax as usize] = HexU64(0x0000_7fff_8000_000b);

    for (fixture, data_fault) in [
        (canonical, false),
        (Fixture::from_json(source).unwrap().fixture, true),
    ] {
        let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
        let mut inner = FlatMemory::from_fixture(&fixture).unwrap();
        if data_fault {
            inner.inject_once(
                0x8000_000b,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(0),
                    linear_address: Some(0x8000_000b),
                }),
            );
        }
        let mut memory = AccessTraceMemory {
            inner,
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let original_rax = state.gpr[Register::Rax as usize];
        let result = run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: if data_fault { 14 } else { 13 },
                error_code: Some(0),
                linear_address: if data_fault { Some(0x8000_000b) } else { None },
            })
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 0x0010_0002);
        assert_eq!(state.gpr[Register::Rax as usize], original_rax);
        assert_eq!(state.fs_selector, 0x0b);
        assert_eq!(state.fs_base, 0x8000_0000);
        assert_eq!(state.fs_cache.unwrap().access, 0xf3);
        assert_eq!(
            memory.inner.snapshot()[1].bytes_hex,
            "00000000000000000000000000f30080"
        );
        assert_eq!(
            memory
                .accesses
                .iter()
                .filter(|(kind, _, _, _)| *kind == AccessKind::Read)
                .count(),
            if data_fault { 2 } else { 1 },
            "only descriptor read plus optional explicit data read are allowed"
        );
        if !data_fault {
            assert!(!memory.accesses.iter().any(|(kind, address, _, context)| {
                *kind == AccessKind::Read
                    && *address == 0x0000_8000_0000_000b
                    && context.origin == od_isa::memory::AccessOrigin::Explicit
            }));
        }
    }
}

#[test]
fn mfence_uses_one_backend_completion_call_and_failure_does_not_retire_it() {
    let directory = arithmetic_fixture_directory();
    let input = Fixture::from_json(
        &std::fs::read(directory.join("s2_21g_mfence_store_fence_load.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let context = AccessContext::default();

    // Begin directly at MFENCE to prove it fetches only its encoding and
    // performs no operand or data-memory access of its own.
    let mut state = input.initial_state.to_cpu().unwrap();
    state.rip += 3;
    let before = state.clone();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 3);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.mfences, [context]);
    assert_eq!(
        memory.accesses,
        [
            (AccessKind::Fetch, before.rip, 1),
            (AccessKind::Fetch, before.rip + 1, 1),
            (AccessKind::Fetch, before.rip + 2, 1),
        ]
    );

    // If the fence backend rejects completion, it does not retire or roll
    // back the store that completed in the preceding instruction.
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: Some(MemoryError::Backend("fence unsupported".into())),
    };
    let result = run(&mut state, &mut memory, config(3, None), || false);
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::Backend("fence unsupported".into()))
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, input.initial_state.rip.get() + 3);
    assert_eq!(state.gpr, input.initial_state.gpr.map(HexU64::get));
    assert_eq!(state.rflags, input.initial_state.rflags.get());
    assert_eq!(memory.mfences, [context]);
    assert_eq!(memory.inner.snapshot()[1].bytes_hex, "8877665544332211");
    assert_eq!(
        memory.accesses.last(),
        Some(&(AccessKind::Fetch, input.initial_state.rip.get() + 5, 1))
    );
}

#[test]
fn movsb_fixtures_match_literal_state_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18a_movsb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSB injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        match answer.fault.as_ref() {
            Some(fault) => {
                assert_eq!(
                    actual.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                        linear_address: Some(fault.address.get()),
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
                // This checks this executor's no-commit policy. Intel SDM 093
                // does not establish the destination-#PF restart snapshot;
                // see ISSUE-016 before treating it as a hardware guarantee.
                assert_eq!(state, original_state, "{case_id}");
                assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
                let expected_accesses = if fault.access == "read" {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 1),
                    ]
                } else {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 1),
                        (AccessKind::Write, destination, 1),
                    ]
                };
                assert_eq!(memory.accesses, expected_accesses, "{case_id}");
            }
            None => {
                assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
                assert_eq!(actual.instructions_executed, 1, "{case_id}");
                assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
                assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
                assert_eq!(state.rip, answer.rip.get(), "{case_id}");
                assert_eq!(
                    actual.rflags_defined,
                    answer.rflags_defined.get(),
                    "{case_id}"
                );
                for region in memory.inner.snapshot() {
                    assert_eq!(
                        region.bytes_hex.as_str(),
                        answer.memory.get(&region.region_id).unwrap().as_str(),
                        "{case_id}: {}",
                        region.region_id
                    );
                }
                assert_eq!(
                    memory.accesses,
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 1),
                        (AccessKind::Write, destination, 1),
                    ],
                    "{case_id}"
                );
            }
        }
    }
}

#[test]
fn movsw_fixtures_match_literal_state_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18b_movsw_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSW injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        match answer.fault.as_ref() {
            Some(fault) => {
                assert_eq!(
                    actual.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                        linear_address: Some(fault.address.get()),
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
                // This checks the executor's local no-commit policy only. The
                // hardware restart snapshot after MOVSW destination #PF is
                // not specified by Intel SDM 093; see ISSUE-016.
                assert_eq!(state, original_state, "{case_id}");
                assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
                let expected_accesses = if fault.access == "read" {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Fetch, 0x0010_0001, 1),
                        (AccessKind::Read, source, 2),
                    ]
                } else {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Fetch, 0x0010_0001, 1),
                        (AccessKind::Read, source, 2),
                        (AccessKind::Write, destination, 2),
                    ]
                };
                assert_eq!(memory.accesses, expected_accesses, "{case_id}");
            }
            None => {
                assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
                assert_eq!(actual.instructions_executed, 1, "{case_id}");
                assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
                assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
                assert_eq!(state.rip, answer.rip.get(), "{case_id}");
                assert_eq!(
                    actual.rflags_defined,
                    answer.rflags_defined.get(),
                    "{case_id}"
                );
                for region in memory.inner.snapshot() {
                    assert_eq!(
                        region.bytes_hex.as_str(),
                        answer.memory.get(&region.region_id).unwrap().as_str(),
                        "{case_id}: {}",
                        region.region_id
                    );
                }
                assert_eq!(
                    memory.accesses,
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Fetch, 0x0010_0001, 1),
                        (AccessKind::Read, source, 2),
                        (AccessKind::Write, destination, 2),
                    ],
                    "{case_id}"
                );
            }
        }
    }
}

#[test]
fn movsd_fixtures_match_literal_state_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18c_movsd_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSD injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        match answer.fault.as_ref() {
            Some(fault) => {
                assert_eq!(
                    actual.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                        linear_address: Some(fault.address.get()),
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
                // This checks the executor's local no-commit policy only. Intel
                // SDM 093 does not specify the MOVSD destination-#PF restart
                // snapshot; see ISSUE-016.
                assert_eq!(state, original_state, "{case_id}");
                assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
                let expected_accesses = if fault.access == "read" {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 4),
                    ]
                } else {
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 4),
                        (AccessKind::Write, destination, 4),
                    ]
                };
                assert_eq!(memory.accesses, expected_accesses, "{case_id}");
            }
            None => {
                assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
                assert_eq!(actual.instructions_executed, 1, "{case_id}");
                assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
                assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
                assert_eq!(state.rip, answer.rip.get(), "{case_id}");
                assert_eq!(
                    actual.rflags_defined,
                    answer.rflags_defined.get(),
                    "{case_id}"
                );
                for region in memory.inner.snapshot() {
                    assert_eq!(
                        region.bytes_hex.as_str(),
                        answer.memory.get(&region.region_id).unwrap().as_str(),
                        "{case_id}: {}",
                        region.region_id
                    );
                }
                assert_eq!(
                    memory.accesses,
                    vec![
                        (AccessKind::Fetch, 0x0010_0000, 1),
                        (AccessKind::Read, source, 4),
                        (AccessKind::Write, destination, 4),
                    ],
                    "{case_id}"
                );
            }
        }
    }
}

#[test]
fn movsq_fixtures_match_literal_state_and_exact_qword_accesses() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18d_movsq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 7);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            let access = match fault.access.as_str() {
                "read" => AccessKind::Read,
                "write" => AccessKind::Write,
                _ => panic!("unknown MOVSQ injected access kind: {}", fault.access),
            };
            memory.inner.inject_once(
                fault.address.get(),
                access,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let source = original_state.gpr[Register::Rsi as usize];
        let destination = original_state.gpr[Register::Rdi as usize];
        let mut expected_accesses: Vec<_> = (0..input.memory[0].length)
            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + u64::from(offset), 1))
            .collect();
        expected_accesses.push((AccessKind::Read, source, 8));
        if answer
            .fault
            .as_ref()
            .is_none_or(|fault| fault.access == "write")
        {
            expected_accesses.push((AccessKind::Write, destination, 8));
        }
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");

        if let Some(fault) = &answer.fault {
            assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(if fault.access == "write" { 6 } else { 4 }),
                    linear_address: Some(fault.address.get()),
                }),
                "{case_id}"
            );
            assert_eq!(actual.instructions_executed, 0, "{case_id}");
            // Local bounded-memory policy only. SDM 093 leaves the hardware
            // MOVSQ destination-#PF restart snapshot unspecified; see ISSUE-016.
            assert_eq!(state, original_state, "{case_id}");
            assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
        } else {
            assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
            assert_eq!(actual.instructions_executed, 1, "{case_id}");
            assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(
                actual.rflags_defined,
                answer.rflags_defined.get(),
                "{case_id}"
            );
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }
    }
}

#[test]
fn stosb_fixtures_match_literal_state_and_write_only_accesses() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18e_stosb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 4);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original_state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        if let Some(fault) = &answer.fault {
            assert_eq!(fault.access, "write", "{case_id}");
            memory.inner.inject_once(
                fault.address.get(),
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let initial_memory = memory.inner.snapshot();
        let mut state = original_state.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let destination = original_state.gpr[Register::Rdi as usize];
        let code_len = input.memory[0].length;
        let expected_accesses = (0..code_len)
            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + u64::from(offset), 1))
            .chain(std::iter::once((AccessKind::Write, destination, 1)))
            .collect::<Vec<_>>();
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");

        if let Some(fault) = &answer.fault {
            assert_eq!(
                actual.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(fault.address.get()),
                }),
                "{case_id}"
            );
            assert_eq!(actual.instructions_executed, 0, "{case_id}");
            // This is the local injected-backend no-commit policy. The
            // hardware restart snapshot for STOSB destination #PF is not
            // established by these tests (ISSUE-016 tracks string faults).
            assert_eq!(state, original_state, "{case_id}");
            assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
        } else {
            assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
            assert_eq!(actual.instructions_executed, 1, "{case_id}");
            assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(
                actual.rflags_defined,
                answer.rflags_defined.get(),
                "{case_id}"
            );
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }
    }
}

#[test]
fn stosb_rejects_prefixes_without_committing() {
    let directory = arithmetic_fixture_directory();
    let mut fixture =
        Fixture::from_json(&std::fs::read(directory.join("s2_18e_stosb_df0.case.json")).unwrap())
            .unwrap()
            .fixture;
    for bytes in ["40aa", "66aa", "2eaa", "64aa", "f2aa"] {
        fixture.memory[0].length = (bytes.len() / 2) as u32;
        fixture.memory[0].initial_bytes_hex = bytes.into();
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(0x0010_0000 + (bytes.len() / 2) as u64));
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let initial_memory = memory.snapshot();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let initial_state = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0xaa },
            "{bytes}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes}");
        assert_eq!(state, initial_state, "{bytes}");
        assert_eq!(memory.snapshot(), initial_memory, "{bytes}");
    }
}

#[test]
fn stos_word_dword_qword_success_matrix_is_write_only_and_preserves_full_state() {
    const FLAG_BITS: [u64; 6] = [1 << 0, 1 << 2, 1 << 4, 1 << 6, 1 << 7, 1 << 11];
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let sizes = [
        (OperandSize::Byte, vec![0xaa]),
        (OperandSize::Word, vec![0x66, 0xab]),
        (OperandSize::Dword, vec![0xab]),
        (OperandSize::Qword, vec![0x48, 0xab]),
    ];
    let values = [
        0,
        0xffff_ffff_ffff_ffff,
        0x0123_4567_89ab_cdef,
        0x8877_6655_4433_2211,
        0x8000_0000_0000_0001,
        0xdead_beef_cafe_babe,
    ];
    let mut cases = 0;
    for (size, code) in sizes {
        for direction in [false, true] {
            for value in values {
                for arithmetic in 0..64 {
                    for background in [0x5a, 0xc3] {
                        let mut fixture = template.clone();
                        fixture.memory[0].length = code.len() as u32;
                        fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                        fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                        fixture.memory[1].initial_bytes_hex =
                            format!("{:02x}", background).repeat(16);
                        fixture.initial_state.rip = HexU64(0x0010_0000);
                        fixture.initial_state.gpr[Register::Rax as usize] = HexU64(value);
                        fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0004);
                        fixture.initial_state.gpr[Register::Rsi as usize] =
                            HexU64(0x0000_0000_0030_0080);
                        fixture.initial_state.gpr[Register::Rcx as usize] =
                            HexU64(0x8877_6655_4433_2211);
                        fixture.initial_state.rflags = HexU64(
                            2 | FLAG_BITS
                                .iter()
                                .enumerate()
                                .filter(|(bit, _)| arithmetic & (1 << bit) != 0)
                                .fold(0, |flags, (_, flag)| flags | flag)
                                | if direction { 1 << 10 } else { 0 },
                        );
                        fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                        let before = fixture.initial_state.to_cpu().unwrap();
                        let mut state = before.clone();
                        let mut memory = MovsbAccessLog {
                            inner: FlatMemory::from_fixture(&fixture).unwrap(),
                            accesses: Vec::new(),
                            mfences: Vec::new(),
                            mfence_error: None,
                        };
                        let start = before.gpr[Register::Rdi as usize];
                        let mut expected_data = vec![background; 16];
                        let stored = value.to_le_bytes();
                        expected_data[4..4 + size.bytes()].copy_from_slice(&stored[..size.bytes()]);
                        let mut expected = before.clone();
                        expected.gpr[Register::Rdi as usize] = if direction {
                            start.wrapping_sub(size.bytes() as u64)
                        } else {
                            start.wrapping_add(size.bytes() as u64)
                        };
                        expected.rip += code.len() as u64;
                        let result = run(
                            &mut state,
                            &mut memory,
                            config(1, fixture.stop_rip.map(HexU64::get)),
                            || false,
                        );
                        let accesses = (0..code.len())
                            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1))
                            .chain(std::iter::once((AccessKind::Write, start, size.bytes())))
                            .collect::<Vec<_>>();
                        assert_eq!(result.exit, RunExit::StopAddress);
                        assert_eq!(result.instructions_executed, 1);
                        assert_eq!(state, expected);
                        assert_eq!(memory.accesses, accesses);
                        assert_eq!(
                            memory.inner.snapshot()[1].bytes_hex,
                            encode_bytes(&expected_data)
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 6_144);
}

#[test]
fn scas_all_widths_full_state_matrix_uses_independent_cmp_flags_and_one_read() {
    const FLAGS: [u64; 6] = [1, 4, 16, 64, 128, 2048];
    const ARITHMETIC_MASK: u64 = 0x8d5;
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms = [
        (OperandSize::Byte, vec![0xae]),
        (OperandSize::Word, vec![0x66, 0xaf]),
        (OperandSize::Dword, vec![0xaf]),
        (OperandSize::Qword, vec![0x48, 0xaf]),
    ];
    let mut cases = 0;
    for (size, code) in forms {
        let mask = size.mask();
        let sign = 1u64 << (size.bytes() * 8 - 1);
        let pairs = [
            (0, 0),
            (1, 1),
            (mask, mask),
            (0, 1),
            (1, 0),
            (sign, 1),
            (sign - 1, mask),
            (mask, sign - 1),
            (sign - 1, sign),
            (sign, sign - 1),
            (0x10, 1),
            (1, 0x10),
        ];
        for direction in [false, true] {
            for (lhs, rhs) in pairs {
                for incoming in 0..64u64 {
                    let result = lhs.wrapping_sub(rhs) & mask;
                    let expected_arithmetic = (u64::from(lhs < rhs) * FLAGS[0])
                        | (u64::from((result as u8).count_ones() % 2 == 0) * FLAGS[1])
                        | (u64::from((lhs ^ rhs ^ result) & 0x10 != 0) * FLAGS[2])
                        | (u64::from(result == 0) * FLAGS[3])
                        | (u64::from(result & sign != 0) * FLAGS[4])
                        | (u64::from((lhs ^ rhs) & (lhs ^ result) & sign != 0) * FLAGS[5]);
                    let mut fixture = template.clone();
                    fixture.memory[0].length = code.len() as u32;
                    fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                    fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                    fixture.memory[1].initial_bytes_hex = "5a".repeat(16);
                    fixture.initial_state.rip = HexU64(0x0010_0000);
                    fixture.initial_state.gpr[Register::Rax as usize] =
                        HexU64(0xa5a5_5a5a_1234_5678 & !mask | lhs);
                    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0004);
                    fixture.initial_state.rflags = HexU64(
                        2 | if direction { 1 << 10 } else { 0 }
                            | FLAGS
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| incoming & (1 << i) != 0)
                                .fold(0, |bits, (_, flag)| bits | flag),
                    );
                    fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                    let before = fixture.initial_state.to_cpu().unwrap();
                    let mut state = before.clone();
                    let mut memory = MovsbAccessLog {
                        inner: FlatMemory::from_fixture(&fixture).unwrap(),
                        accesses: Vec::new(),
                        mfences: Vec::new(),
                        mfence_error: None,
                    };
                    // Place the independent RHS at the pre-state RDI.
                    memory
                        .inner
                        .write_integer(0x0020_0004, AccessContext::default(), size, rhs)
                        .unwrap();
                    let before_memory = memory.inner.snapshot();
                    let mut expected = before.clone();
                    expected.rflags = (before.rflags & !ARITHMETIC_MASK) | expected_arithmetic;
                    expected.gpr[Register::Rdi as usize] = if direction {
                        0x0020_0004u64.wrapping_sub(size.bytes() as u64)
                    } else {
                        0x0020_0004u64.wrapping_add(size.bytes() as u64)
                    };
                    expected.rip += code.len() as u64;
                    let result_run = run(
                        &mut state,
                        &mut memory,
                        config(1, fixture.stop_rip.map(HexU64::get)),
                        || false,
                    );
                    assert_eq!(result_run.exit, RunExit::StopAddress);
                    assert_eq!(result_run.instructions_executed, 1);
                    assert_eq!(state, expected);
                    assert_eq!(result_run.rflags_defined, 0x0cd7);
                    let expected_accesses = (0..code.len())
                        .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1))
                        .chain(std::iter::once((
                            AccessKind::Read,
                            0x0020_0004,
                            size.bytes(),
                        )))
                        .collect::<Vec<_>>();
                    assert_eq!(memory.accesses, expected_accesses);
                    assert_eq!(memory.inner.snapshot(), before_memory);
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 6_144);
}

#[test]
fn scas_address32_full_state_matrix_uses_independent_cmp_flags_and_truncated_read() {
    const FLAGS: [u64; 6] = [1, 4, 16, 64, 128, 2048];
    const ARITHMETIC_MASK: u64 = 0x8d5;
    const HIGH_HALVES: [u64; 3] = [0, 0x1234_5678_0000_0000, 0xffff_ffff_0000_0000];
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms = [
        (OperandSize::Byte, vec![0x67, 0xae]),
        (OperandSize::Word, vec![0x67, 0x66, 0xaf]),
        (OperandSize::Dword, vec![0x67, 0xaf]),
        (OperandSize::Qword, vec![0x67, 0x48, 0xaf]),
    ];
    let mut cases = 0;
    for (size, code) in forms {
        let mask = size.mask();
        let sign = 1u64 << (size.bytes() * 8 - 1);
        let pairs = [
            (0, 0),
            (1, 1),
            (mask, mask),
            (0, 1),
            (1, 0),
            (sign, 1),
            (sign - 1, mask),
            (mask, sign - 1),
            (sign - 1, sign),
            (sign, sign - 1),
            (0x10 & mask, 1),
            (1, 0x10 & mask),
        ];
        for (lhs, rhs) in pairs {
            for direction in [false, true] {
                for incoming in 0..64u64 {
                    let result = lhs.wrapping_sub(rhs) & mask;
                    let expected_arithmetic = (u64::from(lhs < rhs) * FLAGS[0])
                        | (u64::from((result as u8).count_ones() % 2 == 0) * FLAGS[1])
                        | (u64::from((lhs ^ rhs ^ result) & 0x10 != 0) * FLAGS[2])
                        | (u64::from(result == 0) * FLAGS[3])
                        | (u64::from(result & sign != 0) * FLAGS[4])
                        | (u64::from((lhs ^ rhs) & (lhs ^ result) & sign != 0) * FLAGS[5]);
                    let low_rdi = 0x0020_0004u64;
                    for high_half in HIGH_HALVES {
                        let start_rdi = high_half | low_rdi;
                        let mut fixture = template.clone();
                        fixture.memory[0].length = code.len() as u32;
                        fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                        fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                        fixture.memory[1].initial_bytes_hex = "5a".repeat(16);
                        fixture.initial_state.rip = HexU64(0x0010_0000);
                        fixture.initial_state.gpr[Register::Rax as usize] =
                            HexU64(0xa5a5_5a5a_1234_5678 & !mask | lhs);
                        fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(start_rdi);
                        fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(u64::MAX);
                        fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(0);
                        fixture.initial_state.rflags = HexU64(
                            2 | if direction { 1 << 10 } else { 0 }
                                | FLAGS
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, _)| incoming & (1 << i) != 0)
                                    .fold(0, |bits, (_, flag)| bits | flag),
                        );
                        fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                        let before = fixture.initial_state.to_cpu().unwrap();
                        let mut state = before.clone();
                        let mut memory = MovsbAccessLog {
                            inner: FlatMemory::from_fixture(&fixture).unwrap(),
                            accesses: Vec::new(),
                            mfences: Vec::new(),
                            mfence_error: None,
                        };
                        memory
                            .inner
                            .write_integer(low_rdi, AccessContext::default(), size, rhs)
                            .unwrap();
                        let before_memory = memory.inner.snapshot();
                        let mut expected = before.clone();
                        expected.rflags = (before.rflags & !ARITHMETIC_MASK) | expected_arithmetic;
                        let updated = if direction {
                            (low_rdi as u32).wrapping_sub(size.bytes() as u32)
                        } else {
                            (low_rdi as u32).wrapping_add(size.bytes() as u32)
                        };
                        expected.gpr[Register::Rdi as usize] = u64::from(updated);
                        expected.rip += code.len() as u64;
                        let result_run = run(
                            &mut state,
                            &mut memory,
                            config(1, fixture.stop_rip.map(HexU64::get)),
                            || false,
                        );
                        assert_eq!(result_run.exit, RunExit::StopAddress);
                        assert_eq!(result_run.instructions_executed, 1);
                        assert_eq!(state, expected);
                        assert_eq!(result_run.rflags_defined, 0x0cd7);
                        assert_eq!(
                            memory.accesses,
                            (0..code.len())
                                .map(|offset| {
                                    (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1)
                                })
                                .chain(std::iter::once((AccessKind::Read, low_rdi, size.bytes())))
                                .collect::<Vec<_>>()
                        );
                        assert_eq!(memory.inner.snapshot(), before_memory);
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 18_432);
}

#[test]
fn cmps_all_widths_full_state_matrix_uses_ordered_reads_and_independent_cmp_flags() {
    const FLAGS: [u64; 6] = [1, 4, 16, 64, 128, 2048];
    const ARITHMETIC_MASK: u64 = 0x8d5;
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms = [
        (OperandSize::Byte, vec![0xa6]),
        (OperandSize::Word, vec![0x66, 0xa7]),
        (OperandSize::Dword, vec![0xa7]),
        (OperandSize::Qword, vec![0x48, 0xa7]),
    ];
    let mut cases = 0;
    for (size, code) in forms {
        let mask = size.mask();
        let sign = 1u64 << (size.bytes() * 8 - 1);
        let pairs = [
            (0, 0),
            (1, 1),
            (mask, mask),
            (0, 1),
            (1, 0),
            (sign, 1),
            (sign - 1, mask),
            (mask, sign - 1),
            (sign - 1, sign),
            (sign, sign - 1),
            (0x10, 1),
            (1, 0x10),
        ];
        for direction in [false, true] {
            for (lhs, rhs) in pairs {
                for incoming in 0..64u64 {
                    let value = lhs.wrapping_sub(rhs) & mask;
                    let expected_arithmetic = (u64::from(lhs < rhs) * FLAGS[0])
                        | (u64::from((value as u8).count_ones() % 2 == 0) * FLAGS[1])
                        | (u64::from((lhs ^ rhs ^ value) & 0x10 != 0) * FLAGS[2])
                        | (u64::from(value == 0) * FLAGS[3])
                        | (u64::from(value & sign != 0) * FLAGS[4])
                        | (u64::from((lhs ^ rhs) & (lhs ^ value) & sign != 0) * FLAGS[5]);
                    let mut fixture = template.clone();
                    fixture.memory[0].length = code.len() as u32;
                    fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                    fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                    fixture.memory[1].initial_bytes_hex = "5a".repeat(16);
                    fixture.initial_state.rip = HexU64(0x0010_0000);
                    fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
                    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0008);
                    fixture.initial_state.rflags = HexU64(
                        2 | if direction { 1 << 10 } else { 0 }
                            | FLAGS
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| incoming & (1 << i) != 0)
                                .fold(0, |bits, (_, flag)| bits | flag),
                    );
                    fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                    let before = fixture.initial_state.to_cpu().unwrap();
                    let mut state = before.clone();
                    let mut memory = MovsbAccessLog {
                        inner: FlatMemory::from_fixture(&fixture).unwrap(),
                        accesses: Vec::new(),
                        mfences: Vec::new(),
                        mfence_error: None,
                    };
                    memory
                        .inner
                        .write_integer(0x0020_0000, AccessContext::default(), size, lhs)
                        .unwrap();
                    memory
                        .inner
                        .write_integer(0x0020_0008, AccessContext::default(), size, rhs)
                        .unwrap();
                    let before_memory = memory.inner.snapshot();
                    let mut expected = before.clone();
                    expected.rflags = (before.rflags & !ARITHMETIC_MASK) | expected_arithmetic;
                    let stride = if direction {
                        0u64.wrapping_sub(size.bytes() as u64)
                    } else {
                        size.bytes() as u64
                    };
                    expected.gpr[Register::Rsi as usize] = 0x0020_0000u64.wrapping_add(stride);
                    expected.gpr[Register::Rdi as usize] = 0x0020_0008u64.wrapping_add(stride);
                    expected.rip += code.len() as u64;
                    let run_result = run(
                        &mut state,
                        &mut memory,
                        config(1, fixture.stop_rip.map(HexU64::get)),
                        || false,
                    );
                    assert_eq!(run_result.exit, RunExit::StopAddress);
                    assert_eq!(run_result.instructions_executed, 1);
                    assert_eq!(state, expected);
                    assert_eq!(run_result.rflags_defined, 0x0cd7);
                    assert_eq!(
                        memory.accesses,
                        (0..code.len())
                            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1))
                            .chain([
                                (AccessKind::Read, 0x0020_0000, size.bytes()),
                                (AccessKind::Read, 0x0020_0008, size.bytes())
                            ])
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(memory.inner.snapshot(), before_memory);
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 6_144);
}

#[test]
fn cmps_addr32_full_state_matrix_truncates_both_indices_and_uses_independent_cmp_flags() {
    const FLAGS: [u64; 6] = [1, 4, 16, 64, 128, 2048];
    const ARITHMETIC_MASK: u64 = 0x8d5;
    const HIGH_HALVES: [u64; 3] = [0, 0x1234_5678_0000_0000, 0xffff_ffff_0000_0000];
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms = [
        (OperandSize::Byte, vec![0x67, 0xa6]),
        (OperandSize::Word, vec![0x67, 0x66, 0xa7]),
        (OperandSize::Dword, vec![0x67, 0xa7]),
        (OperandSize::Qword, vec![0x67, 0x48, 0xa7]),
    ];
    let mut cases = 0;
    for (size, code) in forms {
        let mask = size.mask();
        let sign = 1u64 << (size.bytes() * 8 - 1);
        let pairs = [
            (0, 0),
            (1, 1),
            (mask, mask),
            (0, 1),
            (1, 0),
            (sign, 1),
            (sign - 1, mask),
            (mask, sign - 1),
            (sign - 1, sign),
            (sign, sign - 1),
            (0x10, 1),
            (1, 0x10),
        ];
        for direction in [false, true] {
            for (lhs, rhs) in pairs {
                for incoming in 0..64u64 {
                    let value = lhs.wrapping_sub(rhs) & mask;
                    let expected_arithmetic = (u64::from(lhs < rhs) * FLAGS[0])
                        | (u64::from((value as u8).count_ones() % 2 == 0) * FLAGS[1])
                        | (u64::from((lhs ^ rhs ^ value) & 0x10 != 0) * FLAGS[2])
                        | (u64::from(value == 0) * FLAGS[3])
                        | (u64::from(value & sign != 0) * FLAGS[4])
                        | (u64::from((lhs ^ rhs) & (lhs ^ value) & sign != 0) * FLAGS[5]);
                    for source_high in HIGH_HALVES {
                        for destination_high in HIGH_HALVES {
                            let mut fixture = template.clone();
                            fixture.memory[0].length = code.len() as u32;
                            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                            fixture.memory[1].initial_bytes_hex = "5a".repeat(16);
                            fixture.initial_state.rip = HexU64(0x0010_0000);
                            fixture.initial_state.gpr[Register::Rsi as usize] =
                                HexU64(source_high | 0x0020_0000);
                            fixture.initial_state.gpr[Register::Rdi as usize] =
                                HexU64(destination_high | 0x0020_0008);
                            fixture.initial_state.rflags = HexU64(
                                2 | if direction { 1 << 10 } else { 0 }
                                    | FLAGS
                                        .iter()
                                        .enumerate()
                                        .filter(|(index, _)| incoming & (1 << index) != 0)
                                        .fold(0, |bits, (_, flag)| bits | flag),
                            );
                            fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                            let before = fixture.initial_state.to_cpu().unwrap();
                            let mut state = before.clone();
                            let mut memory = MovsbAccessLog {
                                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                                accesses: Vec::new(),
                                mfences: Vec::new(),
                                mfence_error: None,
                            };
                            memory
                                .inner
                                .write_integer(0x0020_0000, AccessContext::default(), size, lhs)
                                .unwrap();
                            memory
                                .inner
                                .write_integer(0x0020_0008, AccessContext::default(), size, rhs)
                                .unwrap();
                            let before_memory = memory.inner.snapshot();
                            let mut expected = before.clone();
                            expected.rflags =
                                (before.rflags & !ARITHMETIC_MASK) | expected_arithmetic;
                            let source_low = 0x0020_0000u32;
                            let destination_low = 0x0020_0008u32;
                            let step = size.bytes() as u32;
                            let (source_index, destination_index) = if direction {
                                (
                                    source_low.wrapping_sub(step),
                                    destination_low.wrapping_sub(step),
                                )
                            } else {
                                (
                                    source_low.wrapping_add(step),
                                    destination_low.wrapping_add(step),
                                )
                            };
                            expected.gpr[Register::Rsi as usize] = u64::from(source_index);
                            expected.gpr[Register::Rdi as usize] = u64::from(destination_index);
                            expected.rip += code.len() as u64;
                            let run_result = run(
                                &mut state,
                                &mut memory,
                                config(1, fixture.stop_rip.map(HexU64::get)),
                                || false,
                            );
                            assert_eq!(run_result.exit, RunExit::StopAddress);
                            assert_eq!(run_result.instructions_executed, 1);
                            assert_eq!(state, expected);
                            assert_eq!(run_result.rflags_defined, 0x0cd7);
                            assert_eq!(
                                memory.accesses,
                                (0..code.len())
                                    .map(|offset| {
                                        (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1)
                                    })
                                    .chain([
                                        (AccessKind::Read, 0x0020_0000, size.bytes()),
                                        (AccessKind::Read, 0x0020_0008, size.bytes()),
                                    ])
                                    .collect::<Vec<_>>()
                            );
                            assert_eq!(memory.inner.snapshot(), before_memory);
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 55_296);
}

#[test]
fn cmps_addr32_boundary_matrix_keeps_each_operand_contiguous_and_wraps_both_indices() {
    const HIGH_HALVES: [(u64, u64); 3] = [
        (0, 0),
        (0x1234_5678_0000_0000, 0),
        (0, 0xffff_ffff_0000_0000),
    ];
    const BOUNDARIES: [u32; 4] = [0, 0x1fff, 0xffff_fffc, 0xffff_ffff];
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms = [
        (OperandSize::Byte, vec![0x67, 0xa6]),
        (OperandSize::Word, vec![0x67, 0x66, 0xa7]),
        (OperandSize::Dword, vec![0x67, 0xa7]),
        (OperandSize::Qword, vec![0x67, 0x48, 0xa7]),
    ];
    let mut cases = 0;
    for (size, code) in forms {
        for direction in [false, true] {
            let mut configurations = Vec::new();
            for boundary in BOUNDARIES {
                configurations.push((boundary, 0x0020_0008));
                configurations.push((0x0020_0000, boundary));
            }
            for (source_address, destination_address) in configurations {
                for (source_high, destination_high) in HIGH_HALVES {
                    let mut fixture = template.clone();
                    fixture.memory[0].length = code.len() as u32;
                    fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                    fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                    let source = &mut fixture.memory[1];
                    source.guest_address = HexU64(u64::from(source_address));
                    source.length = size.bytes() as u32;
                    source.initial_bytes_hex = "55".repeat(size.bytes());
                    source.comparison_mask_hex = "ff".repeat(size.bytes());
                    let mut destination = source.clone();
                    destination.region_id = "destination".into();
                    destination.guest_address = HexU64(u64::from(destination_address));
                    fixture.memory.push(destination);
                    fixture.initial_state.rip = HexU64(0x0010_0000);
                    fixture.initial_state.gpr[Register::Rsi as usize] =
                        HexU64(source_high | u64::from(source_address));
                    fixture.initial_state.gpr[Register::Rdi as usize] =
                        HexU64(destination_high | u64::from(destination_address));
                    fixture.initial_state.rflags = HexU64(2 | if direction { 1 << 10 } else { 0 });
                    fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                    let mut state = fixture.initial_state.to_cpu().unwrap();
                    let mut memory = MovsbAccessLog {
                        inner: FlatMemory::from_fixture(&fixture).unwrap(),
                        accesses: Vec::new(),
                        mfences: Vec::new(),
                        mfence_error: None,
                    };
                    let before_memory = memory.inner.snapshot();
                    let mut expected = state.clone();
                    let stride = size.bytes() as u32;
                    let next_source = if direction {
                        source_address.wrapping_sub(stride)
                    } else {
                        source_address.wrapping_add(stride)
                    };
                    let next_destination = if direction {
                        destination_address.wrapping_sub(stride)
                    } else {
                        destination_address.wrapping_add(stride)
                    };
                    expected.gpr[Register::Rsi as usize] = u64::from(next_source);
                    expected.gpr[Register::Rdi as usize] = u64::from(next_destination);
                    expected.rflags = 2 | (1 << 6) | (1 << 2) | if direction { 1 << 10 } else { 0 };
                    expected.rip += code.len() as u64;
                    let result = run(
                        &mut state,
                        &mut memory,
                        config(1, fixture.stop_rip.map(HexU64::get)),
                        || false,
                    );
                    assert_eq!(result.exit, RunExit::StopAddress);
                    assert_eq!(result.instructions_executed, 1);
                    assert_eq!(state, expected);
                    assert_eq!(result.rflags_defined, 0x0cd7);
                    assert_eq!(
                        memory.accesses,
                        (0..code.len())
                            .map(|offset| { (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1) })
                            .chain([
                                (AccessKind::Read, u64::from(source_address), size.bytes(),),
                                (
                                    AccessKind::Read,
                                    u64::from(destination_address),
                                    size.bytes(),
                                ),
                            ])
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(memory.inner.snapshot(), before_memory);
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 192);
}

#[test]
fn cmps_read_failures_preserve_state_and_stop_at_the_failing_operand() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms = [
        (OperandSize::Byte, vec![0xa6]),
        (OperandSize::Word, vec![0x66, 0xa7]),
        (OperandSize::Dword, vec![0xa7]),
        (OperandSize::Qword, vec![0x48, 0xa7]),
    ];
    let errors = [
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(4),
            linear_address: Some(0x0020_0004),
        }),
        MemoryError::Unmapped {
            access: AccessKind::Read,
            address: 0x0020_0004,
        },
        MemoryError::PermissionDenied {
            access: AccessKind::Read,
            address: 0x0020_0004,
        },
        MemoryError::Backend("injected CMPS backend error".into()),
    ];
    for (size, code) in forms {
        for direction in [false, true] {
            for site in [0usize, 1] {
                for error in &errors {
                    let mut fixture = template.clone();
                    fixture.memory[0].length = code.len() as u32;
                    fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                    fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                    fixture.memory[1].initial_bytes_hex = "22".repeat(16);
                    fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
                    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0008);
                    fixture.initial_state.rflags =
                        HexU64(2 | if direction { 1 << 10 } else { 0 } | 0x8d5);
                    fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                    let mut state = fixture.initial_state.to_cpu().unwrap();
                    let before = state.clone();
                    let mut memory = AccessTraceMemory {
                        inner: FlatMemory::from_fixture(&fixture).unwrap(),
                        accesses: Vec::new(),
                        fetch_limit: None,
                    };
                    let failed_address = if site == 0 { 0x0020_0000 } else { 0x0020_0008 };
                    let injected_error = match error {
                        MemoryError::Unmapped { .. } => MemoryError::Unmapped {
                            access: AccessKind::Read,
                            address: failed_address,
                        },
                        MemoryError::PermissionDenied { .. } => MemoryError::PermissionDenied {
                            access: AccessKind::Read,
                            address: failed_address,
                        },
                        MemoryError::Architectural(fault) => {
                            MemoryError::Architectural(ArchitecturalFault {
                                linear_address: Some(failed_address),
                                ..fault.clone()
                            })
                        }
                        other => other.clone(),
                    };
                    memory.inner.inject_once(
                        failed_address,
                        AccessKind::Read,
                        injected_error.clone(),
                    );
                    let ram_before = memory.inner.snapshot();
                    let result = run(&mut state, &mut memory, config(1, None), || false);
                    assert_eq!(result.instructions_executed, 0);
                    assert_eq!(result.rflags_defined, 0x0cd7);
                    assert_eq!(state, before);
                    assert_eq!(memory.inner.snapshot(), ram_before);
                    let data_reads = memory
                        .accesses
                        .iter()
                        .filter(|(kind, _, _, _)| *kind == AccessKind::Read)
                        .collect::<Vec<_>>();
                    assert_eq!(
                        data_reads.len(),
                        site + 1,
                        "{size:?} {direction} {site} {error:?}"
                    );
                    assert_eq!(data_reads[0].1, 0x0020_0000);
                    if site == 1 {
                        assert_eq!(data_reads[1].1, 0x0020_0008);
                    }
                    if let MemoryError::Architectural(fault) = injected_error {
                        assert_eq!(result.exit, RunExit::Fault(fault));
                    } else {
                        assert_eq!(result.exit, RunExit::MemoryError(injected_error));
                    }
                }
            }
        }
    }

    let mut dual_missing = template;
    dual_missing.memory[0].length = 1;
    dual_missing.memory[0].initial_bytes_hex = "a6".into();
    dual_missing.memory[0].comparison_mask_hex = "ff".into();
    dual_missing.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_1000);
    dual_missing.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_2000);
    let mut state = dual_missing.initial_state.to_cpu().unwrap();
    let before = state.clone();
    let mut memory = AccessTraceMemory {
        inner: FlatMemory::from_fixture(&dual_missing).unwrap(),
        accesses: Vec::new(),
        fetch_limit: None,
    };
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(result.rflags_defined, 0x0cd7);
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Read,
            address: 0x0020_1000
        })
    );
    assert_eq!(state, before);
    let reads = memory
        .accesses
        .iter()
        .filter(|(kind, _, _, _)| *kind == AccessKind::Read)
        .collect::<Vec<_>>();
    assert_eq!(
        reads.len(),
        1,
        "local RSI-first policy attempts no RDI read after the first fault"
    );
    assert_eq!(reads[0].1, 0x0020_1000);
}

#[test]
fn cmps_prefix_profile_aliasing_and_canonical_fault_boundaries_are_exact() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms: [&[u8]; 4] = [&[0xa6], &[0x66, 0xa7], &[0xa7], &[0x48, 0xa7]];
    let prefixes: [&[u8]; 9] = [
        &[0xf0],
        &[0xf2],
        &[0xf3],
        &[0x26],
        &[0x2e],
        &[0x36],
        &[0x3e],
        &[0x64],
        &[0x65],
    ];
    let mut rejects = Vec::new();
    for form in forms {
        for prefix in prefixes {
            rejects.push([prefix, form].concat());
        }
    }
    rejects.extend([
        vec![0x40, 0xa7],
        vec![0x47, 0xa7],
        vec![0x66, 0x66, 0xa7],
        vec![0xf2, 0xf3, 0xa7],
        vec![0x64, 0x65, 0xa7],
        vec![0x40, 0xa6],
        vec![0x48, 0xa6],
        vec![0x66, 0xa6],
    ]);
    assert_eq!(rejects.len(), 44);
    for code in rejects {
        let mut fixture = template.clone();
        fixture.memory[0].length = code.len() as u32;
        fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
        fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        let before = fixture.initial_state.to_cpu().unwrap();
        let mut state = before.clone();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let ram_before = memory.inner.snapshot();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Unsupported {
                opcode: *code.last().unwrap()
            },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.inner.snapshot(), ram_before);
        assert!(
            memory
                .accesses
                .iter()
                .all(|(kind, _, _)| *kind == AccessKind::Fetch)
        );
    }
    for rex in 0x48..=0x4f {
        for (code, width) in [
            (vec![rex, 0xa7], OperandSize::Qword),
            (vec![0x66, rex, 0xa7], OperandSize::Qword),
            (vec![rex, 0x66, 0xa7], OperandSize::Word),
        ] {
            let mut fixture = template.clone();
            fixture.memory[0].length = code.len() as u32;
            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
            fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0008);
            fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            memory
                .inner
                .write_integer(0x0020_0000, AccessContext::default(), width, 0)
                .unwrap();
            memory
                .inner
                .write_integer(0x0020_0008, AccessContext::default(), width, 0)
                .unwrap();
            let result = run(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(result.exit, RunExit::StopAddress, "{code:02x?}");
            assert_eq!(state.rip, 0x0010_0000 + code.len() as u64);
            assert_eq!(
                memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind == AccessKind::Read)
                    .count(),
                2
            );
        }
    }

    for (size, code) in [
        (OperandSize::Byte, vec![0xa6]),
        (OperandSize::Word, vec![0x66, 0xa7]),
        (OperandSize::Dword, vec![0xa7]),
        (OperandSize::Qword, vec![0x48, 0xa7]),
    ] {
        for bad_operand in [Register::Rsi, Register::Rdi] {
            let mut fixture = template.clone();
            fixture.memory[0].length = code.len() as u32;
            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            fixture.initial_state.gpr[bad_operand as usize] = HexU64(0x0000_8000_0000_0000);
            fixture.initial_state.gpr[if bad_operand == Register::Rsi {
                Register::Rdi as usize
            } else {
                Register::Rsi as usize
            }] = HexU64(0x0020_0008);
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let before = state.clone();
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            let result = run(&mut state, &mut memory, config(1, None), || false);
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 13,
                    error_code: Some(0),
                    linear_address: None
                }),
                "{size:?} {bad_operand:?}"
            );
            assert_eq!(state, before);
            let reads = memory
                .accesses
                .iter()
                .filter(|(kind, _, _)| *kind == AccessKind::Read)
                .count();
            assert_eq!(
                reads,
                usize::from(bad_operand == Register::Rdi),
                "{size:?} {bad_operand:?}"
            );
        }
    }

    for pointer in [Register::Rsi, Register::Rdi] {
        let mut post = template.clone();
        post.memory[0].length = 1;
        post.memory[0].initial_bytes_hex = "a6".into();
        post.memory[0].comparison_mask_hex = "ff".into();
        post.memory[1].guest_address = HexU64(0x0000_7fff_ffff_ffff);
        post.memory[1].length = 1;
        post.memory[1].initial_bytes_hex = "00".into();
        post.memory[1].comparison_mask_hex = "ff".into();
        let mut low = post.memory[1].clone();
        low.region_id = "low".into();
        low.guest_address = HexU64(0x0020_0000);
        low.length = 16;
        low.initial_bytes_hex = "00".repeat(16);
        low.comparison_mask_hex = "ff".repeat(16);
        post.memory.push(low);
        post.initial_state.gpr[Register::Rsi as usize] = HexU64(if pointer == Register::Rsi {
            0x0000_7fff_ffff_ffff
        } else {
            0x0020_0000
        });
        post.initial_state.gpr[Register::Rdi as usize] = HexU64(if pointer == Register::Rdi {
            0x0000_7fff_ffff_ffff
        } else {
            0x0020_0008
        });
        let mut state = post.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut FlatMemory::from_fixture(&post).unwrap(),
            config(1, None),
            || false,
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.gpr[pointer as usize], 0x0000_8000_0000_0000);
    }

    let mut truncated = template;
    truncated.memory[0].length = 1;
    truncated.memory[0].initial_bytes_hex = "66".into();
    truncated.memory[0].comparison_mask_hex = "ff".into();
    let mut state = truncated.initial_state.to_cpu().unwrap();
    let before = state.clone();
    let result = run(
        &mut state,
        &mut FlatMemory::from_fixture(&truncated).unwrap(),
        config(1, None),
        || false,
    );
    assert_eq!(result.instructions_executed, 0);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(state, before);
}

#[test]
fn cmps_overlap_alignment_page_edges_and_wrapping_are_two_prestate_reads() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    for (size, code) in [
        (OperandSize::Byte, vec![0xa6]),
        (OperandSize::Word, vec![0x66, 0xa7]),
        (OperandSize::Dword, vec![0xa7]),
        (OperandSize::Qword, vec![0x48, 0xa7]),
    ] {
        for offset in [
            0u64,
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            0x0fff,
            (0x2000 - size.bytes()) as u64,
        ] {
            let mut fixture = template.clone();
            fixture.memory[0].length = code.len() as u32;
            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            fixture.memory[1].guest_address = HexU64(0x0040_0000);
            fixture.memory[1].length = 0x2000;
            fixture.memory[1].initial_bytes_hex = "44".repeat(0x2000);
            fixture.memory[1].comparison_mask_hex = "ff".repeat(0x2000);
            let source = 0x0040_0000 + offset;
            let destination = 0x0040_1000;
            fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(source);
            fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(destination);
            fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            let result = run(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(
                result.exit,
                RunExit::StopAddress,
                "{size:?} offset={offset:#x}"
            );
            assert_eq!(
                memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind == AccessKind::Read)
                    .count(),
                2
            );
            assert_eq!(
                memory.accesses[memory.accesses.len() - 2],
                (AccessKind::Read, source, size.bytes())
            );
            assert_eq!(
                memory.accesses[memory.accesses.len() - 1],
                (AccessKind::Read, destination, size.bytes())
            );
        }

        let mut alias = template.clone();
        alias.memory[0].length = code.len() as u32;
        alias.memory[0].initial_bytes_hex = encode_bytes(&code);
        alias.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        alias.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0004);
        alias.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0004);
        let mut state = alias.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&alias).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(result.instructions_executed, 1);
        let reads = memory
            .accesses
            .iter()
            .filter(|(kind, _, _)| *kind == AccessKind::Read)
            .collect::<Vec<_>>();
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].1, 0x0020_0004);
        assert_eq!(reads[1].1, 0x0020_0004);

        if size == OperandSize::Qword {
            let mut overlap = alias.clone();
            overlap.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0004);
            overlap.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0005);
            let mut state = overlap.initial_state.to_cpu().unwrap();
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&overlap).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            let result = run(&mut state, &mut memory, config(1, None), || false);
            assert_eq!(result.instructions_executed, 1);
            let reads = memory
                .accesses
                .iter()
                .filter(|(kind, _, _)| *kind == AccessKind::Read)
                .collect::<Vec<_>>();
            assert_eq!(reads.len(), 2);
            assert_eq!(reads[0].1, 0x0020_0004);
            assert_eq!(reads[1].1, 0x0020_0005);
        }

        let mut wrapping = template.clone();
        wrapping.memory[0].length = code.len() as u32;
        wrapping.memory[0].initial_bytes_hex = encode_bytes(&code);
        wrapping.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        wrapping.memory[1].guest_address = HexU64(0);
        wrapping.memory[1].length = 16;
        wrapping.memory[1].initial_bytes_hex = "44".repeat(16);
        wrapping.memory[1].comparison_mask_hex = "ff".repeat(16);
        wrapping.initial_state.gpr[Register::Rsi as usize] = HexU64(0);
        wrapping.initial_state.gpr[Register::Rdi as usize] = HexU64(0);
        wrapping.initial_state.rflags = HexU64(0x4d7);
        let mut state = wrapping.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&wrapping).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(
            state.gpr[Register::Rsi as usize],
            0u64.wrapping_sub(size.bytes() as u64)
        );
        assert_eq!(
            state.gpr[Register::Rdi as usize],
            0u64.wrapping_sub(size.bytes() as u64)
        );

        if size != OperandSize::Byte {
            let crossing_address = 0x0000_7fff_ffff_ffffu64 - size.bytes() as u64 + 2;
            for bad_operand in [Register::Rsi, Register::Rdi] {
                let mut crossing = template.clone();
                crossing.memory[0].length = code.len() as u32;
                crossing.memory[0].initial_bytes_hex = encode_bytes(&code);
                crossing.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                crossing.initial_state.gpr[Register::Rsi as usize] =
                    HexU64(if bad_operand == Register::Rsi {
                        crossing_address
                    } else {
                        0x0020_0004
                    });
                crossing.initial_state.gpr[Register::Rdi as usize] =
                    HexU64(if bad_operand == Register::Rdi {
                        crossing_address
                    } else {
                        0x0020_0004
                    });
                let mut state = crossing.initial_state.to_cpu().unwrap();
                let before = state.clone();
                let mut memory = MovsbAccessLog {
                    inner: FlatMemory::from_fixture(&crossing).unwrap(),
                    accesses: Vec::new(),
                    mfences: Vec::new(),
                    mfence_error: None,
                };
                let result = run(&mut state, &mut memory, config(1, None), || false);
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None
                    })
                );
                assert_eq!(state, before);
                assert_eq!(
                    memory
                        .accesses
                        .iter()
                        .filter(|(kind, _, _)| *kind == AccessKind::Read)
                        .count(),
                    usize::from(bad_operand == Register::Rdi)
                );
            }
        }
    }
}

#[test]
fn scas_prefix_boundaries_and_rex_normalization_are_exact() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    let forms: [&[u8]; 4] = [&[0xae], &[0x66, 0xaf], &[0xaf], &[0x48, 0xaf]];
    let prefixes: [&[u8]; 9] = [
        &[0xf0],
        &[0xf2],
        &[0xf3],
        &[0x26],
        &[0x2e],
        &[0x36],
        &[0x3e],
        &[0x64],
        &[0x65],
    ];
    let mut rejects = Vec::new();
    for form in forms {
        for prefix in prefixes {
            rejects.push([prefix, form].concat());
        }
    }
    rejects.extend([
        vec![0x40, 0xaf],
        vec![0x47, 0xaf],
        vec![0x66, 0x66, 0xaf],
        vec![0xf2, 0xf3, 0xaf],
        vec![0x64, 0x65, 0xaf],
        vec![0x40, 0xae],
        vec![0x48, 0xae],
        vec![0x66, 0xae],
    ]);
    assert_eq!(rejects.len(), 44);
    for code in rejects {
        let mut fixture = template.clone();
        fixture.memory[0].length = code.len() as u32;
        fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
        fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
        let before = fixture.initial_state.to_cpu().unwrap();
        let mut state = before.clone();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let before_ram = memory.inner.snapshot();
        let outcome = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            outcome.exit,
            RunExit::Unsupported {
                opcode: *code.last().unwrap()
            },
            "{code:02x?}"
        );
        assert_eq!(outcome.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(memory.accesses.len(), code.len());
        assert!(
            memory
                .accesses
                .iter()
                .all(|(kind, _, _)| *kind == AccessKind::Fetch)
        );
        assert_eq!(memory.inner.snapshot(), before_ram);
    }

    for rex in 0x48..=0x4f {
        for (code, width) in [
            (vec![rex, 0xaf], OperandSize::Qword),
            (vec![0x66, rex, 0xaf], OperandSize::Qword),
            (vec![rex, 0x66, 0xaf], OperandSize::Word),
        ] {
            let mut fixture = template.clone();
            fixture.memory[0].length = code.len() as u32;
            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            fixture.memory[1].initial_bytes_hex = "00".repeat(16);
            fixture.initial_state.gpr[Register::Rax as usize] = HexU64(0x8877_6655_4433_2211);
            fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0004);
            fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            memory
                .inner
                .write_integer(0x0020_0004, AccessContext::default(), width, 0)
                .unwrap();
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let original_rax = state.gpr[Register::Rax as usize];
            let result = run(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(result.exit, RunExit::StopAddress, "{code:02x?}");
            assert_eq!(
                state.gpr[Register::Rax as usize],
                original_rax,
                "{code:02x?}"
            );
            assert_eq!(state.rip, 0x0010_0000 + code.len() as u64);
            assert_eq!(
                memory.accesses.last(),
                Some(&(AccessKind::Read, 0x0020_0004, width.bytes()))
            );
        }
    }
}

#[test]
fn scas_reads_each_alignment_page_crossing_and_exact_region_end() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    for (size, code) in [
        (OperandSize::Byte, vec![0xae]),
        (OperandSize::Word, vec![0x66, 0xaf]),
        (OperandSize::Dword, vec![0xaf]),
        (OperandSize::Qword, vec![0x48, 0xaf]),
    ] {
        let base = 0x0020_0000u64;
        let region_len = 0x2000usize;
        let mut offsets = (0..8u64).collect::<Vec<_>>();
        offsets.extend([0x0fff, (region_len - size.bytes()) as u64]);
        for offset in offsets {
            let mut fixture = template.clone();
            fixture.memory[0].length = code.len() as u32;
            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            fixture.memory[1].guest_address = HexU64(base);
            fixture.memory[1].length = region_len as u32;
            fixture.memory[1].initial_bytes_hex = "00".repeat(region_len);
            fixture.memory[1].comparison_mask_hex = "ff".repeat(region_len);
            let address = base + offset;
            fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(address);
            fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            let result = run(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(
                result.exit,
                RunExit::StopAddress,
                "{size:?} offset={offset:#x}"
            );
            let expected_read = (AccessKind::Read, address, size.bytes());
            assert_eq!(
                memory.accesses.last(),
                Some(&expected_read),
                "{size:?} offset={offset:#x}"
            );
            assert_eq!(
                memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind == AccessKind::Read)
                    .count(),
                1
            );
            assert_eq!(
                state.gpr[Register::Rdi as usize],
                address.wrapping_add(size.bytes() as u64)
            );
        }
    }
}

#[test]
fn scas_wraps_rdi_at_zero_and_does_not_touch_any_rsi_sentinel() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    for (size, code) in [
        (OperandSize::Byte, vec![0xae]),
        (OperandSize::Word, vec![0x66, 0xaf]),
        (OperandSize::Dword, vec![0xaf]),
        (OperandSize::Qword, vec![0x48, 0xaf]),
    ] {
        for rsi in [0, 0x0020_0000, 0x0020_1000, 0x0000_8000_0000_0000] {
            let mut fixture = template.clone();
            fixture.memory[0].length = code.len() as u32;
            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            fixture.memory[1].guest_address = HexU64(0);
            fixture.memory[1].initial_bytes_hex = "00".repeat(16);
            fixture.memory[1].length = 16;
            fixture.memory[1].comparison_mask_hex = "ff".repeat(16);
            fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0);
            fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(rsi);
            fixture.initial_state.rflags = HexU64(0x0cd7);
            fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            let result = run(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(result.exit, RunExit::StopAddress, "{size:?} RSI={rsi:#x}");
            assert_eq!(
                state.gpr[Register::Rdi as usize],
                0u64.wrapping_sub(size.bytes() as u64)
            );
            assert_eq!(state.gpr[Register::Rsi as usize], rsi);
            assert_eq!(
                memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind != AccessKind::Fetch)
                    .collect::<Vec<_>>(),
                vec![&(AccessKind::Read, 0, size.bytes())]
            );
        }
    }

    let mut fixture = template;
    fixture.memory[0].length = 1;
    fixture.memory[0].initial_bytes_hex = "ae".into();
    fixture.memory[0].comparison_mask_hex = "ff".into();
    fixture.memory[1].guest_address = HexU64(0x0000_7fff_ffff_ffff);
    fixture.memory[1].length = 1;
    fixture.memory[1].initial_bytes_hex = "00".into();
    fixture.memory[1].comparison_mask_hex = "ff".into();
    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0000_7fff_ffff_ffff);
    fixture.stop_rip = Some(HexU64(0x0010_0001));
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x0000_8000_0000_0000);
    assert_eq!(
        memory.accesses,
        vec![
            (AccessKind::Fetch, 0x0010_0000, 1),
            (AccessKind::Read, 0x0000_7fff_ffff_ffff, 1),
        ]
    );
}

#[test]
fn scas_faults_are_atomic_and_test_composition_tracks_definedness() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    fixture.memory[0].length = 4;
    fixture.memory[0].initial_bytes_hex = "4885c0ae".into();
    fixture.memory[0].comparison_mask_hex = "ffffffff".into();
    fixture.stop_rip = Some(HexU64(0x0010_0004));
    fixture.instruction_budget = 2;
    fixture.initial_state.gpr[Register::Rax as usize] = HexU64(1);
    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0004);
    fixture.memory[1].initial_bytes_hex = "00000000010000000000000000000000".into();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(2, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[Register::Rax as usize], 1);
    assert_eq!(
        state.rflags & 0x8d5,
        0x44,
        "SCASB sets only PF and ZF for 1 == 1"
    );
    assert_eq!(
        result.rflags_defined & 0x8d5,
        0x8d5,
        "SCAS defines AF left undefined by TEST"
    );

    let mut fault_fixture = fixture.clone();
    fault_fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0000_8000_0000_0000);
    let mut fault_state = fault_fixture.initial_state.to_cpu().unwrap();
    let mut fault_memory = FlatMemory::from_fixture(&fault_fixture).unwrap();
    let fault_result = run(&mut fault_state, &mut fault_memory, config(2, None), || {
        false
    });
    assert_eq!(
        fault_result.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None
        })
    );
    assert_eq!(
        fault_result.instructions_executed, 1,
        "the preceding TEST remains retired"
    );
    assert_eq!(fault_state.rip, 0x0010_0003, "RIP remains at SCAS");
    assert_eq!(fault_state.gpr[Register::Rax as usize], 1);
    assert_eq!(
        fault_result.rflags_defined & (1 << 4),
        0,
        "faulting SCAS does not define AF"
    );

    for error in [
        MemoryError::Backend("injected SCAS read backend error".into()),
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(4),
            linear_address: Some(0x0020_0004),
        }),
    ] {
        let mut current = fixture.initial_state.to_cpu().unwrap();
        let mut bounded = FlatMemory::from_fixture(&fixture).unwrap();
        bounded.inject_once(0x0020_0004, AccessKind::Read, error.clone());
        let before_ram = bounded.snapshot();
        let current_result = run(&mut current, &mut bounded, config(2, None), || false);
        if let MemoryError::Architectural(fault) = &error {
            assert_eq!(current_result.exit, RunExit::Fault(fault.clone()));
        } else {
            assert_eq!(current_result.exit, RunExit::MemoryError(error.clone()));
        }
        assert_eq!(current_result.instructions_executed, 1);
        assert_eq!(current.rip, 0x0010_0003);
        assert_eq!(current.gpr[Register::Rax as usize], 1);
        assert_eq!(current_result.rflags_defined & (1 << 4), 0);
        assert_eq!(bounded.snapshot(), before_ram);
    }

    let mut denied = fixture.clone();
    denied.memory[1].permissions.read = false;
    let mut denied_state = denied.initial_state.to_cpu().unwrap();
    let mut denied_memory = FlatMemory::from_fixture(&denied).unwrap();
    let denied_ram = denied_memory.snapshot();
    let denied_result = run(
        &mut denied_state,
        &mut denied_memory,
        config(2, None),
        || false,
    );
    assert!(matches!(
        denied_result.exit,
        RunExit::MemoryError(MemoryError::PermissionDenied {
            access: AccessKind::Read,
            ..
        })
    ));
    assert_eq!(denied_result.instructions_executed, 1);
    assert_eq!(denied_state.rip, 0x0010_0003);
    assert_eq!(denied_result.rflags_defined & (1 << 4), 0);
    assert_eq!(denied_memory.snapshot(), denied_ram);

    let mut truncated = fixture;
    truncated.memory[0].length = 1;
    truncated.memory[0].initial_bytes_hex = "48".into();
    truncated.memory[0].comparison_mask_hex = "ff".into();
    let mut truncated_state = truncated.initial_state.to_cpu().unwrap();
    let before = truncated_state.clone();
    let mut truncated_memory = FlatMemory::from_fixture(&truncated).unwrap();
    let truncated_ram = truncated_memory.snapshot();
    let truncated_result = run(
        &mut truncated_state,
        &mut truncated_memory,
        config(2, None),
        || false,
    );
    assert!(matches!(
        truncated_result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(truncated_result.instructions_executed, 0);
    assert_eq!(truncated_state, before);
    assert_eq!(truncated_memory.snapshot(), truncated_ram);
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScasExpected {
    gpr: [HexU64; 16],
    rflags: HexU64,
    rflags_defined: HexU64,
    rip: HexU64,
    memory: std::collections::BTreeMap<String, String>,
    fault: Option<ScasFault>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ScasFault {
    vector: u8,
    error_code: HexU64,
}

#[test]
fn scas_literal_fixtures_match_full_state_and_access_contract() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, ScasExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18h_scas_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 24);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let initial_memory = memory.inner.snapshot();
        let mut state = original.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let code = decode_bytes(&input.memory[0].initial_bytes_hex).unwrap();
        let has_test = code.starts_with(&[0x48, 0x85, 0xc0]);
        let success = input.expected_exit == ExpectedExit::StopAddress;
        let budget = input.instruction_budget == 0;
        let unmapped = input.expected_exit == ExpectedExit::HarnessError;
        let exception = input.expected_exit == ExpectedExit::Exception;
        let expected_retired = if success {
            if has_test { 2 } else { 1 }
        } else if has_test {
            1
        } else {
            0
        };
        assert_eq!(result.instructions_executed, expected_retired, "{case_id}");
        if success {
            assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        } else if budget {
            assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            );
        } else if unmapped {
            assert!(
                matches!(
                    result.exit,
                    RunExit::MemoryError(MemoryError::Unmapped {
                        access: AccessKind::Read,
                        ..
                    })
                ),
                "{case_id}: {:?}",
                result.exit
            );
        } else if exception {
            let fault = answer.fault.as_ref().expect("literal architectural fault");
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: fault.vector,
                    error_code: Some(fault.error_code.get() as u32),
                    linear_address: None
                }),
                "{case_id}"
            );
        }
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        for region in memory.inner.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
        let expected_accesses = if budget {
            Vec::new()
        } else {
            (0..code.len())
                .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1))
                .chain((success || unmapped).then_some((
                    AccessKind::Read,
                    original.gpr[Register::Rdi as usize],
                    match code[code.len() - 1] {
                        0xae => 1,
                        _ if code.starts_with(&[0x66, 0x48]) => 8,
                        _ if code.starts_with(&[0x66]) || code.starts_with(&[0x48, 0x66]) => 2,
                        _ if code.starts_with(&[0x48]) => 8,
                        _ => 4,
                    },
                )))
                .collect()
        };
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");
        if !success {
            assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
        }
    }
}

#[test]
fn cmps_literal_fixtures_match_full_state_and_ordered_access_contract() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, ScasExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18i_cmps_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 33);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original = input.initial_state.to_cpu().unwrap();
        let mut state = original.clone();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let ram_before = memory.inner.snapshot();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let code = decode_bytes(&input.memory[0].initial_bytes_hex).unwrap();
        let success = input.expected_exit == ExpectedExit::StopAddress;
        let budget = input.expected_exit == ExpectedExit::InstructionBudgetExhausted;
        let backend = input.expected_exit == ExpectedExit::HarnessError;
        let fault = input.expected_exit == ExpectedExit::Exception;
        let prior_test = code.starts_with(&[0x48, 0x85, 0xc0]);
        let expected_retired = if success {
            if prior_test { 2 } else { 1 }
        } else if prior_test {
            1
        } else {
            0
        };
        assert_eq!(result.instructions_executed, expected_retired, "{case_id}");
        if success {
            assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        } else if budget {
            assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            );
        } else if backend {
            assert!(
                matches!(
                    result.exit,
                    RunExit::MemoryError(MemoryError::Unmapped {
                        access: AccessKind::Read,
                        ..
                    })
                ),
                "{case_id}: {:?}",
                result.exit
            );
        } else if fault {
            let fault = answer.fault.as_ref().unwrap();
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: fault.vector,
                    error_code: Some(fault.error_code.get() as u32),
                    linear_address: None
                }),
                "{case_id}"
            );
        }
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        for region in memory.inner.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}: {}",
                region.region_id
            );
        }
        let data_accesses = if budget {
            0
        } else if prior_test {
            if success {
                2
            } else if case_id.ends_with("first_fault") {
                0
            } else {
                1
            }
        } else if case_id.contains("noncanonical") {
            if case_id.ends_with("_first") { 0 } else { 1 }
        } else if case_id.contains("unmapped") {
            if case_id.ends_with("_first") { 1 } else { 2 }
        } else if success {
            2
        } else {
            0
        };
        let expected_accesses = if budget {
            Vec::new()
        } else {
            (0..code.len())
                .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1))
                .chain((0..data_accesses).map(|site| {
                    (
                        AccessKind::Read,
                        if site == 0 {
                            original.gpr[Register::Rsi as usize]
                        } else {
                            original.gpr[Register::Rdi as usize]
                        },
                        match code.last().copied().unwrap() {
                            0xa6 => 1,
                            _ if code.starts_with(&[0x48, 0x66]) => 2,
                            _ if code.starts_with(&[0x48]) || code.starts_with(&[0x66, 0x48]) => 8,
                            _ if code.starts_with(&[0x66]) => 2,
                            _ => 4,
                        },
                    )
                }))
                .collect::<Vec<_>>()
        };
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");
        if !success {
            assert_eq!(memory.inner.snapshot(), ram_before, "{case_id}");
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StosWidthsExpected {
    gpr: [HexU64; 16],
    rflags: HexU64,
    rflags_defined: HexU64,
    rip: HexU64,
    memory: std::collections::BTreeMap<String, String>,
    fault: Option<StosWidthsFault>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StosWidthsFault {
    vector: u8,
    error_code: HexU64,
}

#[test]
fn stos_width_literal_fixtures_match_full_state_memory_and_write_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, StosWidthsExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18g_stos_widths_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 20);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original = input.initial_state.to_cpu().unwrap();
        let start = original.gpr[Register::Rdi as usize];
        let width = match input.memory[0].initial_bytes_hex.as_str() {
            "aa" => 1,
            "66ab" | "4866ab" => 2,
            "ab" => 4,
            "48ab" | "6648ab" => 8,
            other => panic!("unexpected STOS fixture encoding {other}"),
        };
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let initial_memory = memory.inner.snapshot();
        let mut state = original.clone();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let expected_accesses = if input.instruction_budget == 0 {
            Vec::new()
        } else {
            (0..input.memory[0].length as usize)
                .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1))
                .chain((start != 0x0000_8000_0000_0000).then_some((
                    AccessKind::Write,
                    start,
                    width,
                )))
                .collect()
        };
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");
        match input.expected_exit {
            crate::v2::ExpectedExit::StopAddress => {
                assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
                assert_eq!(actual.instructions_executed, 1, "{case_id}");
            }
            crate::v2::ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(
                    actual.exit,
                    RunExit::InstructionBudgetExhausted,
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
            }
            crate::v2::ExpectedExit::HarnessError if start >= 0x0020_1000 => {
                assert_eq!(
                    actual.exit,
                    RunExit::MemoryError(MemoryError::Unmapped {
                        address: start,
                        access: AccessKind::Write,
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
            }
            crate::v2::ExpectedExit::HarnessError => {
                assert_eq!(
                    actual.exit,
                    RunExit::MemoryError(MemoryError::PermissionDenied {
                        address: start,
                        access: AccessKind::Write,
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
            }
            crate::v2::ExpectedExit::Exception => {
                let fault = answer.fault.expect("architectural fault fixture");
                assert_eq!(
                    actual.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: fault.vector,
                        error_code: Some(fault.error_code.get() as u32),
                        linear_address: None,
                    }),
                    "{case_id}"
                );
                assert_eq!(actual.instructions_executed, 0, "{case_id}");
            }
            other => panic!("unexpected STOS fixture outcome {other:?}: {case_id}"),
        }
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            actual.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        for region in memory.inner.snapshot() {
            assert_eq!(
                region.bytes_hex,
                answer.memory.get(&region.region_id).unwrap().as_str(),
                "{case_id}: {}",
                region.region_id
            );
        }
        if !matches!(input.expected_exit, crate::v2::ExpectedExit::StopAddress) {
            assert_eq!(memory.inner.snapshot(), initial_memory, "{case_id}");
            assert_eq!(state, original, "{case_id}");
        }
    }
}

#[test]
fn stos_rex_order_all_encodings_and_ignored_rsi_values_execute_without_reads() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    for rex in 0x48..=0x4f {
        for (code, size) in [
            (vec![rex, 0xab], OperandSize::Qword),
            (vec![0x66, rex, 0xab], OperandSize::Qword),
            (vec![rex, 0x66, 0xab], OperandSize::Word),
        ] {
            for rsi in [0x0030_0080, 0x0020_0008, 0x0020_1000, 0x0000_8000_0000_0000] {
                let mut fixture = template.clone();
                fixture.memory[0].length = code.len() as u32;
                fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
                fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                fixture.memory[1].length = 32;
                fixture.memory[1].initial_bytes_hex = "5a".repeat(32);
                fixture.memory[1].comparison_mask_hex = "ff".repeat(32);
                fixture.initial_state.rip = HexU64(0x0010_0000);
                fixture.initial_state.gpr[Register::Rax as usize] = HexU64(0x1122_3344_5566_7788);
                fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0008);
                fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(rsi);
                fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(0x8877_6655_4433_2211);
                fixture.initial_state.rflags = HexU64(0x0cd7);
                fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                let before = fixture.initial_state.to_cpu().unwrap();
                let mut expected = before.clone();
                expected.rip += code.len() as u64;
                let start = before.gpr[Register::Rdi as usize];
                expected.gpr[Register::Rdi as usize] = if before.rflags & (1 << 10) == 0 {
                    start.wrapping_add(size.bytes() as u64)
                } else {
                    start.wrapping_sub(size.bytes() as u64)
                };
                let mut memory = MovsbAccessLog {
                    inner: FlatMemory::from_fixture(&fixture).unwrap(),
                    accesses: Vec::new(),
                    mfences: Vec::new(),
                    mfence_error: None,
                };
                let mut state = before;
                let result = run(
                    &mut state,
                    &mut memory,
                    config(1, fixture.stop_rip.map(HexU64::get)),
                    || false,
                );
                assert_eq!(
                    result.exit,
                    RunExit::StopAddress,
                    "{code:02x?} RSI={rsi:#x}"
                );
                assert_eq!(state, expected, "{code:02x?} RSI={rsi:#x}");
                let accesses = (0..code.len())
                    .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset as u64, 1))
                    .chain(std::iter::once((AccessKind::Write, start, size.bytes())))
                    .collect::<Vec<_>>();
                assert_eq!(memory.accesses, accesses, "{code:02x?} RSI={rsi:#x}");
            }
        }
    }
}

#[test]
fn stos_prefix_rejects_are_unsupported_before_any_data_access() {
    let forms: [&[u8]; 3] = [&[0x66, 0xab], &[0xab], &[0x48, 0xab]];
    let prefixes: [&[u8]; 9] = [
        &[0xf0],
        &[0xf2],
        &[0xf3],
        &[0x26],
        &[0x2e],
        &[0x36],
        &[0x3e],
        &[0x64],
        &[0x65],
    ];
    let mut cases = Vec::new();
    for form in forms {
        for prefix in prefixes {
            if *prefix == [0xf3] {
                continue;
            }
            cases.push([prefix, form].concat());
        }
    }
    cases.extend([
        vec![0x40, 0xab],
        vec![0x47, 0xab],
        vec![0x66, 0x66, 0xab],
        vec![0xf2, 0xf3, 0xab],
        vec![0x64, 0x65, 0xab],
    ]);
    assert_eq!(cases.len(), 29);
    for code in cases {
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        state.gpr[Register::Rdi as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = memory(&code, 0x1000);
        let before_memory = memory.snapshot();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0xab },
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{code:02x?}");
        assert_eq!(state, before, "{code:02x?}");
        assert_eq!(memory.snapshot(), before_memory, "{code:02x?}");
    }
}

#[test]
fn stos_unaligned_cross_page_write_and_resulting_rdi_boundaries() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    for residue in 0..8u64 {
        let mut fixture = template.clone();
        fixture.memory[0].initial_bytes_hex = "48ab".into();
        fixture.memory[0].length = 2;
        fixture.memory[0].comparison_mask_hex = "ffff".into();
        fixture.memory[1].guest_address = HexU64(0x0030_0000);
        fixture.memory[1].length = 0x2000;
        fixture.memory[1].initial_bytes_hex = "00".repeat(0x2000);
        fixture.memory[1].comparison_mask_hex = "ff".repeat(0x2000);
        fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0030_0100 + residue);
        fixture.initial_state.gpr[Register::Rax as usize] = HexU64(0x0123_4567_89ab_cdef);
        fixture.initial_state.rip = HexU64(0x0010_0000);
        fixture.stop_rip = Some(HexU64(0x0010_0002));
        let start = fixture.initial_state.gpr[Register::Rdi as usize].get();
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(1, Some(0x0010_0002)),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress, "residue={residue}");
        assert_eq!(state.gpr[Register::Rdi as usize], start + 8);
        let image = memory.snapshot()[1].bytes_hex.clone();
        assert_eq!(
            &image[start as usize % 0x2000 * 2..][..16],
            "efcdab8967452301",
            "residue={residue}"
        );
    }

    // A qword write crossing the 4 KiB boundary is one bounded write.
    let mut fixture = template.clone();
    fixture.memory[0].initial_bytes_hex = "48ab".into();
    fixture.memory[0].length = 2;
    fixture.memory[0].comparison_mask_hex = "ffff".into();
    fixture.memory[1].guest_address = HexU64(0x0030_0000);
    fixture.memory[1].length = 0x2000;
    fixture.memory[1].initial_bytes_hex = "00".repeat(0x2000);
    fixture.memory[1].comparison_mask_hex = "ff".repeat(0x2000);
    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0030_0ffd);
    fixture.initial_state.rip = HexU64(0x0010_0000);
    fixture.stop_rip = Some(HexU64(0x0010_0002));
    let mut logged = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let result = run(
        &mut state,
        &mut logged,
        config(1, Some(0x0010_0002)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(
        logged.accesses,
        [
            (AccessKind::Fetch, 0x0010_0000, 1),
            (AccessKind::Fetch, 0x0010_0001, 1),
            (AccessKind::Write, 0x0030_0ffd, 8),
        ]
    );

    // The byte store at the top canonical address commits even though the
    // post-increment RDI is noncanonical; wrapping at u64 also remains explicit.
    for (destination, next, reverse) in [
        (0x0000_7fff_ffff_ffff, 0x0000_8000_0000_0000, false),
        (0, u64::MAX, true),
    ] {
        let mut fixture = template.clone();
        fixture.memory[0].initial_bytes_hex = "aa".into();
        fixture.memory[0].length = 1;
        fixture.memory[0].comparison_mask_hex = "ff".into();
        fixture.memory[1].guest_address = HexU64(destination);
        fixture.memory[1].length = 1;
        fixture.memory[1].initial_bytes_hex = "00".into();
        fixture.memory[1].comparison_mask_hex = "ff".into();
        fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(destination);
        fixture.initial_state.rflags = HexU64(if reverse { 0x0cd7 } else { 0x08d7 });
        fixture.initial_state.rip = HexU64(0x0010_0000);
        fixture.stop_rip = Some(HexU64(0x0010_0001));
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(1, Some(0x0010_0001)),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(state.gpr[Register::Rdi as usize], next);
        assert_eq!(memory.snapshot()[1].bytes_hex, "88");
    }
}

#[test]
fn stos_end_of_region_fault_injection_and_truncated_fetch_are_atomic() {
    let template = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_18e_stosb_df0.case.json"
    ))
    .unwrap()
    .fixture;
    for (offset, succeeds) in [(8, true), (9, false), (12, false)] {
        let mut fixture = template.clone();
        fixture.memory[0].initial_bytes_hex = "48ab".into();
        fixture.memory[0].length = 2;
        fixture.memory[0].comparison_mask_hex = "ffff".into();
        fixture.memory[1].guest_address = HexU64(0x0030_0000);
        fixture.memory[1].length = 16;
        fixture.memory[1].initial_bytes_hex = "5a".repeat(16);
        fixture.memory[1].comparison_mask_hex = "ff".repeat(16);
        fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0030_0000 + offset);
        fixture.initial_state.rip = HexU64(0x0010_0000);
        fixture.stop_rip = Some(HexU64(0x0010_0002));
        let before = fixture.initial_state.to_cpu().unwrap();
        let before_memory = FlatMemory::from_fixture(&fixture).unwrap().snapshot();
        let mut state = before.clone();
        let mut logged = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let result = run(
            &mut state,
            &mut logged,
            config(1, Some(0x0010_0002)),
            || false,
        );
        assert_eq!(logged.accesses.len(), 3, "offset={offset}");
        assert_eq!(
            logged.accesses[2],
            (AccessKind::Write, 0x0030_0000 + offset, 8)
        );
        if succeeds {
            assert_eq!(result.exit, RunExit::StopAddress);
            assert_eq!(state.rip, 0x0010_0002);
            assert_eq!(
                logged.inner.snapshot()[1].bytes_hex,
                format!("{}8877665544332211", "5a".repeat(8))
            );
        } else {
            assert_eq!(
                result.exit,
                RunExit::MemoryError(MemoryError::Unmapped {
                    address: 0x0030_0010,
                    access: AccessKind::Write,
                })
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
            assert_eq!(logged.inner.snapshot(), before_memory);
        }
    }

    for injected in [
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(6),
            linear_address: Some(0x0030_0004),
        }),
        MemoryError::Backend("injected STOS backend failure".into()),
    ] {
        let mut fixture = template.clone();
        fixture.memory[0].initial_bytes_hex = "48ab".into();
        fixture.memory[0].length = 2;
        fixture.memory[0].comparison_mask_hex = "ffff".into();
        fixture.memory[1].guest_address = HexU64(0x0030_0000);
        fixture.memory[1].length = 16;
        fixture.memory[1].initial_bytes_hex = "5a".repeat(16);
        fixture.memory[1].comparison_mask_hex = "ff".repeat(16);
        fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0030_0004);
        fixture.initial_state.rip = HexU64(0x0010_0000);
        fixture.stop_rip = Some(HexU64(0x0010_0002));
        let before = fixture.initial_state.to_cpu().unwrap();
        let mut state = before.clone();
        let mut logged = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let initial_memory = logged.inner.snapshot();
        logged
            .inner
            .inject_once(0x0030_0004, AccessKind::Write, injected.clone());
        let result = run(
            &mut state,
            &mut logged,
            config(1, Some(0x0010_0002)),
            || false,
        );
        match injected {
            MemoryError::Architectural(fault) => assert_eq!(result.exit, RunExit::Fault(fault)),
            other => assert_eq!(result.exit, RunExit::MemoryError(other)),
        }
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
        assert_eq!(logged.inner.snapshot(), initial_memory);
        assert_eq!(
            logged.accesses.last(),
            Some(&(AccessKind::Write, 0x0030_0004, 8))
        );
    }

    // The fetched prefix cannot be completed, and budget zero does not fetch.
    let mut fixture = template;
    fixture.memory[0].initial_bytes_hex = "48".into();
    fixture.memory[0].length = 1;
    fixture.memory[0].comparison_mask_hex = "ff".into();
    fixture.initial_state.rip = HexU64(0x0010_0000);
    fixture.stop_rip = Some(HexU64(0x0010_0001));
    let before = fixture.initial_state.to_cpu().unwrap();
    let mut state = before.clone();
    let mut logged = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(
        &mut state,
        &mut logged,
        config(1, Some(0x0010_0001)),
        || false,
    );
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x0010_0001,
            access: AccessKind::Fetch,
        })
    );
    assert_eq!(state, before);
    assert_eq!(
        logged.accesses,
        [
            (AccessKind::Fetch, 0x0010_0000, 1),
            (AccessKind::Fetch, 0x0010_0001, 1),
        ]
    );
}

#[test]
fn rep_stosb_fixtures_match_literal_progress_and_access_order() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_19a_rep_stosb_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 5);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let original = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let expected_gprs = answer.gpr.map(HexU64::get);
        if let Some(fault) = &answer.fault {
            assert_eq!(fault.access, "write", "{case_id}");
            memory.inner.inject_once(
                fault.address.get(),
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(fault.address.get()),
                }),
            );
        }
        let mut state = original.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        let count = original.gpr[Register::Rcx as usize];
        let initial_rdi = original.gpr[Register::Rdi as usize];
        let expected_addresses = if input.initial_state.rflags.get() & (1 << 10) == 0 {
            (0..count).map(|i| initial_rdi + i).collect::<Vec<_>>()
        } else {
            (0..count).map(|i| initial_rdi - i).collect::<Vec<_>>()
        };
        let mut expected_accesses = (0..2)
            .map(|offset| (AccessKind::Fetch, 0x0010_0000 + offset, 1))
            .collect::<Vec<_>>();
        expected_accesses.extend(
            expected_addresses
                .iter()
                .map(|address| (AccessKind::Write, *address, 1)),
        );
        assert_eq!(memory.accesses, expected_accesses, "{case_id}");
        if answer.fault.is_some() {
            assert_eq!(
                result.exit,
                RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: answer.fault.as_ref().map(|fault| fault.address.get()),
                }),
                "{case_id}"
            );
            assert_eq!(result.instructions_executed, 0, "{case_id}");
            assert_eq!(state.gpr, expected_gprs, "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        } else {
            assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
            assert_eq!(result.instructions_executed, 1, "{case_id}");
            assert_eq!(state.gpr, expected_gprs, "{case_id}");
            assert_eq!(state.rip, answer.rip.get(), "{case_id}");
            assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
            for region in memory.inner.snapshot() {
                assert_eq!(
                    region.bytes_hex,
                    answer.memory.get(&region.region_id).unwrap().as_str(),
                    "{case_id}: {}",
                    region.region_id
                );
            }
        }
    }
}

#[test]
fn rep_stosb_stop_and_iteration_work_limit_preserve_restart_state() {
    let directory = arithmetic_fixture_directory();
    let mut fixture = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_df0.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let start_rip = state.rip;
    let start_rdi = state.gpr[Register::Rdi as usize];
    let mut successful_boundaries = 0;
    let stopped = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || {
            successful_boundaries += 1;
            successful_boundaries == 3
        },
    );
    assert_eq!(stopped.exit, RunExit::StopRequested);
    assert_eq!(stopped.instructions_executed, 0);
    assert_eq!(state.rip, start_rip);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rdi as usize], start_rdi + 2);

    let resumed = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(resumed.exit, RunExit::StopAddress);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(state.gpr[Register::Rdi as usize], start_rdi + 3);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());

    fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(REP_ITERATION_WORK_LIMIT + 1);
    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0000);
    fixture.memory[1].length = (REP_ITERATION_WORK_LIMIT + 1) as u32;
    fixture.memory[1].initial_bytes_hex = "00".repeat((REP_ITERATION_WORK_LIMIT + 1) as usize);
    fixture.memory[1].comparison_mask_hex = "ff".repeat((REP_ITERATION_WORK_LIMIT + 1) as usize);
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let start_rip = state.rip;
    let start_rdi = state.gpr[Register::Rdi as usize];
    let limited = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(limited.exit, RunExit::RepIterationBudgetExhausted);
    assert_eq!(limited.instructions_executed, 0);
    assert_eq!(state.rip, start_rip);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(
        state.gpr[Register::Rdi as usize],
        start_rdi + REP_ITERATION_WORK_LIMIT
    );
    assert!(
        memory.snapshot()[1]
            .bytes_hex
            .starts_with(&"88".repeat(REP_ITERATION_WORK_LIMIT as usize))
    );
    assert!(memory.snapshot()[1].bytes_hex.ends_with("00"));
    let resumed = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(resumed.exit, RunExit::StopAddress);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(
        state.gpr[Register::Rdi as usize],
        start_rdi + REP_ITERATION_WORK_LIMIT + 1
    );
}

#[test]
fn rep_stosb_rejects_other_prefix_combinations_without_commit() {
    let directory = arithmetic_fixture_directory();
    let mut fixture = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    for bytes in ["f2aa", "f366aa", "f340aa"] {
        fixture.memory[0].length = (bytes.len() / 2) as u32;
        fixture.memory[0].initial_bytes_hex = bytes.into();
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let original_state = state.clone();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0xaa },
            "{bytes}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes}");
        assert_eq!(state, original_state, "{bytes}");
    }
}

#[test]
fn rep_lods_prefix_rejects_without_data_access_or_mutation() {
    let dir = arithmetic_fixture_directory();
    let template =
        Fixture::from_json(&std::fs::read(dir.join("s2_19a_rep_stosb_one.case.json")).unwrap())
            .unwrap()
            .fixture;
    let forms: [&[u8]; 4] = [
        &[0xf3, 0xac],
        &[0xf3, 0x66, 0xad],
        &[0xf3, 0xad],
        &[0xf3, 0x48, 0xad],
    ];
    let prefixes: [&[u8]; 9] = [
        &[0xf0],
        &[0xf2],
        &[0xf3],
        &[0x26],
        &[0x2e],
        &[0x36],
        &[0x3e],
        &[0x64],
        &[0x65],
    ];
    let mut cases = Vec::new();
    for form in forms {
        for prefix in prefixes {
            cases.push([prefix, form].concat());
        }
    }
    cases.extend([
        vec![0xf3, 0x66, 0xac],
        vec![0xf3, 0x40, 0xac],
        vec![0xf3, 0x48, 0xac],
        vec![0xf3, 0x40, 0xad],
        vec![0xf3, 0x47, 0xad],
        vec![0xf3, 0x66, 0x66, 0xad],
        vec![0xf3, 0x66, 0x40, 0xad],
        vec![0xf3, 0x66, 0x47, 0xad],
    ]);
    assert_eq!(cases.len(), 44);
    for bytes in cases.into_iter().chain([vec![0x48, 0xf3, 0xac]]) {
        let mut input = template.clone();
        input.memory[0].length = bytes.len() as u32;
        input.memory[0].initial_bytes_hex = encode_bytes(&bytes);
        input.memory[0].comparison_mask_hex = "ff".repeat(bytes.len());
        input.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
        input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Unsupported {
                opcode: *bytes.last().unwrap()
            },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before, "{bytes:02x?}");
        assert!(
            !memory
                .accesses
                .iter()
                .any(|(kind, _, _)| matches!(kind, AccessKind::Read | AccessKind::Write)),
            "{bytes:02x?}"
        );
        assert_eq!(
            memory.inner.snapshot()[1].bytes_hex,
            input.memory[1].initial_bytes_hex
        );
    }
}

#[test]
fn rep_lods_wdq_complete_state_matrix_and_prefix_boundary() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xac]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xad]),
        (OperandSize::Dword, vec![0xf3, 0xad]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xad]),
    ];
    let flag_bits = [0, 2, 4, 6, 7, 11];
    let sentinel = 0x89ab_cdef_7654_3210;
    let mut cases = 0;
    for (width, code) in &widths {
        let size = width.bytes() as usize;
        let mask = width.mask();
        let sign = 1u64 << (size * 8 - 1);
        for df in [false, true] {
            for seed in [0, 1, mask, sign, 0x55 & mask, 0xaa & mask] {
                for count in 0..=3u64 {
                    for pattern in 0..64usize {
                        let mut input = template.clone();
                        input.memory[0].length = code.len() as u32;
                        input.memory[0].initial_bytes_hex = encode_bytes(code);
                        input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                        input.memory[1].guest_address = HexU64(0x0020_0000);
                        input.memory[1].length = (size * 4) as u32;
                        let mut bytes = Vec::new();
                        for k in 0..4u64 {
                            bytes.extend_from_slice(
                                &(seed.wrapping_add(k) & mask).to_le_bytes()[..size],
                            );
                        }
                        input.memory[1].initial_bytes_hex = encode_bytes(&bytes);
                        input.memory[1].comparison_mask_hex = "ff".repeat(bytes.len());
                        input.memory[1].permissions = MemoryPermissions {
                            read: true,
                            write: false,
                            execute: false,
                        };
                        input.initial_state.gpr[Register::Rax as usize] = HexU64(sentinel);
                        input.initial_state.gpr[Register::Rcx as usize] = HexU64(count);
                        let start = if df {
                            0x0020_0000 + 2 * size as u64
                        } else {
                            0x0020_0000
                        };
                        input.initial_state.gpr[Register::Rsi as usize] = HexU64(start);
                        input.initial_state.rflags = HexU64(
                            0x2 | if df { 1 << 10 } else { 0 }
                                | flag_bits
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, _)| pattern & (1 << i) != 0)
                                    .fold(0, |v, (_, b)| v | (1 << b)),
                        );
                        input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                        let before = input.initial_state.to_cpu().unwrap();
                        let mut state = before.clone();
                        let mut memory = MovsbAccessLog {
                            inner: FlatMemory::from_fixture(&input).unwrap(),
                            accesses: Vec::new(),
                            mfences: Vec::new(),
                            mfence_error: None,
                        };
                        let result = run(
                            &mut state,
                            &mut memory,
                            config(1, input.stop_rip.map(HexU64::get)),
                            || false,
                        );
                        assert_eq!(
                            result.exit,
                            RunExit::StopAddress,
                            "{width:?} DF={df} seed={seed:#x} count={count} pattern={pattern}"
                        );
                        assert_eq!(result.instructions_executed, 1);
                        let mut expected = before.clone();
                        expected.gpr[Register::Rcx as usize] = 0;
                        expected.gpr[Register::Rsi as usize] = if df {
                            start.wrapping_sub(count * size as u64)
                        } else {
                            start + count * size as u64
                        };
                        if count != 0 {
                            expected.write(
                                RegisterView::low(Register::Rax, *width),
                                (seed.wrapping_add(if df { 3 - count } else { count - 1 })) & mask,
                            );
                        }
                        expected.rip = input.stop_rip.unwrap().get();
                        assert_eq!(
                            state, expected,
                            "{width:?} DF={df} seed={seed:#x} count={count} pattern={pattern}"
                        );
                        let reads: Vec<_> = memory
                            .accesses
                            .iter()
                            .filter(|(kind, _, _)| *kind == AccessKind::Read)
                            .copied()
                            .collect();
                        let expected_reads: Vec<_> = (0..count)
                            .map(|k| {
                                (
                                    AccessKind::Read,
                                    if df {
                                        start.wrapping_sub(k * size as u64)
                                    } else {
                                        start + k * size as u64
                                    },
                                    size,
                                )
                            })
                            .collect();
                        assert_eq!(reads, expected_reads);
                        assert!(
                            !memory
                                .accesses
                                .iter()
                                .any(|(kind, _, _)| *kind == AccessKind::Write)
                        );
                        assert_eq!(memory.inner.snapshot()[1].bytes_hex, encode_bytes(&bytes));
                        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 12_288);
}

#[test]
fn rep_lods_fault_matrix_commits_only_successful_elements() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xac]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xad]),
        (OperandSize::Dword, vec![0xf3, 0xad]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xad]),
    ];
    let failures = [
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(4),
            linear_address: Some(0),
        }),
        MemoryError::Unmapped {
            address: 0,
            access: AccessKind::Read,
        },
        MemoryError::PermissionDenied {
            address: 0,
            access: AccessKind::Read,
        },
        MemoryError::Backend("injected load failure".into()),
    ];
    let mut cases = 0;
    for (width, code) in widths {
        for df in [false, true] {
            for successes in 0..=2u64 {
                for failure in &failures {
                    let size = width.bytes() as usize;
                    let mut input = template.clone();
                    input.memory[0].length = code.len() as u32;
                    input.memory[0].initial_bytes_hex = encode_bytes(&code);
                    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                    input.memory[1].length = 64;
                    input.memory[1].initial_bytes_hex = "5a".repeat(64);
                    input.memory[1].comparison_mask_hex = "ff".repeat(64);
                    input.memory[1].permissions = MemoryPermissions {
                        read: true,
                        write: false,
                        execute: false,
                    };
                    let start = if df { 0x0020_0030 } else { 0x0020_0000 };
                    input.initial_state.gpr[Register::Rcx as usize] = HexU64(successes + 1);
                    input.initial_state.gpr[Register::Rsi as usize] = HexU64(start);
                    input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                    let address = if df {
                        start.wrapping_sub(successes * size as u64)
                    } else {
                        start + successes * size as u64
                    };
                    let injected = match failure {
                        MemoryError::Architectural(f) => {
                            MemoryError::Architectural(ArchitecturalFault {
                                linear_address: Some(address),
                                ..f.clone()
                            })
                        }
                        MemoryError::Unmapped { access, .. } => MemoryError::Unmapped {
                            address,
                            access: *access,
                        },
                        MemoryError::PermissionDenied { access, .. } => {
                            MemoryError::PermissionDenied {
                                address,
                                access: *access,
                            }
                        }
                        other => other.clone(),
                    };
                    let before = input.initial_state.to_cpu().unwrap();
                    let mut state = before.clone();
                    let mut memory = MovsbAccessLog {
                        inner: FlatMemory::from_fixture(&input).unwrap(),
                        accesses: Vec::new(),
                        mfences: Vec::new(),
                        mfence_error: None,
                    };
                    memory
                        .inner
                        .inject_once(address, AccessKind::Read, injected.clone());
                    let result = run(&mut state, &mut memory, config(1, None), || false);
                    match injected {
                        MemoryError::Architectural(f) => assert_eq!(result.exit, RunExit::Fault(f)),
                        other => assert_eq!(result.exit, RunExit::MemoryError(other)),
                    }
                    assert_eq!(result.instructions_executed, 0);
                    let mut expected = before.clone();
                    expected.gpr[Register::Rcx as usize] = 1;
                    expected.gpr[Register::Rsi as usize] = if df {
                        start.wrapping_sub(successes * size as u64)
                    } else {
                        start + successes * size as u64
                    };
                    if successes > 0 {
                        let v = if df { 0x5a } else { 0x5a };
                        expected.write(
                            RegisterView::low(Register::Rax, width),
                            if size == 8 {
                                u64::from_le_bytes([v; 8])
                            } else {
                                u64::from_le_bytes([v; 8])
                            } & width.mask(),
                        );
                    }
                    assert_eq!(state, expected);
                    let reads: Vec<_> = memory
                        .accesses
                        .iter()
                        .filter(|(kind, _, _)| *kind == AccessKind::Read)
                        .map(|x| x.1)
                        .collect();
                    let mut wanted: Vec<_> = (0..successes)
                        .map(|k| {
                            if df {
                                start.wrapping_sub(k * size as u64)
                            } else {
                                start + k * size as u64
                            }
                        })
                        .collect();
                    wanted.push(address);
                    assert_eq!(reads, wanted);
                    assert_eq!(memory.inner.snapshot()[1].bytes_hex, "5a".repeat(64));
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 96);
}

#[test]
fn rep_lods_stop_replay_limits_and_max_count_are_bounded() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xac]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xad]),
        (OperandSize::Dword, vec![0xf3, 0xad]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xad]),
    ];
    let mut stopped = 0;
    for (width, code) in &widths {
        for df in [false, true] {
            for after in [1u64, 2] {
                let size = width.bytes() as usize;
                let mut input = template.clone();
                input.memory[0].length = code.len() as u32;
                input.memory[0].initial_bytes_hex = encode_bytes(code);
                input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                input.memory[1].guest_address = HexU64(0x0020_0000);
                input.memory[1].length = 64;
                input.memory[1].initial_bytes_hex = "a5".repeat(64);
                input.memory[1].comparison_mask_hex = "ff".repeat(64);
                input.memory[1].permissions = MemoryPermissions {
                    read: true,
                    write: false,
                    execute: false,
                };
                let start = if df { 0x0020_0030 } else { 0x0020_0000 };
                input.initial_state.gpr[Register::Rcx as usize] = HexU64(3);
                input.initial_state.gpr[Register::Rsi as usize] = HexU64(start);
                input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                let mut state = input.initial_state.to_cpu().unwrap();
                let mut memory = MovsbAccessLog {
                    inner: FlatMemory::from_fixture(&input).unwrap(),
                    accesses: Vec::new(),
                    mfences: Vec::new(),
                    mfence_error: None,
                };
                let mut polls = 0;
                let first = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || {
                        polls += 1;
                        polls == after as usize + 1
                    },
                );
                assert_eq!(first.exit, RunExit::StopRequested);
                assert_eq!(first.instructions_executed, 0);
                let mut expected_partial = input.initial_state.to_cpu().unwrap();
                expected_partial.gpr[Register::Rcx as usize] = 3 - after;
                expected_partial.gpr[Register::Rsi as usize] = if df {
                    start.wrapping_sub(after * size as u64)
                } else {
                    start + after * size as u64
                };
                expected_partial.write(
                    RegisterView::low(Register::Rax, *width),
                    u64::from_le_bytes([0xa5; 8]) & width.mask(),
                );
                assert_eq!(state, expected_partial);
                assert_eq!(first.rflags_defined, od_isa::INTEGER_FLAGS);
                let prefix: Vec<_> = memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind == AccessKind::Read)
                    .map(|x| x.1)
                    .collect();
                assert_eq!(
                    prefix,
                    (0..after)
                        .map(|k| if df {
                            start.wrapping_sub(k * size as u64)
                        } else {
                            start + k * size as u64
                        })
                        .collect::<Vec<_>>()
                );
                let resumed = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                assert_eq!(resumed.exit, RunExit::StopAddress);
                assert_eq!(resumed.instructions_executed, 1);
                let mut expected_final = input.initial_state.to_cpu().unwrap();
                expected_final.gpr[Register::Rcx as usize] = 0;
                expected_final.gpr[Register::Rsi as usize] = if df {
                    start.wrapping_sub(3 * size as u64)
                } else {
                    start + 3 * size as u64
                };
                expected_final.write(
                    RegisterView::low(Register::Rax, *width),
                    u64::from_le_bytes([0xa5; 8]) & width.mask(),
                );
                expected_final.rip = input.stop_rip.unwrap().get();
                assert_eq!(state, expected_final);
                assert_eq!(resumed.rflags_defined, od_isa::INTEGER_FLAGS);
                let all: Vec<_> = memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind == AccessKind::Read)
                    .map(|x| x.1)
                    .collect();
                assert_eq!(
                    all,
                    (0..3)
                        .map(|k| if df {
                            start.wrapping_sub(k * size as u64)
                        } else {
                            start + k * size as u64
                        })
                        .collect::<Vec<_>>()
                );
                stopped += 1;
            }
        }
    }
    assert_eq!(stopped, 16);
    // Completion advances to the stop address before polling the callback again.
    let (_width, code) = &widths[0];
    let mut completion = template.clone();
    completion.memory[0].length = code.len() as u32;
    completion.memory[0].initial_bytes_hex = encode_bytes(code);
    completion.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    completion.memory[1].length = 1;
    completion.memory[1].initial_bytes_hex = "5a".into();
    completion.memory[1].comparison_mask_hex = "ff".into();
    completion.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    completion.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
    completion.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
    completion.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
    let mut state = completion.initial_state.to_cpu().unwrap();
    let mut memory = FlatMemory::from_fixture(&completion).unwrap();
    let mut polls = 0;
    let result = run(
        &mut state,
        &mut memory,
        config(1, completion.stop_rip.map(HexU64::get)),
        || {
            polls += 1;
            polls > 1
        },
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(polls, 1);

    // A stop requested at the work-limit boundary takes precedence over the limit exit.
    let (width, code) = &widths[3];
    let size = width.bytes() as usize;
    let mut limit_stop = template.clone();
    limit_stop.memory[0].length = code.len() as u32;
    limit_stop.memory[0].initial_bytes_hex = encode_bytes(code);
    limit_stop.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    limit_stop.memory[1].length = (1025 * size) as u32;
    limit_stop.memory[1].initial_bytes_hex = "a5".repeat(1025 * size);
    limit_stop.memory[1].comparison_mask_hex = "ff".repeat(1025 * size);
    limit_stop.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    limit_stop.initial_state.gpr[Register::Rcx as usize] = HexU64(1025);
    limit_stop.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
    let mut state = limit_stop.initial_state.to_cpu().unwrap();
    let mut memory = FlatMemory::from_fixture(&limit_stop).unwrap();
    let mut polls = 0;
    let result = run(
        &mut state,
        &mut memory,
        config(1, limit_stop.stop_rip.map(HexU64::get)),
        || {
            polls += 1;
            polls == 1025
        },
    );
    assert_eq!(result.exit, RunExit::StopRequested);
    assert_eq!(result.instructions_executed, 0);
    let mut expected = limit_stop.initial_state.to_cpu().unwrap();
    expected.gpr[Register::Rcx as usize] = 1;
    expected.gpr[Register::Rsi as usize] = 0x0020_2000;
    expected.write(
        RegisterView::low(Register::Rax, *width),
        u64::from_le_bytes([0xa5; 8]),
    );
    assert_eq!(state, expected);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);

    // A stop predicate already set at entry is first observed after one successful element.
    let (_width, code) = &widths[0];
    let mut input = template.clone();
    input.memory[0].length = code.len() as u32;
    input.memory[0].initial_bytes_hex = encode_bytes(code);
    input.memory[0].comparison_mask_hex = "ffff".into();
    input.memory[1].length = 8;
    input.memory[1].initial_bytes_hex = "a5".repeat(8);
    input.memory[1].comparison_mask_hex = "ff".repeat(8);
    input.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(3);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let before = state.clone();
    let first = run(&mut state, &mut memory, config(1, None), || true);
    assert_eq!(first.exit, RunExit::StopRequested);
    assert_eq!(state, before);
    assert!(
        !memory
            .accesses
            .iter()
            .any(|(kind, _, _)| *kind == AccessKind::Read)
    );

    let mut limits = 0;
    for (width, code) in &widths {
        for df in [false, true] {
            for count in [1023u64, 1024, 1025] {
                let size = width.bytes() as usize;
                let mut input = template.clone();
                input.memory[0].length = code.len() as u32;
                input.memory[0].initial_bytes_hex = encode_bytes(&code);
                input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                let data_len = 1025 * size;
                input.memory[1].guest_address = HexU64(0x0020_0000);
                input.memory[1].length = data_len as u32;
                input.memory[1].initial_bytes_hex = "a5".repeat(data_len);
                input.memory[1].comparison_mask_hex = "ff".repeat(data_len);
                input.memory[1].permissions = MemoryPermissions {
                    read: true,
                    write: false,
                    execute: false,
                };
                let start = if df {
                    0x0020_0000 + 1024 * size as u64
                } else {
                    0x0020_0000
                };
                input.initial_state.gpr[Register::Rcx as usize] = HexU64(count);
                input.initial_state.gpr[Register::Rsi as usize] = HexU64(start);
                input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                let mut state = input.initial_state.to_cpu().unwrap();
                let mut memory = FlatMemory::from_fixture(&input).unwrap();
                let result = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                let done = count.min(1024);
                let mut expected_state = input.initial_state.to_cpu().unwrap();
                expected_state.gpr[Register::Rcx as usize] = count - done;
                expected_state.gpr[Register::Rsi as usize] = if df {
                    start.wrapping_sub(done * size as u64)
                } else {
                    start + done * size as u64
                };
                if done != 0 {
                    expected_state.write(
                        RegisterView::low(Register::Rax, *width),
                        u64::from_le_bytes([0xa5; 8]) & width.mask(),
                    );
                }
                if count <= 1024 {
                    expected_state.rip = input.stop_rip.unwrap().get();
                }
                assert_eq!(state, expected_state);
                assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
                assert_eq!(
                    result.exit,
                    if count <= 1024 {
                        RunExit::StopAddress
                    } else {
                        RunExit::RepIterationBudgetExhausted
                    }
                );
                assert_eq!(result.instructions_executed, u64::from(count <= 1024));
                assert_eq!(state.gpr[Register::Rcx as usize], count - done);
                assert_eq!(
                    state.gpr[Register::Rsi as usize],
                    if df {
                        start.wrapping_sub(done * size as u64)
                    } else {
                        start + done * size as u64
                    }
                );
                assert_eq!(
                    state.gpr[Register::Rax as usize] & width.mask(),
                    if done == 0 {
                        input.initial_state.gpr[Register::Rax as usize].get() & width.mask()
                    } else {
                        u64::from_le_bytes([0xa5; 8]) & width.mask()
                    }
                );
                assert_eq!(memory.snapshot()[1].bytes_hex, "a5".repeat(data_len));
                limits += 1;
            }
        }
    }
    assert_eq!(limits, 24);
    let (width, code) = &widths[3];
    let size = width.bytes() as usize;
    let mut input = template;
    input.memory[0].length = code.len() as u32;
    input.memory[0].initial_bytes_hex = encode_bytes(code);
    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    input.memory[1].length = (1024 * size) as u32;
    input.memory[1].initial_bytes_hex = "a5".repeat(1024 * size);
    input.memory[1].comparison_mask_hex = "ff".repeat(1024 * size);
    input.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(u64::MAX);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted);
    let mut expected_max = input.initial_state.to_cpu().unwrap();
    expected_max.gpr[Register::Rcx as usize] = u64::MAX - 1024;
    expected_max.gpr[Register::Rsi as usize] = 0x0020_2000;
    expected_max.write(
        RegisterView::low(Register::Rax, *width),
        u64::from_le_bytes([0xa5; 8]),
    );
    assert_eq!(state, expected_max);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
}

#[test]
fn rep_lods_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(dir.join("s2_19c_rep_lods_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 36);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::Exception => assert!(
                matches!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    })
                ),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::HarnessError => assert!(
                matches!(result.exit, RunExit::MemoryError(_)),
                "{id}: {:?}",
                result.exit
            ),
            _ => panic!("unexpected REP LODS outcome {id}"),
        }
        if let Some(fault) = answer.get("fault") {
            if input.expected_exit == ExpectedExit::Exception {
                assert_eq!(fault["vector"], 13, "{id}");
                assert_eq!(fault["error_code"], 0, "{id}");
                assert!(fault["linear_address"].is_null(), "{id}");
                assert!(
                    matches!(
                        result.exit,
                        RunExit::Fault(ArchitecturalFault {
                            vector: 13,
                            error_code: Some(0),
                            linear_address: None
                        })
                    ),
                    "{id}"
                );
            } else {
                assert_eq!(fault["access"], "read", "{id}");
                let address: HexU64 = serde_json::from_value(fault["address"].clone()).unwrap();
                assert!(
                    matches!(result.exit, RunExit::MemoryError(MemoryError::Unmapped { address: actual, access: AccessKind::Read }) if actual == address.get()),
                    "{id}: {:?}",
                    result.exit
                );
            }
        } else if id.ends_with("_unmapped_first") {
            assert!(
                matches!(result.exit, RunExit::MemoryError(MemoryError::Unmapped { address: actual, access: AccessKind::Read }) if actual == input.initial_state.gpr[Register::Rsi as usize].get()),
                "{id}: {:?}",
                result.exit
            );
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired = if id.starts_with("s2_19c_test_rep_lods_") {
            if input.expected_exit == ExpectedExit::StopAddress {
                2
            } else {
                1
            }
        } else {
            u64::from(input.expected_exit == ExpectedExit::StopAddress)
        };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn rep_lods_address32_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19j_rep_lods_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 9);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        if input.expected_exit == ExpectedExit::InstructionBudgetExhausted {
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}");
            assert_eq!(result.instructions_executed, 0, "{id}");
        } else if input.expected_exit == ExpectedExit::StopAddress {
            assert_eq!(result.exit, RunExit::StopAddress, "{id}");
            assert_eq!(
                result.instructions_executed,
                u64::from(input.stop_rip != Some(input.initial_state.rip)),
                "{id}"
            );
        } else {
            let fault = &answer["fault"];
            let address: HexU64 = serde_json::from_value(fault["address"].clone()).unwrap();
            assert!(
                matches!(result.exit, RunExit::MemoryError(MemoryError::Unmapped { address: actual, access: AccessKind::Read }) if actual == address.get()),
                "{id}: {:?}",
                result.exit
            );
            assert_eq!(result.instructions_executed, 0, "{id}");
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
    }
}

#[test]
fn rep_stos_address32_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19k_rep_stos_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 11);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}");
            }
            ExpectedExit::HarnessError => {
                let fault = &answer["fault"];
                let address: HexU64 = serde_json::from_value(fault["address"].clone()).unwrap();
                assert!(
                    matches!(result.exit, RunExit::MemoryError(MemoryError::Unmapped { address: actual, access: AccessKind::Write }) if actual == address.get()),
                    "{id}: {:?}",
                    result.exit
                );
            }
            other => panic!("unexpected S2-19k expected exit {other:?}: {id}"),
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let retired = u64::from(
            input.expected_exit == ExpectedExit::StopAddress
                && input.stop_rip != Some(input.initial_state.rip),
        );
        assert_eq!(result.instructions_executed, retired, "{id}");
    }
}

#[test]
fn rep_scasb_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(dir.join("s2_19e_rep_scasb_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 24);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::Exception => assert!(
                matches!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    })
                ),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::HarnessError => assert!(
                matches!(result.exit, RunExit::MemoryError(_)),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::Unsupported => {
                assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xae }, "{id}")
            }
            _ => panic!("unexpected REP SCASB outcome {id}"),
        }
        if let Some(fault) = answer.get("fault") {
            if input.expected_exit == ExpectedExit::Exception {
                assert_eq!(fault["vector"], 13, "{id}");
                assert_eq!(fault["error_code"], 0, "{id}");
                assert!(fault["linear_address"].is_null(), "{id}");
            } else if input.expected_exit == ExpectedExit::HarnessError {
                assert_eq!(fault["access"], "read", "{id}");
                let address: HexU64 = serde_json::from_value(fault["address"].clone()).unwrap();
                assert!(
                    matches!(result.exit, RunExit::MemoryError(MemoryError::Unmapped { address: actual, access: AccessKind::Read }) if actual == address.get()),
                    "{id}: {:?}",
                    result.exit
                );
            }
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired = if id.starts_with("s2_19e_cmp_rax_rax_rep_scasb_success") {
            2
        } else if id.starts_with("s2_19e_cmp_rax_rax_rep_scasb_")
            || id.starts_with("s2_19e_test_rax_rep_scasb_")
        {
            1
        } else {
            u64::from(input.expected_exit == ExpectedExit::StopAddress)
        };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn rep_scasb_addr32_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19m_rep_scasb_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 13);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::HarnessError => {
                assert!(
                    matches!(result.exit, RunExit::MemoryError(_)),
                    "{id}: {:?}",
                    result.exit
                )
            }
            ExpectedExit::Unsupported => {
                let opcode = if id.contains("cmpsd32") {
                    0xa7
                } else if id.ends_with("_af_rejected") {
                    0xaf
                } else {
                    0xae
                };
                assert_eq!(result.exit, RunExit::Unsupported { opcode }, "{id}");
            }
            _ => panic!("unexpected REP SCASB address32 outcome {id}"),
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired = if id.starts_with("s2_19m_cmp_rax_rax") {
            2
        } else if id.starts_with("s2_19m_test_rax") {
            1
        } else {
            u64::from(input.expected_exit == ExpectedExit::StopAddress)
        };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn rep_scas_wdq_addr32_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19o_rep_scas_wdq_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 14);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::HarnessError => assert!(
                matches!(result.exit, RunExit::MemoryError(_)),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::Unsupported => assert_eq!(
                result.exit,
                RunExit::Unsupported {
                    opcode: if id.contains("cmpsd") { 0xa7 } else { 0xaf }
                },
                "{id}"
            ),
            _ => panic!("unexpected S2-19o outcome {id}"),
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired = if id.starts_with("s2_19o_cmp_rax_rax") {
            2
        } else if id.starts_with("s2_19o_test_rax") {
            1
        } else {
            u64::from(input.expected_exit == ExpectedExit::StopAddress)
        };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn rep_cmpsb_addr32_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19n_rep_cmpsb_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 13);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::HarnessError => {
                assert!(
                    matches!(result.exit, RunExit::MemoryError(_)),
                    "{id}: {:?}",
                    result.exit
                )
            }
            ExpectedExit::Unsupported => {
                let opcode = if id.contains("cmpsd32") { 0xa7 } else { 0xa6 };
                assert_eq!(result.exit, RunExit::Unsupported { opcode }, "{id}");
            }
            _ => panic!("unexpected REP CMPSB address32 outcome {id}"),
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired = if id.starts_with("s2_19n_cmp_rax_rax") {
            2
        } else if id.starts_with("s2_19n_test_rax") {
            1
        } else {
            u64::from(input.expected_exit == ExpectedExit::StopAddress)
        };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn rep_cmps_wdq_addr32_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19p_rep_cmps_wdq_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 14);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::HarnessError => assert!(
                matches!(result.exit, RunExit::MemoryError(_)),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::Unsupported => {
                assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xa7 }, "{id}");
            }
            _ => panic!("unexpected S2-19p outcome {id}"),
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired = if id.starts_with("s2_19p_cmp_to") {
            2
        } else if id.starts_with("s2_19p_test_to") {
            1
        } else {
            u64::from(input.expected_exit == ExpectedExit::StopAddress)
        };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn repne_scasb_addr32_testkit_sidecar_carries_token_across_runs() {
    let mut input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_19m_scasb32_zero_f3_67.case.json"
    ))
    .unwrap()
    .fixture;
    input.memory[0].initial_bytes_hex = "f267ae".into();
    input.memory[0].comparison_mask_hex = "ffffff".into();
    input.memory[1].initial_bytes_hex = "4241".into();
    input.memory[1].length = 2;
    input.memory[1].comparison_mask_hex = "ffff".into();
    input.stop_rip = None;
    input.expected_exit = ExpectedExit::InstructionBudgetExhausted;
    input.initial_state.rip = HexU64(0x0010_0000);
    input.initial_state.gpr[0] = HexU64(0x41);
    input.initial_state.gpr[1] = HexU64(2);
    input.initial_state.gpr[7] = HexU64(0x0030_0000);
    input.initial_state.rflags = HexU64(2);
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let mut continuation = None;
    let mut stop_calls = 0;
    let first = run_with_rep_continuation(
        &mut state,
        &mut memory,
        config(1, None),
        od_isa::INTEGER_FLAGS,
        &mut continuation,
        || {
            stop_calls += 1;
            stop_calls == 2
        },
    )
    .unwrap();
    assert_eq!(first.exit, RunExit::StopRequested);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x0030_0001);
    assert!(continuation.is_some());
    let resumed = run_with_rep_continuation(
        &mut state,
        &mut memory,
        config(1, None),
        first.rflags_defined,
        &mut continuation,
        || false,
    )
    .unwrap();
    assert_eq!(resumed.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, 0x0010_0003);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x0030_0002);
    assert_eq!(state.rflags, 0x46);
    assert!(continuation.is_none());
}

#[test]
fn repne_scasb_addr32_literal_fixtures_use_fresh_continuation_tokens() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19q_repne_scasb_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 2);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut continuation = None;
        let result = run_with_rep_continuation(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert_eq!(result.exit, RunExit::StopAddress, "{id}");
        assert_eq!(result.instructions_executed, 1, "{id}");
        assert!(continuation.is_none(), "one-shot fixture completed: {id}");
        let gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        assert_eq!(state.gpr, gpr.map(HexU64::get), "{id}");
        let flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let defined: HexU64 = serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.rflags, flags.get(), "{id}");
        assert_eq!(result.rflags_defined, defined.get(), "{id}");
        assert_eq!(state.rip, rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
    }
}

#[test]
fn repne_scas_wdq_addr32_literal_fixtures_use_fresh_continuation_tokens() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19s_repne_scas_wdq_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 3);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut continuation = None;
        let result = run_with_rep_continuation(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert_eq!(result.exit, RunExit::StopAddress, "{id}");
        assert_eq!(result.instructions_executed, 1, "{id}");
        assert!(continuation.is_none(), "one-shot fixture completed: {id}");
        let gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        assert_eq!(state.gpr, gpr.map(HexU64::get), "{id}");
        let flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let defined: HexU64 = serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.rflags, flags.get(), "{id}");
        assert_eq!(result.rflags_defined, defined.get(), "{id}");
        assert_eq!(state.rip, rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
    }
}

#[test]
fn repne_cmps_wdq_addr32_literal_fixtures_use_fresh_continuation_tokens() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19t_repne_cmps_wdq_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 3);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut continuation = None;
        let result = run_with_rep_continuation(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert_eq!(result.exit, RunExit::StopAddress, "{id}");
        assert_eq!(result.instructions_executed, 1, "{id}");
        assert!(continuation.is_none(), "one-shot token: {id}");
        let gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        assert_eq!(state.gpr, gpr.map(HexU64::get), "{id}");
        let flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let defined: HexU64 = serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.rflags, flags.get(), "{id}");
        assert_eq!(result.rflags_defined, defined.get(), "{id}");
        assert_eq!(state.rip, rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
    }

    let mut input = Fixture::from_json(
        &std::fs::read(dir.join("s2_19t_repne_cmpsd_mismatch.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(1026);
    for region in input
        .memory
        .iter_mut()
        .filter(|region| region.region_id != "code")
    {
        region.length = 4104;
        region.comparison_mask_hex = "ff".repeat(4104);
        region.initial_bytes_hex = region.initial_bytes_hex.repeat(1026);
        if region.region_id == "destination" {
            region.guest_address = HexU64(0x5000);
        }
    }
    input.initial_state.gpr[Register::Rdi as usize] = HexU64(0x5000);
    input.stop_rip = None;
    input.expected_exit = ExpectedExit::InstructionBudgetExhausted;
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let mut continuation = None;
    let mut callbacks = 0;
    let first = run_with_rep_continuation(
        &mut state,
        &mut memory,
        config(1, None),
        od_isa::INTEGER_FLAGS,
        &mut continuation,
        || {
            callbacks += 1;
            callbacks == 2
        },
    )
    .unwrap();
    assert_eq!(first.exit, RunExit::StopRequested);
    assert_eq!(state.gpr[Register::Rcx as usize], 1025);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x3004);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x5004);
    assert!(continuation.is_some());
    let resumed = run_with_rep_continuation(
        &mut state,
        &mut memory,
        config(1, None),
        first.rflags_defined,
        &mut continuation,
        || false,
    )
    .unwrap();
    assert_eq!(resumed.exit, RunExit::RepIterationBudgetExhausted);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x4004);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x6004);
    assert!(continuation.is_some());
    let completed = run_with_rep_continuation(
        &mut state,
        &mut memory,
        config(1, Some(0x0010_0003)),
        resumed.rflags_defined,
        &mut continuation,
        || false,
    )
    .unwrap();
    assert_eq!(completed.exit, RunExit::StopAddress);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x4008);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x6008);
    assert!(continuation.is_none());
}

#[test]
fn repne_cmps_wdq_addr32_dual_invalid_addresses_keep_source_first_trace() {
    for (size, code) in [
        (OperandSize::Word, vec![0xf2, 0x67, 0x66, 0xa7]),
        (OperandSize::Dword, vec![0x67, 0xf2, 0xa7]),
        (OperandSize::Qword, vec![0xf2, 0x67, 0x48, 0xa7]),
    ] {
        let mut fixture = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_19t_repne_cmpsw_equal.case.json"
        ))
        .unwrap()
        .fixture;
        fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
        fixture.memory[0].length = code.len() as u32;
        fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        fixture.memory.truncate(1);
        fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
        fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(0x7000);
        fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x8000);
        fixture.initial_state.rflags = HexU64(2);
        fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let mut memory = AccessTraceMemory {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let mut continuation = None;
        let result = od_isa::execute::run_with_rep_continuation(
            &mut state,
            &mut memory,
            config(1, fixture.stop_rip.map(HexU64::get)),
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert!(matches!(
            result.exit,
            RunExit::MemoryError(MemoryError::Unmapped {
                address: 0x7000,
                access: AccessKind::Read
            })
        ));
        let reads: Vec<_> = memory
            .accesses
            .iter()
            .filter(|(kind, _, _, _)| *kind == AccessKind::Read)
            .map(|(kind, address, length, _)| (*kind, *address, *length))
            .collect();
        assert_eq!(reads, [(AccessKind::Read, 0x7000, size.bytes())]);
        assert_eq!(state.gpr[Register::Rcx as usize], 1);
        assert_eq!(state.gpr[Register::Rsi as usize], 0x7000);
        assert_eq!(state.gpr[Register::Rdi as usize], 0x8000);
        assert_eq!(state.rflags, 2);
    }
}

#[test]
fn repne_cmps_wdq_addr32_uses_contiguous_cross_4g_reads_and_width_exact_tail_accesses() {
    let forms = [
        (OperandSize::Word, vec![0xf2, 0x67, 0x66, 0xa7]),
        (OperandSize::Dword, vec![0x67, 0xf2, 0xa7]),
        (OperandSize::Qword, vec![0xf2, 0x67, 0x48, 0xa7]),
    ];
    for (size, code) in forms {
        let width = size.bytes() as usize;
        let boundary = u32::MAX - (width as u32 / 2 - 1);
        for source_crosses in [true, false] {
            let mut fixture = Fixture::from_json(include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_19t_repne_cmpsw_equal.case.json"
            ))
            .unwrap()
            .fixture;
            fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
            fixture.memory[0].length = code.len() as u32;
            fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            let source = &mut fixture.memory[1];
            source.guest_address = HexU64(if source_crosses {
                u64::from(boundary)
            } else {
                0x3000
            });
            source.length = width as u32;
            source.initial_bytes_hex = "55".repeat(width);
            source.comparison_mask_hex = "ff".repeat(width);
            let destination = &mut fixture.memory[2];
            destination.guest_address = HexU64(if source_crosses {
                0x3100
            } else {
                u64::from(boundary)
            });
            destination.length = width as u32;
            destination.initial_bytes_hex = "55".repeat(width);
            destination.comparison_mask_hex = "ff".repeat(width);
            fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
            fixture.initial_state.gpr[Register::Rsi as usize] = HexU64(
                0xfeed_face_0000_0000 | u64::from(if source_crosses { boundary } else { 0x3000 }),
            );
            fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(
                0xcafe_babe_0000_0000
                    | u64::from(if source_crosses { 0x3100u32 } else { boundary }),
            );
            fixture.initial_state.rflags = HexU64(2);
            fixture.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let mut memory = AccessTraceMemory {
                inner: FlatMemory::from_fixture(&fixture).unwrap(),
                accesses: Vec::new(),
                fetch_limit: None,
            };
            let mut continuation = None;
            let result = od_isa::execute::run_with_rep_continuation(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                od_isa::INTEGER_FLAGS,
                &mut continuation,
                || false,
            )
            .unwrap();
            assert_eq!(result.exit, RunExit::StopAddress);
            assert_eq!(
                state.gpr[Register::Rsi as usize],
                if source_crosses {
                    u64::from(boundary.wrapping_add(width as u32))
                } else {
                    0x3000 + width as u64
                }
            );
            assert_eq!(
                state.gpr[Register::Rdi as usize],
                if source_crosses {
                    0x3100 + width as u64
                } else {
                    u64::from(boundary.wrapping_add(width as u32))
                }
            );
            assert_eq!(
                memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _, _)| *kind == AccessKind::Read)
                    .map(|(kind, address, length, _)| (*kind, *address, *length))
                    .collect::<Vec<_>>(),
                [
                    (
                        AccessKind::Read,
                        fixture.memory[1].guest_address.get(),
                        width
                    ),
                    (
                        AccessKind::Read,
                        fixture.memory[2].guest_address.get(),
                        width
                    )
                ]
            );
            assert!(continuation.is_none());
        }
    }

    let code = [0xf2, 0x67, 0x48, 0xa7];
    for failed_operand in [0usize, 1] {
        let mut fixture = Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_19t_repne_cmpsw_equal.case.json"
        ))
        .unwrap()
        .fixture;
        fixture.memory[0].initial_bytes_hex = encode_bytes(&code);
        fixture.memory[0].length = code.len() as u32;
        fixture.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        fixture.memory[1].guest_address = HexU64(if failed_operand == 0 {
            0xffff_fffc
        } else {
            0x3000
        });
        fixture.memory[1].length = if failed_operand == 0 { 7 } else { 8 };
        fixture.memory[1].initial_bytes_hex = "55".repeat(fixture.memory[1].length as usize);
        fixture.memory[1].comparison_mask_hex = "ff".repeat(fixture.memory[1].length as usize);
        fixture.memory[2].guest_address = HexU64(if failed_operand == 1 {
            0xffff_fffc
        } else {
            0x3100
        });
        fixture.memory[2].length = if failed_operand == 1 { 7 } else { 8 };
        fixture.memory[2].initial_bytes_hex = "55".repeat(fixture.memory[2].length as usize);
        fixture.memory[2].comparison_mask_hex = "ff".repeat(fixture.memory[2].length as usize);
        fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(2);
        fixture.initial_state.gpr[Register::Rsi as usize] =
            HexU64(fixture.memory[1].guest_address.get());
        fixture.initial_state.gpr[Register::Rdi as usize] =
            HexU64(fixture.memory[2].guest_address.get());
        let before = fixture.initial_state.clone();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let mut memory = AccessTraceMemory {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let mut continuation = None;
        let result = od_isa::execute::run_with_rep_continuation(
            &mut state,
            &mut memory,
            config(1, None),
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert!(matches!(result.exit, RunExit::MemoryError(_)));
        assert_eq!(
            state.gpr[Register::Rcx as usize],
            before.gpr[Register::Rcx as usize].get()
        );
        assert_eq!(state.rflags, before.rflags.get());
        let reads: Vec<_> = memory
            .accesses
            .iter()
            .filter(|(kind, _, _, _)| *kind == AccessKind::Read)
            .map(|(_, address, length, _)| (*address, *length))
            .collect();
        assert_eq!(
            reads,
            if failed_operand == 0 {
                vec![(0xffff_fffc, 8)]
            } else {
                vec![(0x3000, 8), (0xffff_fffc, 8)]
            }
        );
        assert!(continuation.is_some());
    }
}

#[test]
fn repne_cmpsb_addr32_continuation_sidecar_and_literal_fixtures() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_19r_repne_cmpsb_addr32_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 2);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(directory.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut continuation = None;
        let result = run_with_rep_continuation(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            || false,
        )
        .unwrap();
        assert_eq!(result.exit, RunExit::StopAddress, "{id}");
        assert_eq!(result.instructions_executed, 1, "{id}");
        assert!(continuation.is_none(), "one-shot token: {id}");
        let gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        assert_eq!(state.gpr, gpr.map(HexU64::get), "{id}");
        let flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let defined: HexU64 = serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.rflags, flags.get(), "{id}");
        assert_eq!(result.rflags_defined, defined.get(), "{id}");
        assert_eq!(state.rip, rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
    }

    let mut input = Fixture::from_json(
        &std::fs::read(directory.join("s2_19r_cmpsb_mismatch_then_match.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    input.stop_rip = None;
    input.expected_exit = ExpectedExit::InstructionBudgetExhausted;
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let mut continuation = None;
    let mut callbacks = 0;
    let first = run_with_rep_continuation(
        &mut state,
        &mut memory,
        config(1, None),
        od_isa::INTEGER_FLAGS,
        &mut continuation,
        || {
            callbacks += 1;
            callbacks == 2
        },
    )
    .unwrap();
    assert_eq!(first.exit, RunExit::StopRequested);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert!(continuation.is_some());
    let resumed = run_with_rep_continuation(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(0x0010_0003),
            context: AccessContext::default(),
        },
        first.rflags_defined,
        &mut continuation,
        || false,
    )
    .unwrap();
    assert_eq!(resumed.exit, RunExit::StopAddress);
    assert_eq!(state.rip, 0x0010_0003);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert!(continuation.is_none());
}

#[test]
fn rep_scas_wdq_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(dir.join("s2_19g_rep_scas_wdq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 59);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::Exception => assert!(
                matches!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    })
                ),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::HarnessError => assert!(
                matches!(result.exit, RunExit::MemoryError(_)),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::Unsupported => {
                assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xaf }, "{id}")
            }
            _ => panic!("unexpected REPE SCASW/D/Q outcome {id}"),
        }
        if let Some(fault) = answer.get("fault") {
            if input.expected_exit == ExpectedExit::Exception {
                assert_eq!(fault["vector"], 13, "{id}");
                assert_eq!(fault["error_code"], 0, "{id}");
                assert!(fault["linear_address"].is_null(), "{id}");
            } else if input.expected_exit == ExpectedExit::HarnessError {
                assert_eq!(fault["access"], "read", "{id}");
                let address: HexU64 = serde_json::from_value(fault["address"].clone()).unwrap();
                assert!(
                    matches!(result.exit, RunExit::MemoryError(MemoryError::Unmapped { address: actual, access: AccessKind::Read }) if actual == address.get()),
                    "{id}: {:?}",
                    result.exit
                );
            }
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired =
            if id.starts_with("s2_19g_cmp_rax_rax_rep_scas_") && id.ends_with("_success") {
                2
            } else if id.starts_with("s2_19g_cmp_rax_rax_rep_scas_")
                || id.starts_with("s2_19g_test_rax_rep_scas_")
            {
                1
            } else {
                u64::from(input.expected_exit == ExpectedExit::StopAddress)
            };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn rep_cmpsb_literal_fixtures_match_full_expected_answers() {
    let dir = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(dir.join("s2_19f_rep_cmpsb_expected.json")).unwrap())
            .unwrap();
    assert_eq!(answers.len(), 31);
    for (id, answer) in answers {
        let input =
            Fixture::from_json(&std::fs::read(dir.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted, "{id}")
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted, "{id}")
            }
            ExpectedExit::Exception => assert!(
                matches!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None
                    })
                ),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::HarnessError => assert!(
                matches!(result.exit, RunExit::MemoryError(_)),
                "{id}: {:?}",
                result.exit
            ),
            ExpectedExit::Unsupported => {
                assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xa6 }, "{id}")
            }
            _ => panic!("unexpected REP CMPSB outcome {id}"),
        }
        if let Some(fault) = answer.get("fault") {
            if input.expected_exit == ExpectedExit::Exception {
                assert_eq!(fault["vector"], 13, "{id}");
                assert_eq!(fault["error_code"], 0, "{id}");
                assert!(fault["linear_address"].is_null(), "{id}");
            } else if input.expected_exit == ExpectedExit::HarnessError {
                assert_eq!(fault["access"], "read", "{id}");
                let address: HexU64 = serde_json::from_value(fault["address"].clone()).unwrap();
                assert!(
                    matches!(result.exit, RunExit::MemoryError(MemoryError::Unmapped { address: actual, access: AccessKind::Read }) if actual == address.get()),
                    "{id}: {:?}",
                    result.exit
                );
            }
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{id}");
        assert_eq!(state.rflags, expected_flags.get(), "{id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{id}");
        assert_eq!(state.rip, expected_rip.get(), "{id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(region.bytes_hex, expected_memory[&region.region_id], "{id}");
        }
        let expected_retired = if id.starts_with("s2_19f_cmp_rax_rax_rep_cmpsb_success") {
            2
        } else if id.starts_with("s2_19f_cmp_rax_rax_rep_cmpsb_")
            || id.starts_with("s2_19f_test_rax_rep_cmpsb_")
        {
            1
        } else {
            u64::from(input.expected_exit == ExpectedExit::StopAddress)
        };
        assert_eq!(result.instructions_executed, expected_retired, "{id}");
    }
}

#[test]
fn rep_lods_alignment_page_wrap_and_next_pointer_edges() {
    let dir = arithmetic_fixture_directory();
    let mut input =
        Fixture::from_json(&std::fs::read(dir.join("s2_19a_rep_stosb_one.case.json")).unwrap())
            .unwrap()
            .fixture;
    // An unaligned qword read crossing a 4 KiB boundary is one exact access.
    let code = [0xf3, 0x48, 0xad];
    input.memory[0].length = 3;
    input.memory[0].initial_bytes_hex = encode_bytes(&code);
    input.memory[0].comparison_mask_hex = "ffffff".into();
    input.memory[1].guest_address = HexU64(0x0020_0ff8);
    input.memory[1].length = 16;
    input.memory[1].initial_bytes_hex = "0102030405060708090a0b0c0d0e0f10".into();
    input.memory[1].comparison_mask_hex = "ff".repeat(16);
    input.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0ffc);
    input.stop_rip = Some(HexU64(0x0010_0003));
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(
        &mut state,
        &mut memory,
        config(1, input.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(state.gpr[Register::Rax as usize], 0x0c0b0a0908070605);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x0020_1004);
    assert!(
        memory
            .accesses
            .contains(&(AccessKind::Read, 0x0020_0ffc, 8))
    );

    // The final low-canonical byte read succeeds; the next REP element faults at the now-noncanonical RSI.
    let code = [0xf3, 0xac];
    input.memory[0].length = 2;
    input.memory[0].initial_bytes_hex = encode_bytes(&code);
    input.memory[0].comparison_mask_hex = "ffff".into();
    input.memory[1].guest_address = HexU64(0x0000_7fff_ffff_ffff);
    input.memory[1].length = 1;
    input.memory[1].initial_bytes_hex = "5a".into();
    input.memory[1].comparison_mask_hex = "ff".into();
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(2);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0000_7fff_ffff_ffff);
    input.stop_rip = Some(HexU64(0x0010_0002));
    let mut state = input.initial_state.to_cpu().unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            ..
        })
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state.gpr[Register::Rax as usize] & 0xff, 0x5a);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x0000_8000_0000_0000);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(
        state.gpr[Register::Rax as usize] & !0xff,
        before.gpr[Register::Rax as usize] & !0xff
    );

    // RSI decrement wraps at zero only after the first successful byte read.
    input.memory[0].length = 2;
    input.memory[0].initial_bytes_hex = encode_bytes(&[0xf3, 0xac]);
    input.memory[0].comparison_mask_hex = "ffff".into();
    input.memory[1].guest_address = HexU64(0);
    input.memory[1].length = 1;
    input.memory[1].initial_bytes_hex = "5a".into();
    input.memory[1].comparison_mask_hex = "ff".into();
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(2);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0);
    input.initial_state.rflags = HexU64(0x0cd7);
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state.rip, input.initial_state.rip.get());
    assert_eq!(state.gpr[Register::Rax as usize] & 0xff, 0x5a);
    assert_eq!(state.gpr[Register::Rsi as usize], u64::MAX);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.rflags, input.initial_state.rflags.get());
    assert_eq!(
        memory
            .accesses
            .iter()
            .filter(|(kind, _, _)| *kind == AccessKind::Read)
            .map(|x| x.1)
            .collect::<Vec<_>>(),
        [0, u64::MAX]
    );

    // A width-sized source range that extends past its only mapped byte range commits nothing.
    input.memory[0].length = 3;
    input.memory[0].initial_bytes_hex = encode_bytes(&[0xf3, 0xad, 0x00]);
    input.memory[0].comparison_mask_hex = "ffffff".into();
    input.memory[1].guest_address = HexU64(0x0020_0000);
    input.memory[1].length = 3;
    input.memory[1].initial_bytes_hex = "010203".into();
    input.memory[1].comparison_mask_hex = "ffffff".into();
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
    input.initial_state.rflags = HexU64(0x08d7);
    let mut state = input.initial_state.to_cpu().unwrap();
    let before = state.clone();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Read,
            ..
        })
    ));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
}

#[test]
fn rep_movs_wdq_complete_state_matrix_and_alternating_access_order() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xa4], false),
        (OperandSize::Word, vec![0xf3, 0x66, 0xa5], false),
        (OperandSize::Dword, vec![0xf3, 0xa5], false),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xa5], false),
        (OperandSize::Byte, vec![0xf3, 0x67, 0xa4], true),
        (OperandSize::Word, vec![0xf3, 0x67, 0x66, 0xa5], true),
        (OperandSize::Dword, vec![0xf3, 0x67, 0xa5], true),
        (OperandSize::Qword, vec![0xf3, 0x67, 0x48, 0xa5], true),
    ];
    let flag_bits = [0, 2, 4, 6, 7, 11];
    let destination_base = 0x0030_0000;
    let mut cases = 0;
    for (width, code, addr32) in &widths {
        let size = width.bytes() as usize;
        let mask = width.mask();
        let sign = 1u64 << (size * 8 - 1);
        for df in [false, true] {
            for seed in [0, 1, mask, sign, 0x55 & mask, 0xaa & mask] {
                for count in 0..=3u64 {
                    for pattern in 0..64usize {
                        let mut input = template.clone();
                        input.memory[0].length = code.len() as u32;
                        input.memory[0].initial_bytes_hex = encode_bytes(code);
                        input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                        input.memory[1].guest_address = HexU64(0x0020_0000);
                        input.memory[1].length = (size * 4) as u32;
                        let mut source_bytes = Vec::new();
                        for k in 0..4u64 {
                            source_bytes.extend_from_slice(
                                &(seed.wrapping_add(k) & mask).to_le_bytes()[..size],
                            );
                        }
                        input.memory[1].initial_bytes_hex = encode_bytes(&source_bytes);
                        input.memory[1].comparison_mask_hex = "ff".repeat(source_bytes.len());
                        input.memory[1].permissions = MemoryPermissions {
                            read: true,
                            write: false,
                            execute: false,
                        };
                        input.memory.push(crate::case::MemoryRegionSpec {
                            region_id: "destination".to_string(),
                            guest_address: HexU64(destination_base),
                            length: (size * 4) as u32,
                            permissions: MemoryPermissions {
                                read: true,
                                write: true,
                                execute: false,
                            },
                            initial_bytes_hex: "a5".repeat(size * 4),
                            comparison_mask_hex: "ff".repeat(size * 4),
                        });
                        input.initial_state.gpr[Register::Rcx as usize] = HexU64(if *addr32 {
                            0xfeed_beef_0000_0000 | count
                        } else {
                            count
                        });
                        let src_start = if df {
                            0x0020_0000 + 2 * size as u64
                        } else {
                            0x0020_0000
                        };
                        let dst_start = if df {
                            destination_base + 2 * size as u64
                        } else {
                            destination_base
                        };
                        input.initial_state.gpr[Register::Rsi as usize] =
                            HexU64(src_start | if *addr32 { 0xaaaa_bbbb_0000_0000 } else { 0 });
                        input.initial_state.gpr[Register::Rdi as usize] =
                            HexU64(dst_start | if *addr32 { 0xcccc_dddd_0000_0000 } else { 0 });
                        input.initial_state.rflags = HexU64(
                            0x2 | if df { 1 << 10 } else { 0 }
                                | flag_bits
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, _)| pattern & (1 << i) != 0)
                                    .fold(0, |v, (_, b)| v | (1 << b)),
                        );
                        input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                        let before = input.initial_state.to_cpu().unwrap();
                        let mut state = before.clone();
                        let mut memory = MovsbAccessLog {
                            inner: FlatMemory::from_fixture(&input).unwrap(),
                            accesses: Vec::new(),
                            mfences: Vec::new(),
                            mfence_error: None,
                        };
                        let result = run(
                            &mut state,
                            &mut memory,
                            config(1, input.stop_rip.map(HexU64::get)),
                            || false,
                        );
                        assert_eq!(
                            result.exit,
                            RunExit::StopAddress,
                            "code={code:02x?} addr32={addr32}"
                        );
                        assert_eq!(result.instructions_executed, 1);
                        let step = if df {
                            0u64.wrapping_sub(size as u64)
                        } else {
                            size as u64
                        };
                        let mut expected = before;
                        if count != 0 {
                            expected.gpr[Register::Rcx as usize] = 0;
                            expected.gpr[Register::Rsi as usize] = if *addr32 {
                                u64::from(
                                    (src_start as u32)
                                        .wrapping_add((step.wrapping_mul(count)) as u32),
                                )
                            } else {
                                src_start.wrapping_add(step.wrapping_mul(count))
                            };
                            expected.gpr[Register::Rdi as usize] = if *addr32 {
                                u64::from(
                                    (dst_start as u32)
                                        .wrapping_add((step.wrapping_mul(count)) as u32),
                                )
                            } else {
                                dst_start.wrapping_add(step.wrapping_mul(count))
                            };
                        }
                        expected.rip = input.stop_rip.unwrap().get();
                        assert_eq!(
                            state, expected,
                            "{width:?} addr32={addr32} DF={df} seed={seed:#x} count={count} flags={pattern}"
                        );
                        let expected_accesses: Vec<_> = (0..count)
                            .flat_map(|k| {
                                let offset = step.wrapping_mul(k);
                                [
                                    (AccessKind::Read, src_start.wrapping_add(offset), size),
                                    (AccessKind::Write, dst_start.wrapping_add(offset), size),
                                ]
                            })
                            .collect();
                        let data_accesses: Vec<_> = memory
                            .accesses
                            .iter()
                            .copied()
                            .filter(|(kind, _, _)| {
                                matches!(kind, AccessKind::Read | AccessKind::Write)
                            })
                            .collect();
                        assert_eq!(data_accesses, expected_accesses);
                        let mut expected_destination = vec![0xa5; size * 4];
                        for k in 0..count {
                            let index = if df { 2 - k as usize } else { k as usize };
                            expected_destination[index * size..(index + 1) * size]
                                .copy_from_slice(&source_bytes[index * size..(index + 1) * size]);
                        }
                        assert_eq!(
                            memory.inner.snapshot()[1].bytes_hex,
                            encode_bytes(&source_bytes)
                        );
                        assert_eq!(
                            memory.inner.snapshot()[2].bytes_hex,
                            encode_bytes(&expected_destination)
                        );
                        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 24_576);
}

#[test]
fn rep_movs_faults_preserve_only_fully_transferred_iterations() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xa4]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xa5]),
        (OperandSize::Dword, vec![0xf3, 0xa5]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xa5]),
        (OperandSize::Byte, vec![0xf3, 0x67, 0xa4]),
        (OperandSize::Word, vec![0xf3, 0x67, 0x66, 0xa5]),
        (OperandSize::Dword, vec![0xf3, 0x67, 0xa5]),
        (OperandSize::Qword, vec![0xf3, 0x67, 0x48, 0xa5]),
    ];
    let failures = [
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(4),
            linear_address: Some(0),
        }),
        MemoryError::Unmapped {
            address: 0,
            access: AccessKind::Read,
        },
        MemoryError::PermissionDenied {
            address: 0,
            access: AccessKind::Read,
        },
        MemoryError::Backend("injected transfer failure".into()),
    ];
    let mut cases = 0;
    for (width, code) in widths {
        let addr32 = code.get(1) == Some(&0x67);
        let size = width.bytes() as usize;
        for df in [false, true] {
            for successes in 0..=2u64 {
                for site in [AccessKind::Read, AccessKind::Write] {
                    for failure in &failures {
                        let mut input = template.clone();
                        input.memory[0].length = code.len() as u32;
                        input.memory[0].initial_bytes_hex = encode_bytes(&code);
                        input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                        input.memory[1].guest_address = HexU64(0x0020_0000);
                        input.memory[1].length = 64;
                        input.memory[1].initial_bytes_hex = "5a".repeat(64);
                        input.memory[1].comparison_mask_hex = "ff".repeat(64);
                        input.memory[1].permissions = MemoryPermissions {
                            read: true,
                            write: false,
                            execute: false,
                        };
                        input.memory.push(crate::case::MemoryRegionSpec {
                            region_id: "destination".into(),
                            guest_address: HexU64(0x0030_0000),
                            length: 64,
                            permissions: MemoryPermissions {
                                read: true,
                                write: true,
                                execute: false,
                            },
                            initial_bytes_hex: "a5".repeat(64),
                            comparison_mask_hex: "ff".repeat(64),
                        });
                        let src = if df { 0x0020_0030 } else { 0x0020_0000 };
                        let dst = if df { 0x0030_0030 } else { 0x0030_0000 };
                        input.initial_state.gpr[Register::Rcx as usize] = HexU64(if addr32 {
                            0xfeed_beef_0000_0000 | (successes + 1)
                        } else {
                            successes + 1
                        });
                        input.initial_state.gpr[Register::Rsi as usize] =
                            HexU64(src | if addr32 { 0xaaaa_bbbb_0000_0000 } else { 0 });
                        input.initial_state.gpr[Register::Rdi as usize] =
                            HexU64(dst | if addr32 { 0xcccc_dddd_0000_0000 } else { 0 });
                        input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                        let offset = if df {
                            0u64.wrapping_sub(size as u64 * successes)
                        } else {
                            size as u64 * successes
                        };
                        let address =
                            (if site == AccessKind::Read { src } else { dst }).wrapping_add(offset);
                        let injected = match failure {
                            MemoryError::Architectural(f) => {
                                MemoryError::Architectural(ArchitecturalFault {
                                    linear_address: Some(address),
                                    ..f.clone()
                                })
                            }
                            MemoryError::Unmapped { access, .. } => MemoryError::Unmapped {
                                address,
                                access: if site == AccessKind::Read {
                                    *access
                                } else {
                                    site
                                },
                            },
                            MemoryError::PermissionDenied { access, .. } => {
                                MemoryError::PermissionDenied {
                                    address,
                                    access: if site == AccessKind::Read {
                                        *access
                                    } else {
                                        site
                                    },
                                }
                            }
                            other => other.clone(),
                        };
                        let before = input.initial_state.to_cpu().unwrap();
                        let mut state = before.clone();
                        let mut memory = MovsbAccessLog {
                            inner: FlatMemory::from_fixture(&input).unwrap(),
                            accesses: Vec::new(),
                            mfences: Vec::new(),
                            mfence_error: None,
                        };
                        memory.inner.inject_once(address, site, injected.clone());
                        let result = run(&mut state, &mut memory, config(1, None), || false);
                        match injected {
                            MemoryError::Architectural(f) => {
                                assert_eq!(result.exit, RunExit::Fault(f))
                            }
                            other => assert_eq!(result.exit, RunExit::MemoryError(other)),
                        }
                        assert_eq!(result.instructions_executed, 0);
                        let step = if df {
                            0u64.wrapping_sub(size as u64)
                        } else {
                            size as u64
                        };
                        let mut expected = before;
                        if addr32 && successes == 0 {
                            expected.gpr[Register::Rcx as usize] = 0xfeed_beef_0000_0001;
                        } else {
                            expected.gpr[Register::Rcx as usize] = 1;
                        }
                        if successes != 0 {
                            expected.gpr[Register::Rsi as usize] = if addr32 {
                                u64::from(
                                    (src as u32)
                                        .wrapping_add((step.wrapping_mul(successes)) as u32),
                                )
                            } else {
                                src.wrapping_add(step.wrapping_mul(successes))
                            };
                            expected.gpr[Register::Rdi as usize] = if addr32 {
                                u64::from(
                                    (dst as u32)
                                        .wrapping_add((step.wrapping_mul(successes)) as u32),
                                )
                            } else {
                                dst.wrapping_add(step.wrapping_mul(successes))
                            };
                        }
                        assert_eq!(
                            state, expected,
                            "{width:?} addr32={addr32} DF={df} successes={successes} site={site:?} failure={failure:?}"
                        );
                        let mut expected_log = Vec::new();
                        for k in 0..successes {
                            let off = step.wrapping_mul(k);
                            expected_log.push((AccessKind::Read, src.wrapping_add(off), size));
                            expected_log.push((AccessKind::Write, dst.wrapping_add(off), size));
                        }
                        if site == AccessKind::Read {
                            expected_log.push((site, address, size));
                        } else {
                            expected_log.push((AccessKind::Read, src.wrapping_add(offset), size));
                            expected_log.push((site, address, size));
                        }
                        let actual: Vec<_> = memory
                            .accesses
                            .iter()
                            .copied()
                            .filter(|(k, _, _)| matches!(k, AccessKind::Read | AccessKind::Write))
                            .collect();
                        assert_eq!(actual, expected_log);
                        assert_eq!(memory.inner.snapshot()[1].bytes_hex, "5a".repeat(64));
                        let mut expected_destination = vec![0xa5; 64];
                        for k in 0..successes {
                            let dst_address = dst.wrapping_add(step.wrapping_mul(k));
                            let offset = (dst_address - 0x0030_0000) as usize;
                            expected_destination[offset..offset + size].fill(0x5a);
                        }
                        assert_eq!(
                            memory.inner.snapshot()[2].bytes_hex,
                            encode_bytes(&expected_destination)
                        );
                        let before_retry = state.clone();
                        let retry = run(
                            &mut state,
                            &mut memory,
                            config(1, Some(0x0010_0000 + code.len() as u64)),
                            || false,
                        );
                        assert_eq!(retry.exit, RunExit::StopAddress);
                        assert_eq!(retry.instructions_executed, 1);
                        assert_eq!(state.gpr[Register::Rcx as usize], 0);
                        let next = |value: u64| {
                            if addr32 {
                                u64::from(if df {
                                    (value as u32).wrapping_sub(size as u32)
                                } else {
                                    (value as u32).wrapping_add(size as u32)
                                })
                            } else if df {
                                value.wrapping_sub(size as u64)
                            } else {
                                value.wrapping_add(size as u64)
                            }
                        };
                        assert_eq!(
                            state.gpr[Register::Rsi as usize],
                            next(before_retry.gpr[Register::Rsi as usize])
                        );
                        assert_eq!(
                            state.gpr[Register::Rdi as usize],
                            next(before_retry.gpr[Register::Rdi as usize])
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 384);
}

#[test]
fn rep_movs_overlap_uses_sequential_source_first_transfers() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xa4]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xa5]),
        (OperandSize::Dword, vec![0xf3, 0xa5]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xa5]),
    ];
    let mut cases = 0;
    for (width, base_code) in widths {
        let size = width.bytes() as usize;
        let mut offsets = vec![0isize, size as isize, -(size as isize)];
        if width != OperandSize::Byte {
            offsets.extend([1, -1]);
        }
        for addr32 in [false, true] {
            let code = if addr32 {
                match width {
                    OperandSize::Byte => vec![0xf3, 0x67, 0xa4],
                    OperandSize::Word => vec![0xf3, 0x67, 0x66, 0xa5],
                    OperandSize::Dword => vec![0xf3, 0x67, 0xa5],
                    OperandSize::Qword => vec![0xf3, 0x67, 0x48, 0xa5],
                }
            } else {
                base_code.clone()
            };
            for df in [false, true] {
                for delta in &offsets {
                    let mut input = template.clone();
                    input.memory[0].length = code.len() as u32;
                    input.memory[0].initial_bytes_hex = encode_bytes(&code);
                    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                    input.memory[1].guest_address = HexU64(0x0020_0000);
                    input.memory[1].length = 64;
                    let initial: Vec<u8> = (0..64)
                        .map(|i| (i as u8).wrapping_mul(13).wrapping_add(7))
                        .collect();
                    input.memory[1].initial_bytes_hex = encode_bytes(&initial);
                    input.memory[1].comparison_mask_hex = "ff".repeat(64);
                    input.memory[1].permissions = MemoryPermissions {
                        read: true,
                        write: true,
                        execute: false,
                    };
                    input.memory.truncate(2);
                    let src_offset = 24usize;
                    let dst_offset = (src_offset as isize + *delta) as usize;
                    input.initial_state.gpr[Register::Rcx as usize] =
                        HexU64(if addr32 { 0xfeed_beef_0000_0003 } else { 3 });
                    input.initial_state.gpr[Register::Rsi as usize] = HexU64(
                        (0x0020_0000 + src_offset as u64 + if df { (2 * size) as u64 } else { 0 })
                            | if addr32 { 0xaaaa_bbbb_0000_0000 } else { 0 },
                    );
                    input.initial_state.gpr[Register::Rdi as usize] = HexU64(
                        (0x0020_0000 + dst_offset as u64 + if df { (2 * size) as u64 } else { 0 })
                            | if addr32 { 0xcccc_dddd_0000_0000 } else { 0 },
                    );
                    input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                    let mut expected = initial.clone();
                    for k in 0..3usize {
                        let displacement = if df {
                            -(k as isize) * size as isize
                        } else {
                            k as isize * size as isize
                        };
                        let source = (src_offset as isize
                            + if df { 2 * size as isize } else { 0 }
                            + displacement) as usize;
                        let destination = (dst_offset as isize
                            + if df { 2 * size as isize } else { 0 }
                            + displacement) as usize;
                        let element = expected[source..source + size].to_vec();
                        expected[destination..destination + size].copy_from_slice(&element);
                    }
                    let mut state = input.initial_state.to_cpu().unwrap();
                    let mut memory = MovsbAccessLog {
                        inner: FlatMemory::from_fixture(&input).unwrap(),
                        accesses: Vec::new(),
                        mfences: Vec::new(),
                        mfence_error: None,
                    };
                    let result = run(&mut state, &mut memory, config(1, None), || false);
                    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
                    assert_eq!(result.instructions_executed, 1);
                    assert_eq!(
                        memory.inner.snapshot()[1].bytes_hex,
                        encode_bytes(&expected),
                        "{width:?} DF={df} delta={delta}"
                    );
                    let accesses: Vec<_> = memory
                        .accesses
                        .iter()
                        .copied()
                        .filter(|(k, _, _)| matches!(k, AccessKind::Read | AccessKind::Write))
                        .collect();
                    assert_eq!(accesses.len(), 6);
                    for pair in accesses.chunks_exact(2) {
                        assert_eq!(pair[0].0, AccessKind::Read);
                        assert_eq!(pair[1].0, AccessKind::Write);
                        assert_eq!(pair[0].2, size);
                        assert_eq!(pair[1].2, size);
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 72);
}

#[test]
fn rep_movs_addr32_ecx_limits_completion_and_tracked_resume() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let code = [0xf3, 0x67, 0xa4];
    let limit = REP_ITERATION_WORK_LIMIT as u32;
    for count in [0u32, 1, limit - 1, limit, limit + 1, u32::MAX] {
        let mut input = template.clone();
        input.memory[0].length = code.len() as u32;
        input.memory[0].initial_bytes_hex = encode_bytes(&code);
        input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        input.memory[1].guest_address = HexU64(0x0020_0000);
        input.memory[1].length = (limit as usize + 1) as u32;
        input.memory[1].initial_bytes_hex = "5a".repeat(limit as usize + 1);
        input.memory[1].comparison_mask_hex = "ff".repeat(limit as usize + 1);
        input.memory[1].permissions = MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        };
        input.memory.push(crate::case::MemoryRegionSpec {
            region_id: "destination".into(),
            guest_address: HexU64(0x0030_0000),
            length: limit + 1,
            permissions: MemoryPermissions {
                read: true,
                write: true,
                execute: false,
            },
            initial_bytes_hex: "a5".repeat(limit as usize + 1),
            comparison_mask_hex: "ff".repeat(limit as usize + 1),
        });
        input.initial_state.gpr[Register::Rcx as usize] =
            HexU64(0xfeed_beef_0000_0000 | u64::from(count));
        input.initial_state.gpr[Register::Rsi as usize] = HexU64(0xaaaa_bbbb_0020_0000);
        input.initial_state.gpr[Register::Rdi as usize] = HexU64(0xcccc_dddd_0030_0000);
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let result = run(
            &mut state,
            &mut memory,
            config(1, Some(0x0010_0000 + code.len() as u64)),
            || false,
        );
        let completed = count.min(limit);
        if count <= limit {
            assert_eq!(result.exit, RunExit::StopAddress, "ECX={count}");
            assert_eq!(result.instructions_executed, 1, "ECX={count}");
            assert_eq!(state.rip, 0x0010_0000 + code.len() as u64, "ECX={count}");
        } else {
            assert_eq!(
                result.exit,
                RunExit::RepIterationBudgetExhausted,
                "ECX={count}"
            );
            assert_eq!(result.instructions_executed, 0, "ECX={count}");
            assert_eq!(state.rip, 0x0010_0000, "ECX={count}");
        }
        assert_eq!(
            state.gpr[Register::Rcx as usize],
            if count == 0 {
                0xfeed_beef_0000_0000
            } else {
                u64::from(count - completed)
            }
        );
        assert_eq!(
            state.gpr[Register::Rsi as usize],
            if count == 0 {
                0xaaaa_bbbb_0020_0000
            } else {
                0x0020_0000 + u64::from(completed)
            }
        );
        assert_eq!(
            state.gpr[Register::Rdi as usize],
            if count == 0 {
                0xcccc_dddd_0030_0000
            } else {
                0x0030_0000 + u64::from(completed)
            }
        );
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(
            memory.inner.snapshot()[1].bytes_hex,
            "5a".repeat(limit as usize + 1)
        );
        let expected_destination = format!(
            "{}{}",
            "5a".repeat(completed as usize),
            "a5".repeat(limit as usize + 1 - completed as usize)
        );
        assert_eq!(memory.inner.snapshot()[2].bytes_hex, expected_destination);
        let expected_accesses: Vec<_> = (0..completed)
            .flat_map(|offset| {
                [
                    (AccessKind::Read, 0x0020_0000 + u64::from(offset), 1),
                    (AccessKind::Write, 0x0030_0000 + u64::from(offset), 1),
                ]
            })
            .collect();
        let actual_accesses: Vec<_> = memory
            .accesses
            .iter()
            .copied()
            .filter(|(kind, _, _)| matches!(kind, AccessKind::Read | AccessKind::Write))
            .collect();
        assert_eq!(actual_accesses, expected_accesses);

        if count == limit + 1 {
            let resumed = run_with_rflags_defined(
                &mut state,
                &mut memory,
                config(1, Some(0x0010_0000 + code.len() as u64)),
                result.rflags_defined,
                || false,
            )
            .unwrap();
            assert_eq!(resumed.exit, RunExit::StopAddress);
            assert_eq!(resumed.instructions_executed, 1);
            assert_eq!(state.gpr[Register::Rcx as usize], 0);
            assert_eq!(
                state.gpr[Register::Rsi as usize],
                0x0020_0000 + u64::from(limit + 1)
            );
            assert_eq!(
                state.gpr[Register::Rdi as usize],
                0x0030_0000 + u64::from(limit + 1)
            );
        }
    }
}

#[test]
fn rep_movs_addr32_stop_and_resume_preserves_tracked_state() {
    let code = [0xf3, 0x67, 0xa4];
    let directory = arithmetic_fixture_directory();
    let mut input = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    input.memory[0].length = code.len() as u32;
    input.memory[0].initial_bytes_hex = encode_bytes(&code);
    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    input.memory[1].guest_address = HexU64(0x0020_0000);
    input.memory[1].initial_bytes_hex = "1122".into();
    input.memory[1].comparison_mask_hex = "ffff".into();
    input.memory[1].length = 2;
    input.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    input.memory.push(crate::case::MemoryRegionSpec {
        region_id: "destination".into(),
        guest_address: HexU64(0x0030_0000),
        length: 2,
        permissions: MemoryPermissions {
            read: true,
            write: true,
            execute: false,
        },
        initial_bytes_hex: "a5a5".into(),
        comparison_mask_hex: "ffff".into(),
    });
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let mut state = LongModeState::default();
    state.rip = 0x0010_0000;
    state.gpr[Register::Rcx as usize] = 0xfeed_beef_0000_0002;
    state.gpr[Register::Rsi as usize] = 0xaaaa_bbbb_0020_0000;
    state.gpr[Register::Rdi as usize] = 0xcccc_dddd_0030_0000;
    state.rflags = 0x08d7;
    let mut stop_checks = 0;
    let partial = od_isa::execute::run_with_rflags_defined(
        &mut state,
        &mut memory,
        config(1, None),
        0x02,
        || {
            stop_checks += 1;
            stop_checks >= 2
        },
    )
    .unwrap();
    assert_eq!(partial.exit, RunExit::StopRequested);
    assert_eq!(partial.instructions_executed, 0);
    assert_eq!(partial.rflags_defined, 0x02);
    assert_eq!(state.rip, 0x0010_0000);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x0020_0001);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x0030_0001);
    assert_eq!(memory.snapshot()[2].bytes_hex, "11a5");

    let resumed = run_with_rflags_defined(
        &mut state,
        &mut memory,
        config(1, Some(0x0010_0003)),
        partial.rflags_defined,
        || false,
    )
    .unwrap();
    assert_eq!(resumed.exit, RunExit::StopAddress);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(resumed.rflags_defined, 0x02);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x0020_0002);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x0030_0002);
    assert_eq!(state.rip, 0x0010_0003);
    assert_eq!(memory.snapshot()[2].bytes_hex, "1122");
}

#[test]
fn test_undefined_af_is_preserved_through_addr32_rep_limit_and_resume() {
    let code = [0x48, 0x85, 0xc0, 0xf3, 0x67, 0xa4];
    let directory = arithmetic_fixture_directory();
    let mut input = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    input.memory[0].length = code.len() as u32;
    input.memory[0].initial_bytes_hex = encode_bytes(&code);
    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    input.memory[1].guest_address = HexU64(0x0020_0000);
    input.memory[1].length = 1025;
    input.memory[1].initial_bytes_hex = "5a".repeat(1025);
    input.memory[1].comparison_mask_hex = "ff".repeat(1025);
    input.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    input.memory.push(crate::case::MemoryRegionSpec {
        region_id: "destination".into(),
        guest_address: HexU64(0x0030_0000),
        length: 1025,
        permissions: MemoryPermissions {
            read: true,
            write: true,
            execute: false,
        },
        initial_bytes_hex: "a5".repeat(1025),
        comparison_mask_hex: "ff".repeat(1025),
    });
    let mut state = input.initial_state.to_cpu().unwrap();
    state.gpr[Register::Rax as usize] = 0;
    state.gpr[Register::Rcx as usize] = 0xfeed_beef_0000_0401;
    state.gpr[Register::Rsi as usize] = 0xaaaa_bbbb_0020_0000;
    state.gpr[Register::Rdi as usize] = 0xcccc_dddd_0030_0000;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let partial = run_with_rflags_defined(
        &mut state,
        &mut memory,
        config(2, None),
        od_isa::INTEGER_FLAGS,
        || false,
    )
    .unwrap();
    assert_eq!(partial.exit, RunExit::RepIterationBudgetExhausted);
    assert_eq!(partial.instructions_executed, 1);
    assert_eq!(partial.rflags_defined, 0x0cc7);
    assert_eq!(state.rip, 0x0010_0003);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x0020_0400);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x0030_0400);

    let resumed = run_with_rflags_defined(
        &mut state,
        &mut memory,
        config(1, Some(0x0010_0006)),
        partial.rflags_defined,
        || false,
    )
    .unwrap();
    assert_eq!(resumed.exit, RunExit::StopAddress);
    assert_eq!(resumed.instructions_executed, 1);
    assert_eq!(resumed.rflags_defined, 0x0cc7);
    assert_eq!(state.rip, 0x0010_0006);
    assert_eq!(state.gpr[Register::Rcx as usize], 0);
}

#[test]
fn rep_movs_stop_replay_and_iteration_limit_boundaries() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xa4]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xa5]),
        (OperandSize::Dword, vec![0xf3, 0xa5]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xa5]),
    ];
    let mut stop_cases = 0;
    for (width, code) in &widths {
        let size = width.bytes() as usize;
        for df in [false, true] {
            let mut input = template.clone();
            input.memory[0].length = code.len() as u32;
            input.memory[0].initial_bytes_hex = encode_bytes(code);
            input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            input.memory[1].guest_address = HexU64(0x0020_0000);
            input.memory[1].length = 64;
            input.memory[1].initial_bytes_hex = "3c".repeat(64);
            input.memory[1].comparison_mask_hex = "ff".repeat(64);
            input.memory[1].permissions = MemoryPermissions {
                read: true,
                write: false,
                execute: false,
            };
            input.memory.push(crate::case::MemoryRegionSpec {
                region_id: "destination".into(),
                guest_address: HexU64(0x0030_0000),
                length: 64,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "a5".repeat(64),
                comparison_mask_hex: "ff".repeat(64),
            });
            let src = if df { 0x0020_0030 } else { 0x0020_0000 };
            let dst = if df { 0x0030_0030 } else { 0x0030_0000 };
            input.initial_state.gpr[Register::Rcx as usize] = HexU64(3);
            input.initial_state.gpr[Register::Rsi as usize] = HexU64(src);
            input.initial_state.gpr[Register::Rdi as usize] = HexU64(dst);
            input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
            for after in [1u64, 2] {
                let before = input.initial_state.to_cpu().unwrap();
                let mut state = before.clone();
                let mut memory = MovsbAccessLog {
                    inner: FlatMemory::from_fixture(&input).unwrap(),
                    accesses: Vec::new(),
                    mfences: Vec::new(),
                    mfence_error: None,
                };
                let mut stop_polls = 0;
                let stopped = run(&mut state, &mut memory, config(1, None), || {
                    stop_polls += 1;
                    stop_polls == after as usize + 1
                });
                assert_eq!(stopped.exit, RunExit::StopRequested);
                assert_eq!(stopped.instructions_executed, 0);
                let step = if df {
                    0u64.wrapping_sub(size as u64)
                } else {
                    size as u64
                };
                let mut expected_partial = before.clone();
                expected_partial.gpr[Register::Rcx as usize] = 3 - after;
                expected_partial.gpr[Register::Rsi as usize] =
                    src.wrapping_add(step.wrapping_mul(after));
                expected_partial.gpr[Register::Rdi as usize] =
                    dst.wrapping_add(step.wrapping_mul(after));
                assert_eq!(state, expected_partial);
                let replay = run(&mut state, &mut memory, config(1, None), || false);
                assert_eq!(replay.exit, RunExit::InstructionBudgetExhausted);
                assert_eq!(replay.instructions_executed, 1);
                let mut expected_final = before;
                expected_final.gpr[Register::Rcx as usize] = 0;
                expected_final.gpr[Register::Rsi as usize] = src.wrapping_add(step.wrapping_mul(3));
                expected_final.gpr[Register::Rdi as usize] = dst.wrapping_add(step.wrapping_mul(3));
                expected_final.rip = 0x0010_0000 + code.len() as u64;
                assert_eq!(state, expected_final);
                stop_cases += 1;
            }
        }
    }
    assert_eq!(stop_cases, 16);

    let mut limit_cases = 0;
    for (width, code) in &widths {
        let size = width.bytes() as usize;
        for df in [false, true] {
            for count in [1023u64, 1024, 1025] {
                let mut input = template.clone();
                input.memory[0].length = code.len() as u32;
                input.memory[0].initial_bytes_hex = encode_bytes(code);
                input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                for (index, (base, id, writable, fill)) in [
                    (0x0020_0000, "source", false, 0x3c),
                    (0x0030_0000, "destination", true, 0xa5),
                ]
                .into_iter()
                .enumerate()
                {
                    let region = crate::case::MemoryRegionSpec {
                        region_id: id.into(),
                        guest_address: HexU64(base),
                        length: (size * 1024) as u32,
                        permissions: MemoryPermissions {
                            read: true,
                            write: writable,
                            execute: false,
                        },
                        initial_bytes_hex: format!("{fill:02x}").repeat(size * 1024),
                        comparison_mask_hex: "ff".repeat(size * 1024),
                    };
                    if index == 0 {
                        input.memory[1] = region;
                    } else {
                        input.memory.push(region);
                    }
                }
                let src = if df {
                    0x0020_0000 + (1023 * size) as u64
                } else {
                    0x0020_0000
                };
                let dst = if df {
                    0x0030_0000 + (1023 * size) as u64
                } else {
                    0x0030_0000
                };
                input.initial_state.gpr[Register::Rcx as usize] = HexU64(count);
                input.initial_state.gpr[Register::Rsi as usize] = HexU64(src);
                input.initial_state.gpr[Register::Rdi as usize] = HexU64(dst);
                input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                let mut state = input.initial_state.to_cpu().unwrap();
                let mut memory = MovsbAccessLog {
                    inner: FlatMemory::from_fixture(&input).unwrap(),
                    accesses: Vec::new(),
                    mfences: Vec::new(),
                    mfence_error: None,
                };
                let result = run(&mut state, &mut memory, config(1, None), || false);
                let transferred = count.min(1024);
                assert_eq!(result.instructions_executed, u64::from(count <= 1024));
                assert_eq!(
                    result.exit,
                    if count == 1025 {
                        RunExit::RepIterationBudgetExhausted
                    } else {
                        RunExit::InstructionBudgetExhausted
                    }
                );
                assert_eq!(state.gpr[Register::Rcx as usize], count - transferred);
                let step = if df {
                    0u64.wrapping_sub(size as u64)
                } else {
                    size as u64
                };
                assert_eq!(
                    state.gpr[Register::Rsi as usize],
                    src.wrapping_add(step.wrapping_mul(transferred))
                );
                assert_eq!(
                    state.gpr[Register::Rdi as usize],
                    dst.wrapping_add(step.wrapping_mul(transferred))
                );
                assert_eq!(
                    state.rip,
                    if count <= 1024 {
                        0x0010_0000 + code.len() as u64
                    } else {
                        0x0010_0000
                    }
                );
                limit_cases += 1;
            }
        }
    }
    assert_eq!(limit_cases, 24);

    // STOP is observed before the local work limit after the 1024th success.
    let (_width, code) = &widths[0];
    let mut input = template;
    input.memory[0].length = code.len() as u32;
    input.memory[0].initial_bytes_hex = encode_bytes(code);
    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    for (index, (base, id, writable, fill)) in [
        (0x0020_0000, "source", false, 0x3c),
        (0x0030_0000, "destination", true, 0xa5),
    ]
    .into_iter()
    .enumerate()
    {
        let region = crate::case::MemoryRegionSpec {
            region_id: id.into(),
            guest_address: HexU64(base),
            length: 1024,
            permissions: MemoryPermissions {
                read: true,
                write: writable,
                execute: false,
            },
            initial_bytes_hex: format!("{fill:02x}").repeat(1024),
            comparison_mask_hex: "ff".repeat(1024),
        };
        if index == 0 {
            input.memory[1] = region;
        } else {
            input.memory.push(region);
        }
    }
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(1025);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
    input.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0030_0000);
    let mut state = input.initial_state.to_cpu().unwrap();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let mut polls = 0;
    let stopped = run(&mut state, &mut memory, config(1, None), || {
        polls += 1;
        polls == 1025
    });
    assert_eq!(stopped.exit, RunExit::StopRequested);
    assert_eq!(state.gpr[Register::Rcx as usize], 1);
    assert_eq!(state.gpr[Register::Rsi as usize], 0x0020_0400);
    assert_eq!(state.gpr[Register::Rdi as usize], 0x0030_0400);
}

#[test]
fn rep_movs_zero_count_initial_stop_and_u64_max_are_bounded() {
    let directory = arithmetic_fixture_directory();
    let mut input = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let code = [0xf3, 0xa4];
    input.memory[0].initial_bytes_hex = encode_bytes(&code);
    input.memory[0].length = code.len() as u32;
    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    input.initial_state.gpr[Register::Rcx as usize] = HexU64(0);
    input.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0000_8000_0000_0000);
    input.initial_state.gpr[Register::Rdi as usize] = HexU64(0xffff_8000_0000_0000);
    let mut state = input.initial_state.to_cpu().unwrap();
    let before = state.clone();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(
        &mut state,
        &mut memory,
        config(1, Some(0x0010_0002)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let mut expected = before.clone();
    expected.rip = 0x0010_0002;
    assert_eq!(state, expected);
    assert!(
        !memory
            .accesses
            .iter()
            .any(|(kind, _, _)| matches!(kind, AccessKind::Read | AccessKind::Write))
    );

    let mut stopped_state = before.clone();
    let mut stopped_memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&input).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let stopped = run(
        &mut stopped_state,
        &mut stopped_memory,
        config(1, None),
        || true,
    );
    assert_eq!(stopped.exit, RunExit::StopRequested);
    assert_eq!(stopped.instructions_executed, 0);
    assert_eq!(stopped_state, before);
    assert!(stopped_memory.accesses.is_empty());

    let mut bounded = input.clone();
    bounded.memory[1].guest_address = HexU64(0x0020_0000);
    bounded.memory[1].length = 1024;
    bounded.memory[1].initial_bytes_hex = "3c".repeat(1024);
    bounded.memory[1].comparison_mask_hex = "ff".repeat(1024);
    bounded.memory[1].permissions = MemoryPermissions {
        read: true,
        write: false,
        execute: false,
    };
    bounded.memory.push(crate::case::MemoryRegionSpec {
        region_id: "destination".into(),
        guest_address: HexU64(0x0030_0000),
        length: 1024,
        permissions: MemoryPermissions {
            read: true,
            write: true,
            execute: false,
        },
        initial_bytes_hex: "a5".repeat(1024),
        comparison_mask_hex: "ff".repeat(1024),
    });
    bounded.initial_state.gpr[Register::Rcx as usize] = HexU64(u64::MAX);
    bounded.initial_state.gpr[Register::Rsi as usize] = HexU64(0x0020_0000);
    bounded.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0030_0000);
    let mut bounded_state = bounded.initial_state.to_cpu().unwrap();
    let mut bounded_memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&bounded).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let bounded_result = run(
        &mut bounded_state,
        &mut bounded_memory,
        config(1, None),
        || false,
    );
    assert_eq!(bounded_result.exit, RunExit::RepIterationBudgetExhausted);
    assert_eq!(bounded_result.instructions_executed, 0);
    assert_eq!(bounded_state.gpr[Register::Rcx as usize], u64::MAX - 1024);
    assert_eq!(bounded_state.gpr[Register::Rsi as usize], 0x0020_0400);
    assert_eq!(bounded_state.gpr[Register::Rdi as usize], 0x0030_0400);
    assert_eq!(bounded_state.rip, 0x0010_0000);
}

#[test]
fn rep_movs_page_edge_and_operand_range_failures_are_width_exact() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xa4]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xa5]),
        (OperandSize::Dword, vec![0xf3, 0xa5]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xa5]),
    ];
    let mut cases = 0;
    for (width, code) in widths {
        let size = width.bytes() as usize;
        for failing_site in [None, Some(AccessKind::Read), Some(AccessKind::Write)] {
            let mut input = template.clone();
            input.memory[0].length = code.len() as u32;
            input.memory[0].initial_bytes_hex = encode_bytes(&code);
            input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            input.memory[1] = crate::case::MemoryRegionSpec {
                region_id: "source".into(),
                guest_address: HexU64(0x0020_0000),
                length: 4096,
                permissions: MemoryPermissions {
                    read: true,
                    write: false,
                    execute: false,
                },
                initial_bytes_hex: "5a".repeat(4096),
                comparison_mask_hex: "ff".repeat(4096),
            };
            input.memory.push(crate::case::MemoryRegionSpec {
                region_id: "destination".into(),
                guest_address: HexU64(0x0030_0000),
                length: 4096,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "a5".repeat(4096),
                comparison_mask_hex: "ff".repeat(4096),
            });
            let edge = 4096 - size;
            let src = if failing_site == Some(AccessKind::Read) {
                0x0020_0000 + edge as u64 + 1
            } else {
                0x0020_0000 + edge as u64
            };
            let dst = if failing_site == Some(AccessKind::Write) {
                0x0030_0000 + edge as u64 + 1
            } else {
                0x0030_0000 + edge as u64
            };
            input.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
            input.initial_state.gpr[Register::Rsi as usize] = HexU64(src);
            input.initial_state.gpr[Register::Rdi as usize] = HexU64(dst);
            let before = input.initial_state.to_cpu().unwrap();
            let mut state = before.clone();
            let mut memory = MovsbAccessLog {
                inner: FlatMemory::from_fixture(&input).unwrap(),
                accesses: Vec::new(),
                mfences: Vec::new(),
                mfence_error: None,
            };
            let result = run(&mut state, &mut memory, config(1, None), || false);
            let data_log: Vec<_> = memory
                .accesses
                .iter()
                .copied()
                .filter(|(k, _, _)| matches!(k, AccessKind::Read | AccessKind::Write))
                .collect();
            if let Some(site) = failing_site {
                assert_eq!(
                    result.exit,
                    RunExit::MemoryError(MemoryError::Unmapped {
                        address: if site == AccessKind::Read {
                            0x0020_1000
                        } else {
                            0x0030_1000
                        },
                        access: site
                    })
                );
                assert_eq!(result.instructions_executed, 0);
                assert_eq!(state, before);
                if site == AccessKind::Read {
                    assert_eq!(data_log, vec![(AccessKind::Read, src, size)]);
                } else {
                    assert_eq!(
                        data_log,
                        vec![
                            (AccessKind::Read, src, size),
                            (AccessKind::Write, dst, size)
                        ]
                    );
                }
                assert_eq!(memory.inner.snapshot()[2].bytes_hex, "a5".repeat(4096));
            } else {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
                assert_eq!(result.instructions_executed, 1);
                assert_eq!(
                    data_log,
                    vec![
                        (AccessKind::Read, src, size),
                        (AccessKind::Write, dst, size)
                    ]
                );
                assert_eq!(
                    memory.inner.snapshot()[2].bytes_hex[edge * 2..(edge + size) * 2],
                    "5a".repeat(size)
                );
                assert_eq!(state.gpr[Register::Rsi as usize], src + size as u64);
                assert_eq!(state.gpr[Register::Rdi as usize], dst + size as u64);
            }
            cases += 1;
        }
    }
    assert_eq!(cases, 12);
}

#[test]
fn rep_movs_pointer_wrap_and_truncated_fetch_keep_precise_progress() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let code = [0xf3, 0xa4];
    for wrap_source in [true, false] {
        let mut input = template.clone();
        input.memory[0].initial_bytes_hex = encode_bytes(&code);
        input.memory[0].length = code.len() as u32;
        input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        input.memory[1] = crate::case::MemoryRegionSpec {
            region_id: "source".into(),
            guest_address: HexU64(if wrap_source { 0 } else { 0x001f_ffff }),
            length: if wrap_source { 1 } else { 2 },
            permissions: MemoryPermissions {
                read: true,
                write: false,
                execute: false,
            },
            initial_bytes_hex: if wrap_source { "5a" } else { "3c5a" }.into(),
            comparison_mask_hex: if wrap_source { "ff" } else { "ffff" }.into(),
        };
        input.memory.push(crate::case::MemoryRegionSpec {
            region_id: "destination".into(),
            guest_address: HexU64(if wrap_source { 0x0030_0000 } else { 0 }),
            length: 1,
            permissions: MemoryPermissions {
                read: true,
                write: true,
                execute: false,
            },
            initial_bytes_hex: "a5".into(),
            comparison_mask_hex: "ff".into(),
        });
        input.initial_state.gpr[Register::Rcx as usize] = HexU64(2);
        input.initial_state.gpr[Register::Rsi as usize] =
            HexU64(if wrap_source { 0 } else { 0x0020_0000 });
        input.initial_state.gpr[Register::Rdi as usize] =
            HexU64(if wrap_source { 0x0030_0000 } else { 0 });
        input.initial_state.rflags = HexU64(0x0cd7);
        let before = input.initial_state.to_cpu().unwrap();
        let mut state = before.clone();
        let mut memory = MovsbAccessLog {
            inner: FlatMemory::from_fixture(&input).unwrap(),
            accesses: Vec::new(),
            mfences: Vec::new(),
            mfence_error: None,
        };
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::AddressOverflow {
                address: u64::MAX,
                length: 1
            })
        );
        assert_eq!(result.instructions_executed, 0);
        let mut expected = before;
        expected.gpr[Register::Rcx as usize] = 1;
        if wrap_source {
            expected.gpr[Register::Rsi as usize] = u64::MAX;
            expected.gpr[Register::Rdi as usize] = 0x002f_ffff;
        } else {
            expected.gpr[Register::Rsi as usize] = 0x001f_ffff;
            expected.gpr[Register::Rdi as usize] = u64::MAX;
        }
        assert_eq!(state, expected);
        let data_log: Vec<_> = memory
            .accesses
            .iter()
            .copied()
            .filter(|(k, _, _)| matches!(k, AccessKind::Read | AccessKind::Write))
            .collect();
        if wrap_source {
            assert_eq!(
                data_log,
                vec![
                    (AccessKind::Read, 0, 1),
                    (AccessKind::Write, 0x0030_0000, 1),
                    (AccessKind::Read, u64::MAX, 1)
                ]
            );
        } else {
            assert_eq!(
                data_log,
                vec![
                    (AccessKind::Read, 0x0020_0000, 1),
                    (AccessKind::Write, 0, 1),
                    (AccessKind::Read, 0x001f_ffff, 1),
                    (AccessKind::Write, u64::MAX, 1)
                ]
            );
        }
        assert_eq!(memory.inner.snapshot()[2].bytes_hex, "5a");
    }

    let mut truncated = template;
    truncated.memory[0].length = 1;
    truncated.memory[0].initial_bytes_hex = "f3".into();
    truncated.memory[0].comparison_mask_hex = "ff".into();
    truncated.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
    let before = truncated.initial_state.to_cpu().unwrap();
    let mut state = before.clone();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&truncated).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x0010_0001,
            access: AccessKind::Fetch
        })
    );
    assert_eq!(state, before);
    assert!(
        !memory
            .accesses
            .iter()
            .any(|(kind, _, _)| matches!(kind, AccessKind::Read | AccessKind::Write))
    );
}

#[test]
fn rep_stos_wdq_complete_state_matrix_and_prefix_boundary() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xaa]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xab]),
        (OperandSize::Dword, vec![0xf3, 0xab]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xab]),
    ];
    let values = [0, 1, 0x1122, 0x1122_3344, 0x1122_3344_5566_7788, u64::MAX];
    let flag_bits = [0, 2, 4, 6, 7, 11];
    let mut completed = 0;
    for (width, code) in &widths {
        let size = width.bytes() as usize;
        for df in [false, true] {
            for value in values {
                for count in 0..=3u64 {
                    for pattern in 0..64usize {
                        let mut input = template.clone();
                        let gprs = &mut input.initial_state.gpr;
                        gprs[0] = HexU64(value);
                        gprs[1] = HexU64(count);
                        gprs[7] = HexU64(0x0020_0010);
                        input.initial_state.rflags = HexU64(
                            0x2 | if df { 1 << 10 } else { 0 }
                                | flag_bits
                                    .iter()
                                    .enumerate()
                                    .filter(|(index, _)| pattern & (1 << index) != 0)
                                    .fold(0, |bits, (_, bit)| bits | (1 << bit)),
                        );
                        input.memory[0].length = code.len() as u32;
                        input.memory[0].initial_bytes_hex = encode_bytes(code);
                        input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                        input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                        let data_len = 64usize;
                        input.memory[1].length = data_len as u32;
                        input.memory[1].initial_bytes_hex = "5a".repeat(data_len);
                        input.memory[1].comparison_mask_hex = "ff".repeat(data_len);

                        let before = input.initial_state.to_cpu().unwrap();
                        let mut state = before.clone();
                        let mut memory = FlatMemory::from_fixture(&input).unwrap();
                        let result = run(
                            &mut state,
                            &mut memory,
                            config(1, input.stop_rip.map(HexU64::get)),
                            || false,
                        );
                        assert_eq!(result.exit, RunExit::StopAddress);
                        assert_eq!(result.instructions_executed, 1);
                        let mut expected_data = vec![0x5a; data_len];
                        let value_bytes = value.to_le_bytes();
                        for index in 0..count as usize {
                            let address = if df {
                                16usize - index * size
                            } else {
                                16usize + index * size
                            };
                            expected_data[address..address + size]
                                .copy_from_slice(&value_bytes[..size]);
                        }
                        let expected_rdi = if df {
                            0x0020_0010u64.wrapping_sub(count * size as u64)
                        } else {
                            0x0020_0010u64.wrapping_add(count * size as u64)
                        };
                        let mut expected_state = before.clone();
                        expected_state.gpr[Register::Rcx as usize] = 0;
                        expected_state.gpr[Register::Rdi as usize] = expected_rdi;
                        expected_state.rip = input.stop_rip.unwrap().get();
                        assert_eq!(state, expected_state);
                        assert_eq!(memory.snapshot()[1].bytes_hex, encode_bytes(&expected_data));
                        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
                        completed += 1;
                    }
                }
            }
        }
    }
    assert_eq!(completed, 12_288);

    let mut input = template;
    for (bytes, width) in [
        (vec![0x66, 0xf3, 0xab], OperandSize::Word),
        (vec![0xf3, 0x66, 0xab], OperandSize::Word),
        (vec![0xf3, 0x48, 0x66, 0xab], OperandSize::Word),
        (vec![0xf3, 0x66, 0x48, 0xab], OperandSize::Qword),
    ] {
        input.memory[0].length = bytes.len() as u32;
        input.memory[0].initial_bytes_hex = encode_bytes(&bytes);
        input.memory[0].comparison_mask_hex = "ff".repeat(bytes.len());
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        state.gpr[Register::Rcx as usize] = 1;
        state.gpr[Register::Rdi as usize] = 0x0020_0000;
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(result.instructions_executed, 1, "{bytes:02x?}");
        assert_eq!(
            state.gpr[Register::Rdi as usize],
            0x0020_0000 + width.bytes() as u64
        );
        assert_eq!(state.gpr[Register::Rcx as usize], 0);
    }
}

#[test]
fn rep_stos_wdq_rejects_the_exact_out_of_profile_prefix_set() {
    let directory = arithmetic_fixture_directory();
    let mut fixture = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let forms = [
        vec![0xf3, 0x66, 0xab],
        vec![0xf3, 0xab],
        vec![0xf3, 0x48, 0xab],
    ];
    let prefixes = [0xf0, 0xf2, 0xf3, 0x26, 0x2e, 0x36, 0x3e, 0x64, 0x65];
    let mut rejected = Vec::new();
    for form in forms {
        for prefix in prefixes {
            let mut bytes = vec![prefix];
            bytes.extend_from_slice(&form);
            rejected.push(bytes);
        }
    }
    rejected.extend([
        vec![0xf3, 0x40, 0xab],
        vec![0xf3, 0x47, 0xab],
        vec![0xf3, 0x66, 0x66, 0xab],
        vec![0xf3, 0x66, 0x40, 0xab],
        vec![0xf3, 0x66, 0x47, 0xab],
    ]);
    assert_eq!(rejected.len(), 32);
    for bytes in rejected {
        fixture.memory[0].length = bytes.len() as u32;
        fixture.memory[0].initial_bytes_hex = encode_bytes(&bytes);
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len());
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Unsupported { opcode: 0xab },
            "{bytes:02x?}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes:02x?}");
        assert_eq!(state, before, "{bytes:02x?}");
    }

    for bytes in [
        vec![0xf3, 0x66, 0xaa],
        vec![0xf3, 0x48, 0xaa],
        vec![0xf3, 0x26, 0xaa],
    ] {
        fixture.memory[0].length = bytes.len() as u32;
        fixture.memory[0].initial_bytes_hex = encode_bytes(&bytes);
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len());
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(result.exit, RunExit::Unsupported { opcode: 0xaa });
    }
}

#[test]
fn rep_stos_wdq_write_fault_matrix_preserves_only_completed_iterations() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xaa]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xab]),
        (OperandSize::Dword, vec![0xf3, 0xab]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xab]),
    ];
    let failures = [
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(6),
            linear_address: Some(0),
        }),
        MemoryError::Unmapped {
            address: 0,
            access: AccessKind::Write,
        },
        MemoryError::PermissionDenied {
            address: 0,
            access: AccessKind::Write,
        },
        MemoryError::Backend("injected store failure".into()),
    ];
    let mut cases = 0;
    for (width, code) in widths {
        for df in [false, true] {
            for successes in 0..=2u64 {
                for failure in &failures {
                    let mut input = template.clone();
                    input.memory[0].length = code.len() as u32;
                    input.memory[0].initial_bytes_hex = encode_bytes(&code);
                    input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                    input.memory[1].length = 64;
                    input.memory[1].initial_bytes_hex = "00".repeat(64);
                    input.memory[1].comparison_mask_hex = "ff".repeat(64);
                    let start = if df { 0x0020_0030 } else { 0x0020_0000 };
                    input.initial_state.gpr[Register::Rcx as usize] = HexU64(successes + 1);
                    input.initial_state.gpr[Register::Rdi as usize] = HexU64(start);
                    input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                    let fault_address = if df {
                        start.wrapping_sub(successes * width.bytes() as u64)
                    } else {
                        start + successes * width.bytes() as u64
                    };
                    let before = input.initial_state.to_cpu().unwrap();
                    let mut state = before.clone();
                    let mut memory = MovsbAccessLog {
                        inner: FlatMemory::from_fixture(&input).unwrap(),
                        accesses: Vec::new(),
                        mfences: Vec::new(),
                        mfence_error: None,
                    };
                    let injected = match failure {
                        MemoryError::Architectural(fault) => {
                            MemoryError::Architectural(ArchitecturalFault {
                                linear_address: Some(fault_address),
                                ..fault.clone()
                            })
                        }
                        MemoryError::Unmapped { access, .. } => MemoryError::Unmapped {
                            address: fault_address,
                            access: *access,
                        },
                        MemoryError::PermissionDenied { access, .. } => {
                            MemoryError::PermissionDenied {
                                address: fault_address,
                                access: *access,
                            }
                        }
                        other => other.clone(),
                    };
                    memory
                        .inner
                        .inject_once(fault_address, AccessKind::Write, injected.clone());
                    let result = run(&mut state, &mut memory, config(1, None), || false);
                    match injected {
                        MemoryError::Architectural(fault) => {
                            assert_eq!(result.exit, RunExit::Fault(fault));
                        }
                        other => assert_eq!(result.exit, RunExit::MemoryError(other)),
                    }
                    assert_eq!(result.instructions_executed, 0);
                    let expected_rdi = if df {
                        start.wrapping_sub(successes * width.bytes() as u64)
                    } else {
                        start + successes * width.bytes() as u64
                    };
                    let mut expected_state = before.clone();
                    expected_state.gpr[Register::Rcx as usize] = 1;
                    expected_state.gpr[Register::Rdi as usize] = expected_rdi;
                    assert_eq!(state, expected_state);
                    let mut expected = vec![0u8; 64];
                    let value = before.gpr[Register::Rax as usize].to_le_bytes();
                    for index in 0..successes as usize {
                        let offset = if df {
                            48 - index * width.bytes()
                        } else {
                            index * width.bytes()
                        };
                        expected[offset..offset + width.bytes()]
                            .copy_from_slice(&value[..width.bytes()]);
                    }
                    assert_eq!(
                        memory.inner.snapshot()[1].bytes_hex,
                        encode_bytes(&expected)
                    );
                    let writes: Vec<_> = memory
                        .accesses
                        .iter()
                        .filter(|(kind, _, _)| *kind == AccessKind::Write)
                        .collect();
                    assert_eq!(writes.len(), successes as usize + 1);
                    assert_eq!(writes.last().unwrap().1, fault_address);
                    assert_eq!(writes.last().unwrap().2, width.bytes());
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 96);
}

#[test]
fn rep_stos_wdq_stop_replay_limit_and_max_count_boundaries() {
    let directory = arithmetic_fixture_directory();
    let template = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let widths = [
        (OperandSize::Byte, vec![0xf3, 0xaa]),
        (OperandSize::Word, vec![0xf3, 0x66, 0xab]),
        (OperandSize::Dword, vec![0xf3, 0xab]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xab]),
    ];
    let mut stop_cases = 0;
    for (width, code) in &widths {
        for df in [false, true] {
            for after in [1u64, 2] {
                let mut input = template.clone();
                input.memory[0].length = code.len() as u32;
                input.memory[0].initial_bytes_hex = encode_bytes(code);
                input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                input.memory[1].length = 64;
                input.memory[1].initial_bytes_hex = "00".repeat(64);
                input.memory[1].comparison_mask_hex = "ff".repeat(64);
                let start = if df { 0x0020_0030 } else { 0x0020_0000 };
                input.initial_state.gpr[Register::Rcx as usize] = HexU64(3);
                input.initial_state.gpr[Register::Rdi as usize] = HexU64(start);
                input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                let mut state = input.initial_state.to_cpu().unwrap();
                let mut memory = MovsbAccessLog {
                    inner: FlatMemory::from_fixture(&input).unwrap(),
                    accesses: Vec::new(),
                    mfences: Vec::new(),
                    mfence_error: None,
                };
                let mut polls = 0;
                let stopped = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || {
                        polls += 1;
                        polls == after + 1
                    },
                );
                assert_eq!(stopped.exit, RunExit::StopRequested);
                assert_eq!(stopped.instructions_executed, 0);
                let next = if df {
                    start.wrapping_sub(after * width.bytes() as u64)
                } else {
                    start + after * width.bytes() as u64
                };
                let mut expected_partial = input.initial_state.to_cpu().unwrap();
                expected_partial.gpr[Register::Rcx as usize] = 3 - after;
                expected_partial.gpr[Register::Rdi as usize] = next;
                assert_eq!(state, expected_partial);
                let first_writes: Vec<_> = memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind == AccessKind::Write)
                    .copied()
                    .collect();
                let expected_first: Vec<_> = (0..after)
                    .map(|index| {
                        let address = if df {
                            start.wrapping_sub(index * width.bytes() as u64)
                        } else {
                            start + index * width.bytes() as u64
                        };
                        (AccessKind::Write, address, width.bytes())
                    })
                    .collect();
                assert_eq!(first_writes, expected_first);
                let resumed = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                assert_eq!(resumed.exit, RunExit::StopAddress);
                assert_eq!(resumed.instructions_executed, 1);
                let mut expected_final = input.initial_state.to_cpu().unwrap();
                expected_final.gpr[Register::Rcx as usize] = 0;
                expected_final.gpr[Register::Rdi as usize] = if df {
                    start.wrapping_sub(3 * width.bytes() as u64)
                } else {
                    start + 3 * width.bytes() as u64
                };
                expected_final.rip = input.stop_rip.unwrap().get();
                assert_eq!(state, expected_final);
                let all_writes: Vec<_> = memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _)| *kind == AccessKind::Write)
                    .copied()
                    .collect();
                let expected_all: Vec<_> = (0..3)
                    .map(|index| {
                        let address = if df {
                            start.wrapping_sub(index * width.bytes() as u64)
                        } else {
                            start + index * width.bytes() as u64
                        };
                        (AccessKind::Write, address, width.bytes())
                    })
                    .collect();
                assert_eq!(all_writes, expected_all);
                stop_cases += 1;
            }
        }
    }
    assert_eq!(stop_cases, 16);

    let mut limit_cases = 0;
    let mut stop_at_limit_cases = 0;
    for (width, code) in &widths {
        for df in [false, true] {
            for count in [1023u64, 1024, 1025] {
                let mut input = template.clone();
                input.memory[0].length = code.len() as u32;
                input.memory[0].initial_bytes_hex = encode_bytes(code);
                input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
                input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
                let data_len = 1025 * width.bytes();
                input.memory[1].guest_address = HexU64(if df { 0x0020_0000 } else { 0x0020_0000 });
                input.memory[1].length = data_len as u32;
                input.memory[1].initial_bytes_hex = "00".repeat(data_len);
                input.memory[1].comparison_mask_hex = "ff".repeat(data_len);
                let start = if df {
                    0x0020_0000 + 1024 * width.bytes() as u64
                } else {
                    0x0020_0000
                };
                input.initial_state.gpr[Register::Rcx as usize] = HexU64(count);
                input.initial_state.gpr[Register::Rdi as usize] = HexU64(start);
                input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
                let mut state = input.initial_state.to_cpu().unwrap();
                let mut memory = FlatMemory::from_fixture(&input).unwrap();
                let result = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                let expected_exit = if count <= 1024 {
                    RunExit::StopAddress
                } else {
                    RunExit::RepIterationBudgetExhausted
                };
                assert_eq!(
                    result.exit, expected_exit,
                    "{width:?}, DF={df}, count={count}"
                );
                let done = count.min(1024);
                let expected_rdi = if df {
                    start.wrapping_sub(done * width.bytes() as u64)
                } else {
                    start.wrapping_add(done * width.bytes() as u64)
                };
                let mut expected_state = input.initial_state.to_cpu().unwrap();
                expected_state.gpr[Register::Rcx as usize] = count - done;
                expected_state.gpr[Register::Rdi as usize] = expected_rdi;
                if count <= 1024 {
                    expected_state.rip = input.stop_rip.unwrap().get();
                }
                assert_eq!(state, expected_state);
                assert_eq!(result.instructions_executed, u64::from(count <= 1024));
                limit_cases += 1;
            }

            // STOP is observed at the boundary after store 1024 before the
            // iteration limit is returned.
            let mut input = template.clone();
            input.memory[0].length = code.len() as u32;
            input.memory[0].initial_bytes_hex = encode_bytes(code);
            input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let data_len = 1025 * width.bytes();
            input.memory[1].length = data_len as u32;
            input.memory[1].initial_bytes_hex = "00".repeat(data_len);
            input.memory[1].comparison_mask_hex = "ff".repeat(data_len);
            let start = if df {
                0x0020_0000 + 1024 * width.bytes() as u64
            } else {
                0x0020_0000
            };
            input.initial_state.gpr[Register::Rcx as usize] = HexU64(1025);
            input.initial_state.gpr[Register::Rdi as usize] = HexU64(start);
            input.initial_state.rflags = HexU64(if df { 0x0cd7 } else { 0x08d7 });
            let mut state = input.initial_state.to_cpu().unwrap();
            let mut memory = FlatMemory::from_fixture(&input).unwrap();
            let mut polls = 0;
            let result = run(
                &mut state,
                &mut memory,
                config(1, input.stop_rip.map(HexU64::get)),
                || {
                    polls += 1;
                    polls == 1025
                },
            );
            assert_eq!(result.exit, RunExit::StopRequested);
            let mut expected_stop = input.initial_state.to_cpu().unwrap();
            expected_stop.gpr[Register::Rcx as usize] = 1;
            expected_stop.gpr[Register::Rdi as usize] = if df {
                start.wrapping_sub(1024 * width.bytes() as u64)
            } else {
                start + 1024 * width.bytes() as u64
            };
            assert_eq!(state, expected_stop);
            stop_at_limit_cases += 1;
        }
    }
    assert_eq!(limit_cases, 24);
    assert_eq!(stop_at_limit_cases, 8);

    let mut max = template;
    let code = [0xf3, 0x48, 0xab];
    max.memory[0].length = code.len() as u32;
    max.memory[0].initial_bytes_hex = encode_bytes(&code);
    max.memory[0].comparison_mask_hex = "ff".repeat(code.len());
    max.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
    max.memory[1].length = (1024 * 8) as u32;
    max.memory[1].initial_bytes_hex = "00".repeat(1024 * 8);
    max.memory[1].comparison_mask_hex = "ff".repeat(1024 * 8);
    max.initial_state.gpr[Register::Rcx as usize] = HexU64(u64::MAX);
    max.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0000);
    let mut state = max.initial_state.to_cpu().unwrap();
    let mut expected_max = state.clone();
    let mut memory = FlatMemory::from_fixture(&max).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(1, max.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(result.exit, RunExit::RepIterationBudgetExhausted);
    expected_max.gpr[Register::Rcx as usize] = u64::MAX - 1024;
    expected_max.gpr[Register::Rdi as usize] = 0x0020_2000;
    assert_eq!(state, expected_max);
}

#[test]
fn rep_stos_wdq_literal_fixtures_match_the_complete_local_contract() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_19b_rep_stos_wdq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 28);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        match input.expected_exit {
            ExpectedExit::StopAddress => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ExpectedExit::InstructionBudgetExhausted => {
                assert_eq!(
                    result.exit,
                    RunExit::InstructionBudgetExhausted,
                    "{case_id}"
                )
            }
            ExpectedExit::RepIterationBudgetExhausted => {
                assert_eq!(
                    result.exit,
                    RunExit::RepIterationBudgetExhausted,
                    "{case_id}"
                )
            }
            ExpectedExit::Exception => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    }),
                    "{case_id}"
                )
            }
            ExpectedExit::HarnessError => {
                assert!(
                    matches!(result.exit, RunExit::MemoryError(_)),
                    "{case_id}: {:?}",
                    result.exit
                )
            }
            _ => panic!("unexpected REP fixture outcome in {case_id}"),
        }
        let expected_gpr: [HexU64; 16] = serde_json::from_value(answer["gpr"].clone()).unwrap();
        let expected_flags: HexU64 = serde_json::from_value(answer["rflags"].clone()).unwrap();
        let expected_defined: HexU64 =
            serde_json::from_value(answer["rflags_defined"].clone()).unwrap();
        let expected_rip: HexU64 = serde_json::from_value(answer["rip"].clone()).unwrap();
        assert_eq!(state.gpr, expected_gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rflags, expected_flags.get(), "{case_id}");
        assert_eq!(result.rflags_defined, expected_defined.get(), "{case_id}");
        assert_eq!(state.rip, expected_rip.get(), "{case_id}");
        let expected_memory: std::collections::BTreeMap<String, String> =
            serde_json::from_value(answer["memory"].clone()).unwrap();
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, expected_memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn rep_stos_wdq_zero_budget_fetch_alignment_page_and_wrap_edges() {
    let directory = arithmetic_fixture_directory();
    let mut fixture = Fixture::from_json(
        &std::fs::read(directory.join("s2_19a_rep_stosb_one.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;

    // Zero count does not validate a noncanonical RDI; budget zero does not fetch.
    fixture.memory[0].length = 3;
    fixture.memory[0].initial_bytes_hex = "f348ab".into();
    fixture.memory[0].comparison_mask_hex = "ffffff".into();
    fixture.stop_rip = Some(HexU64(0x0010_0003));
    fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(0);
    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0000800000000000);
    fixture.memory[1].length = 16;
    fixture.memory[1].initial_bytes_hex = "00".repeat(16);
    fixture.memory[1].comparison_mask_hex = "ff".repeat(16);
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let zero = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(zero.exit, RunExit::StopAddress);
    assert_eq!(zero.instructions_executed, 1);
    assert_eq!(memory.accesses.len(), 3);
    assert!(
        memory
            .accesses
            .iter()
            .all(|(kind, _, _)| *kind == AccessKind::Fetch)
    );

    fixture.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
    fixture.initial_state.gpr[Register::Rdi as usize] = HexU64(0x0020_0000);
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let before = state.clone();
    let mut memory = MovsbAccessLog {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        mfences: Vec::new(),
        mfence_error: None,
    };
    let budget = run(
        &mut state,
        &mut memory,
        config(0, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert_eq!(budget.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(state, before);
    assert!(memory.accesses.is_empty());

    // An incomplete prefix fetch remains a backend fetch error with no state commit.
    let mut malformed = fixture.clone();
    malformed.memory[0].length = 1;
    malformed.memory[0].initial_bytes_hex = "f3".into();
    malformed.memory[0].comparison_mask_hex = "ff".into();
    malformed.stop_rip = None;
    malformed.expected_exit = ExpectedExit::HarnessError;
    let mut state = malformed.initial_state.to_cpu().unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&malformed).unwrap();
    let truncated = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        truncated.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(state, before);

    let widths = [
        (OperandSize::Word, vec![0xf3, 0x66, 0xab]),
        (OperandSize::Dword, vec![0xf3, 0xab]),
        (OperandSize::Qword, vec![0xf3, 0x48, 0xab]),
    ];
    for (width, code) in widths {
        for residue in 0..width.bytes() {
            let mut input = fixture.clone();
            input.memory[0].length = code.len() as u32;
            input.memory[0].initial_bytes_hex = encode_bytes(&code);
            input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
            input.memory[1].guest_address = HexU64(0x0020_0ff8);
            input.memory[1].length = 64;
            input.memory[1].initial_bytes_hex = "00".repeat(64);
            input.memory[1].comparison_mask_hex = "ff".repeat(64);
            let address = 0x0020_0ffcu64 + residue as u64;
            input.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
            input.initial_state.gpr[Register::Rdi as usize] = HexU64(address);
            input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
            let mut state = input.initial_state.to_cpu().unwrap();
            let mut memory = FlatMemory::from_fixture(&input).unwrap();
            let result = run(
                &mut state,
                &mut memory,
                config(1, input.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(result.exit, RunExit::StopAddress);
            assert_eq!(
                state.gpr[Register::Rdi as usize],
                address + width.bytes() as u64
            );
            let mut expected = vec![0u8; 64];
            expected[(4 + residue) as usize..(4 + residue) as usize + width.bytes()]
                .copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes()[..width.bytes()]);
            assert_eq!(memory.snapshot()[1].bytes_hex, encode_bytes(&expected));
        }

        let mut input = fixture.clone();
        input.memory[0].length = code.len() as u32;
        input.memory[0].initial_bytes_hex = encode_bytes(&code);
        input.memory[0].comparison_mask_hex = "ff".repeat(code.len());
        input.memory[1].guest_address = HexU64(0);
        input.memory[1].length = width.bytes() as u32;
        input.memory[1].initial_bytes_hex = "00".repeat(width.bytes());
        input.memory[1].comparison_mask_hex = "ff".repeat(width.bytes());
        input.initial_state.gpr[Register::Rcx as usize] = HexU64(1);
        input.initial_state.gpr[Register::Rdi as usize] = HexU64(0);
        input.initial_state.rflags = HexU64(0x0cd7);
        input.stop_rip = Some(HexU64(0x0010_0000 + code.len() as u64));
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(1, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(
            state.gpr[Register::Rdi as usize],
            0u64.wrapping_sub(width.bytes() as u64)
        );
        assert_eq!(
            memory.snapshot()[1].bytes_hex,
            "8877665544332211"[..width.bytes() * 2]
        );
    }
}

#[test]
fn movsd_rejects_non_w_rex_rep_and_segment_prefixes_at_execution_boundary() {
    let directory = arithmetic_fixture_directory();
    let mut fixture =
        Fixture::from_json(&std::fs::read(directory.join("s2_18c_movsd_df0.case.json")).unwrap())
            .unwrap()
            .fixture;
    for (bytes, rip) in [
        ("40a5", 0x0010_0002),
        ("26a5", 0x0010_0002),
        ("2e48a5", 0x0010_0003),
        ("2ea5", 0x0010_0002),
        ("36a5", 0x0010_0002),
        ("3ea5", 0x0010_0002),
        ("64a5", 0x0010_0002),
        ("65a5", 0x0010_0002),
        ("f2a5", 0x0010_0002),
    ] {
        fixture.memory[0].length = (bytes.len() / 2) as u32;
        fixture.memory[0].initial_bytes_hex = bytes.into();
        fixture.memory[0].comparison_mask_hex = "ff".repeat(bytes.len() / 2);
        fixture.stop_rip = Some(HexU64(rip));
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let original_state = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert!(
            matches!(result.exit, RunExit::Unsupported { .. }),
            "{bytes}"
        );
        assert_eq!(result.instructions_executed, 0, "{bytes}");
        assert_eq!(state, original_state, "{bytes}");
    }
}

#[test]
fn movsd_memory_store_writes_exact_source_qword_and_commits_after_success() {
    use crate::v2::Profile;

    let raw = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_9d_movsd_memory_store_rax.case.json"
    );
    let base = Fixture::from_json(raw).unwrap().fixture;
    for source in 0..16u8 {
        let mut fixture = base.clone();
        let mut code = vec![0xf2];
        if source >= 8 {
            code.push(0x44);
        }
        code.extend([0x0f, 0x11, (source & 7) << 3]);
        let bytes = code
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let region = fixture
            .memory
            .iter_mut()
            .find(|r| r.region_id == "code")
            .unwrap();
        region.length = code.len() as u32;
        region.initial_bytes_hex = bytes;
        region.comparison_mask_hex = "ff".repeat(code.len());
        fixture.stop_rip = Some(HexU64(fixture.initial_state.rip.get() + code.len() as u64));
        let mut state = fixture
            .initial_state
            .to_cpu_for(Profile::LongMovsdMemStoreV1)
            .unwrap();
        let before = state.clone();
        let mut memory = AccessTraceMemory {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run(
            &mut state,
            &mut memory,
            config(1, fixture.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
        let mut expected_state = before.clone();
        expected_state.rip = fixture.stop_rip.unwrap().get();
        assert_eq!(state, expected_state, "source {source}");
        let data = memory
            .inner
            .snapshot()
            .into_iter()
            .find(|r| r.region_id == "data")
            .unwrap();
        let mut expected = (0..32)
            .map(|index| {
                u8::from_str_radix(
                    &base.memory[1].initial_bytes_hex[index * 2..index * 2 + 2],
                    16,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let raw_source = before.sse.as_ref().unwrap().xmm[usize::from(source)];
        expected[1..9].copy_from_slice(&raw_source[..8]);
        assert_eq!(
            data.bytes_hex,
            expected
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let data_accesses = memory
            .accesses
            .iter()
            .filter(|(_, address, _, _)| *address >= 0x20_0000)
            .collect::<Vec<_>>();
        assert_eq!(data_accesses.len(), 1, "source {source}");
        assert_eq!(data_accesses[0].0, AccessKind::Write);
        assert_eq!(data_accesses[0].1, 0x20_0001);
        assert_eq!(data_accesses[0].2, 8);
    }

    let fixture = base.clone();
    let mut state = fixture
        .initial_state
        .to_cpu_for(Profile::LongMovsdMemStoreV1)
        .unwrap();
    let before = state.clone();
    let mut memory = AccessTraceMemory {
        inner: FlatMemory::from_fixture(&fixture).unwrap(),
        accesses: Vec::new(),
        fetch_limit: None,
    };
    memory.inner.inject_once(
        0x20_0001,
        AccessKind::Write,
        MemoryError::PermissionDenied {
            address: 0x20_0001,
            access: AccessKind::Write,
        },
    );
    let result = run(
        &mut state,
        &mut memory,
        config(1, fixture.stop_rip.map(HexU64::get)),
        || false,
    );
    assert!(matches!(result.exit, RunExit::MemoryError(_)));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(
        memory
            .accesses
            .iter()
            .filter(|(kind, _, _, _)| *kind == AccessKind::Write)
            .count(),
        1
    );
    assert!(
        !memory
            .accesses
            .iter()
            .any(|(kind, _, _, _)| *kind == AccessKind::Read)
    );

    for (address, canonical) in [
        (0x0000_8000_0000_0000, false),
        (0x0000_7fff_ffff_ffff, false),
        (0x0000_7fff_ffff_fff8, true),
        (0xffff_8000_0000_0000, true),
        (u64::MAX - 3, false),
    ] {
        let fixture = base.clone();
        let mut state = fixture
            .initial_state
            .to_cpu_for(Profile::LongMovsdMemStoreV1)
            .unwrap();
        state.gpr[Register::Rax as usize] = address;
        let before = state.clone();
        let mut memory = AccessTraceMemory {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(state, before);
        if !canonical {
            assert!(
                matches!(result.exit, RunExit::Fault(_) | RunExit::MemoryError(_)),
                "address {address:#x}: {result:?}"
            );
            assert!(
                !memory
                    .accesses
                    .iter()
                    .any(|(kind, _, _, _)| *kind == AccessKind::Write)
            );
        } else {
            assert!(
                matches!(result.exit, RunExit::MemoryError(_)),
                "address {address:#x}: {result:?}"
            );
            assert_eq!(
                memory
                    .accesses
                    .iter()
                    .filter(|(kind, _, _, _)| *kind == AccessKind::Write)
                    .count(),
                1
            );
        }
    }

    for boundary in [
        "missing_sse",
        "fetch_failure",
        "truncated_code",
        "stop",
        "budget",
    ] {
        let fixture = base.clone();
        let mut state = fixture
            .initial_state
            .to_cpu_for(Profile::LongMovsdMemStoreV1)
            .unwrap();
        if boundary == "missing_sse" {
            state.sse = None;
        }
        let before = state.clone();
        let mut memory = AccessTraceMemory {
            inner: FlatMemory::from_fixture(&fixture).unwrap(),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        if boundary == "fetch_failure" {
            memory.inner.inject_once(
                state.rip + 2,
                AccessKind::Fetch,
                MemoryError::Unmapped {
                    address: state.rip + 2,
                    access: AccessKind::Fetch,
                },
            );
        } else if boundary == "truncated_code" {
            memory.fetch_limit = Some(state.rip + 4);
        }
        let (stop_rip, budget) = match boundary {
            "stop" => (Some(state.rip), 1),
            "budget" => (None, 0),
            _ => (None, 1),
        };
        let result = run(&mut state, &mut memory, config(budget, stop_rip), || false);
        assert_eq!(result.instructions_executed, 0, "{boundary}");
        assert_eq!(state, before, "{boundary}");
        assert!(
            !memory
                .accesses
                .iter()
                .any(|(kind, _, _, _)| *kind == AccessKind::Write),
            "{boundary}"
        );
        assert!(
            !memory
                .accesses
                .iter()
                .any(|(kind, _, _, _)| *kind == AccessKind::Read),
            "{boundary}"
        );
        if boundary == "stop" || boundary == "budget" {
            assert!(memory.accesses.is_empty(), "{boundary}");
        }
    }
}

#[test]
fn movsw_accepts_ignored_misplaced_rex_after_normalization() {
    let directory = arithmetic_fixture_directory();
    let mut fixture =
        Fixture::from_json(&std::fs::read(directory.join("s2_18b_movsw_df0.case.json")).unwrap())
            .unwrap()
            .fixture;
    let expected: std::collections::BTreeMap<String, MovsbExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_18b_movsw_expected.json")).unwrap(),
    )
    .unwrap();
    let answer = expected.get("s2_18b_movsw_df0").unwrap();
    fixture.memory[0].length = 3;
    fixture.memory[0].initial_bytes_hex = "4866a5".into();
    fixture.memory[0].comparison_mask_hex = "ffffff".into();
    fixture.stop_rip = Some(HexU64(0x0010_0003));
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr, answer.gpr.map(HexU64::get));
    assert_eq!(state.rflags, answer.rflags.get());
    assert_eq!(state.rip, 0x0010_0003);
    assert_eq!(
        memory
            .snapshot()
            .into_iter()
            .find(|r| r.region_id == "data")
            .unwrap()
            .bytes_hex,
        answer.memory.get("data").unwrap().as_str()
    );
}

#[test]
fn movsb_rejects_prefixes_and_other_string_widths_without_commit() {
    for code in [
        &[0x2e, 0xa4][..],
        &[0x66, 0xa4][..],
        &[0x48, 0xa4][..],
        &[0x66, 0xa4][..],
        &[0x2e, 0x66, 0xa5][..],
        &[0xf3, 0x66, 0xa4][..],
    ] {
        let mut state = LongModeState {
            syscall: None,
            cr2: None,
            tr: None,
            cr3: None,
            cr4: None,
            cr0: None,
            sti_shadow: None,
            rip: 0x100000,
            gpr: [0; 16],
            rflags: 2,
            fs_base: 0,
            gs_base: 0,
            kernel_gs_base: None,
            fs_selector: 0,
            gs_selector: 0,
            ds_selector: 0,
            es_selector: 0,
            cs_selector: None,
            cs_cache: None,
            ss_selector: None,
            ss_cache: None,
            ds_unusable: false,
            ds_cache: None,
            es_unusable: false,
            ss_unusable: false,
            fs_unusable: false,
            gs_unusable: false,
            gdtr: None,
            idtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
            sse: None,
            es_cache: None,

            activity: None,
        };
        let before = state.clone();
        let outcome = step(
            &mut state,
            &mut memory(code, 0x100000),
            AccessContext::default(),
        )
        .unwrap();
        assert!(
            matches!(outcome, StepOutcome::Unsupported { .. }),
            "{code:02x?}"
        );
        assert_eq!(state, before, "{code:02x?}");
    }
}

#[test]
fn push_immediate16_fixtures_match_literal_words_boundaries_and_faults() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17h_push_immediate16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 11);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        match case_id.as_str() {
            "s2_17h_push_fault_bottom" => memory.inject_once(
                0x002f_fffe,
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(0x002f_fffe),
                }),
            ),
            "s2_17h_push_truncated_imm8" => memory.inject_once(
                0x0010_0002,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0002),
                }),
            ),
            "s2_17h_push_truncated_imm16" => memory.inject_once(
                0x0010_0003,
                AccessKind::Fetch,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0010_0003),
                }),
            ),
            _ => {}
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let before = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, before, "{case_id}: no CPU commit");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn push_memory_fixtures_read_sources_before_bounded_stack_writes() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17i_push_memory_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 9);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        if case_id == "s2_17i_push_mem_read_fault" {
            memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            );
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("instruction_budget_exhausted", None) => assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{case_id}"
            ),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, initial, "{case_id}: no CPU commit");
                assert_eq!(state.rip, expected.fault_rip.get(), "{case_id}");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn word_push_memory_fixtures_read_prestate_source_and_write_two_bounded_bytes() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17k_push_memory16_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 8);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        match case_id.as_str() {
            "s2_17k_push_mem_read_fault" => memory.inject_once(
                0x0020_0000,
                AccessKind::Read,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(5),
                    linear_address: Some(0x0020_0000),
                }),
            ),
            "s2_17k_push_mem_stack_fault" => memory.inject_once(
                0x002f_ffff,
                AccessKind::Write,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(6),
                    linear_address: Some(0x002f_ffff),
                }),
            ),
            _ => {}
        }
        let mut state = input.initial_state.to_cpu().unwrap();
        let initial = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(
            result.exit,
            if let Some(expected) = &answer.exception {
                RunExit::Fault(ArchitecturalFault {
                    vector: expected.vector,
                    error_code: Some(expected.error_code),
                    linear_address: expected.linear_address.map(HexU64::get),
                })
            } else {
                RunExit::StopAddress
            },
            "{case_id}"
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(
            result.instructions_executed, answer.instructions_executed,
            "{case_id}"
        );
        match (answer.exit.as_str(), answer.exception.as_ref()) {
            ("stop_address", None) => assert_eq!(result.exit, RunExit::StopAddress, "{case_id}"),
            ("exception", Some(expected)) => {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: expected.vector,
                        error_code: Some(expected.error_code),
                        linear_address: expected.linear_address.map(HexU64::get),
                    }),
                    "{case_id}"
                );
                assert_eq!(state, initial, "{case_id}: no CPU commit");
            }
            other => panic!("invalid literal answer for {case_id}: {}", other.0),
        }
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn word_push_memory_bounded_stack_crossing_preserves_cpu_and_ram() {
    let directory = arithmetic_fixture_directory();
    let input = Fixture::from_json(
        &std::fs::read(directory.join("s2_17k_push_mem_base.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let before_memory = memory.snapshot();
    let mut state = input.initial_state.to_cpu().unwrap();
    state.gpr[Register::Rsp as usize] = 0x0030_0001;
    let before_state = state.clone();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x002f_ffff,
            access: AccessKind::Write
        })
    );
    assert_eq!(state, before_state);
    assert_eq!(memory.snapshot(), before_memory);
}

#[test]
fn address32_word_push_keeps_the_full_64_bit_stack_pointer() {
    let directory = arithmetic_fixture_directory();
    let mut input = Fixture::from_json(
        &std::fs::read(directory.join("s2_17k_push_mem_addr32.case.json")).unwrap(),
    )
    .unwrap()
    .fixture;
    let high_stack = 0x0000_0001_0030_0000;
    let high_rsp = high_stack + 0x78;
    input
        .memory
        .iter_mut()
        .find(|region| region.region_id == "stack")
        .unwrap()
        .guest_address = HexU64(high_stack);
    input.initial_state.gpr[Register::Rsp as usize] = HexU64(high_rsp);

    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let mut state = input.initial_state.to_cpu().unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);

    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[Register::Rsp as usize], high_rsp - 2);
    assert_eq!(state.rip, 0x0010_0004);
    let stack = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "stack")
        .unwrap();
    let bytes = decode_bytes(&stack.bytes_hex).unwrap();
    assert_eq!(&bytes[0x76..0x78], &[0xef, 0xbe]);
    assert!(bytes[..0x76].iter().all(|byte| *byte == 0xa5));
    assert!(bytes[0x78..].iter().all(|byte| *byte == 0xa5));
}

#[test]
fn pop_memory_fixtures_use_postincrement_rsp_and_full_literal_state() {
    let directory = arithmetic_fixture_directory();
    let mut raw_answers: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17j_pop_memory_expected.json")).unwrap(),
    )
    .unwrap();
    raw_answers
        .as_object_mut()
        .unwrap()
        .remove("s2_17j_pop_mem_store_pf");
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> =
        serde_json::from_value(raw_answers).unwrap();
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 1, "{case_id}");
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn word_pop_memory_fixtures_use_two_byte_stack_and_postincrement_destination() {
    let directory = arithmetic_fixture_directory();
    let mut raw_answers: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17l_pop_memory16_expected.json")).unwrap(),
    )
    .unwrap();
    raw_answers
        .as_object_mut()
        .unwrap()
        .remove("s2_17l_pop_mem_store_pf");
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> =
        serde_json::from_value(raw_answers).unwrap();
    assert_eq!(answers.len(), 6);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 1, "{case_id}");
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn memory_stack_rexw_overrides_66_for_qword_operands() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, IndirectJumpExpected> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_17m_memory_stack_rexw_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 4);
    for (case_id, answer) in answers {
        let input = Fixture::from_json(
            &std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap(),
        )
        .unwrap()
        .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(state.rip, answer.rip.get(), "{case_id}");
        assert_eq!(state.rflags, answer.rflags.get(), "{case_id}");
        assert_eq!(
            result.rflags_defined,
            answer.rflags_defined.get(),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 1, "{case_id}");
        assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
        for region in memory.snapshot() {
            assert_eq!(
                region.bytes_hex, answer.memory[&region.region_id],
                "{case_id}"
            );
        }
    }
}

#[test]
fn pop_memory_store_fault_reports_pf_without_claiming_architectural_cpu_poststate() {
    let directory = arithmetic_fixture_directory();
    for case_id in ["s2_17j_pop_mem_store_pf", "s2_17l_pop_mem_store_pf"] {
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before_memory = memory.snapshot();
        memory.inject_once(
            0x0020_0000,
            AccessKind::Write,
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x0020_0000),
            }),
        );
        let mut state = input.initial_state.to_cpu().unwrap();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x0020_0000),
            }),
            "{case_id}"
        );
        assert_eq!(result.instructions_executed, 0, "{case_id}");
        // Intel SDM 093 does not state whether RSP increment is committed
        // when the destination store faults; deliberately make no CPU
        // post-state assertion for either operand width.
        assert_eq!(memory.snapshot(), before_memory, "{case_id}");
    }
}

#[test]
fn pop_memory_bounded_destination_crossing_preserves_ram_and_model_state() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17j_pop_mem_base.case.json")).unwrap();
    let input = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let before_memory = memory.snapshot();
    let mut state = input.initial_state.to_cpu().unwrap();
    state.gpr[Register::Rax as usize] = 0x0020_0004;
    let before_state = state.clone();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x0020_0008,
            access: AccessKind::Write
        })
    ));
    assert_eq!(state, before_state);
    assert_eq!(memory.snapshot(), before_memory);
}

#[test]
fn word_pop_memory_bounded_destination_crossing_preserves_ram_and_model_state() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17l_pop_mem_base.case.json")).unwrap();
    let input = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&input).unwrap();
    let before_memory = memory.snapshot();
    let mut state = input.initial_state.to_cpu().unwrap();
    state.gpr[Register::Rax as usize] = 0x0020_0001;
    let before_state = state.clone();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            address: 0x0020_0002,
            access: AccessKind::Write
        })
    ));
    assert_eq!(state, before_state);
    assert_eq!(memory.snapshot(), before_memory);
}

#[test]
fn push_pop_stack_faults_and_bounds_leave_state_and_memory_uncommitted() {
    let directory = arithmetic_fixture_directory();
    for (case, access, address) in [
        ("s2_17a_push_rax", AccessKind::Write, 0x3000f8),
        ("s2_17a_pop_rax", AccessKind::Read, 0x3000f8),
        ("s2_17b_push_imm8_min", AccessKind::Write, 0x3000f8),
    ] {
        let raw = std::fs::read(directory.join(format!("{case}.case.json"))).unwrap();
        let fixture = Fixture::from_json(&raw).unwrap().fixture;
        for injected in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                linear_address: Some(address),
            }),
            MemoryError::Backend("injected stack backend failure".into()),
        ] {
            let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
            let before_memory = memory.snapshot();
            memory.inject_once(address, access, injected.clone());
            let mut state = fixture.initial_state.to_cpu().unwrap();
            let before_state = state.clone();
            let result = run(
                &mut state,
                &mut memory,
                config(1, fixture.stop_rip.map(HexU64::get)),
                || false,
            );
            let expected = match injected {
                MemoryError::Architectural(fault) => RunExit::Fault(fault),
                error => RunExit::MemoryError(error),
            };
            assert_eq!(result.exit, expected, "{case}");
            assert_eq!(result.instructions_executed, 0, "{case}");
            assert_eq!(state, before_state, "{case}");
            assert_eq!(memory.snapshot(), before_memory, "{case}");
        }
    }

    for (code, rsp, access, address) in [
        (&[0x50][..], 0x300000, AccessKind::Write, 0x2ffff8),
        (&[0x6a, 0x80][..], 0x300000, AccessKind::Write, 0x2ffff8),
        (&[0x58][..], 0x300100, AccessKind::Read, 0x300100),
    ] {
        let raw = std::fs::read(directory.join("s2_17a_push_rax.case.json")).unwrap();
        let mut fixture = Fixture::from_json(&raw).unwrap().fixture;
        let code_region = fixture
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code_region.length = code.len() as u32;
        code_region.initial_bytes_hex = crate::memory::encode_bytes(code);
        code_region.comparison_mask_hex = "ff".repeat(code.len());
        fixture.stop_rip = Some(HexU64(0x100000 + code.len() as u64));
        fixture.initial_state.gpr[4] = HexU64(rsp);
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let before_memory = memory.snapshot();
        let mut state = fixture.initial_state.to_cpu().unwrap();
        let before_state = state.clone();
        let result = run(
            &mut state,
            &mut memory,
            config(1, fixture.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::Unmapped { address, access })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before_state);
        assert_eq!(memory.snapshot(), before_memory);
    }
}

#[test]
fn push_pop_obey_instruction_budget_after_one_committed_instruction() {
    let directory = arithmetic_fixture_directory();
    let raw = std::fs::read(directory.join("s2_17a_push_rax.case.json")).unwrap();
    let fixture = Fixture::from_json(&raw).unwrap().fixture;
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let mut state = fixture.initial_state.to_cpu().unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[4], 0x3000f8);
}

#[test]
fn push_pop_noncanonical_stack_ranges_raise_stack_fault_before_memory_access() {
    for (code, rsp) in [
        (&[0x50][..], 0x0000_8000_0000_0008), // PUSH range begins noncanonical.
        (&[0x58][..], 0x0000_8000_0000_0000), // POP range begins noncanonical.
        (&[0x58][..], 0x0000_7fff_ffff_ffff), // POP range crosses the canonical hole.
    ] {
        let mut memory = memory(code, 0x1000);
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        state.gpr[4] = rsp;
        let before_state = state.clone();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 12,
                error_code: Some(0),
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before_state);
    }
}

#[test]
fn truncated_push_immediates_fault_during_fetch_without_state_commit() {
    for code in [
        &[0x6a][..],
        &[0x68][..],
        &[0x68, 0][..],
        &[0x68, 0, 0][..],
        &[0x68, 0, 0, 0][..],
    ] {
        let mut memory = memory(code, 0x1000);
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        state.gpr[4] = 0x300100;
        let before_state = state.clone();
        let result = run(&mut state, &mut memory, config(1, None), || false);
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::Unmapped {
                address: 0x1000 + code.len() as u64,
                access: AccessKind::Fetch,
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before_state);
    }
}

#[test]
fn s2_39c_literal_fxsave_fixture_writes_the_observed_rip_relative_image() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39c_fxsave_rip_relative_literal.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: fixture.instruction_budget,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, 0x80695);
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.sse, before.sse);

    let area = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "data")
        .unwrap();
    let bytes = decode_bytes(&area.bytes_hex).unwrap();
    assert_eq!(&bytes[0..5], &[0x7f, 0x03, 0x00, 0x18, 0x77]);
    assert_eq!(
        &bytes[6..14],
        &[0xaa, 0x05, 0xef, 0xcd, 0xab, 0x89, 0x2b, 0]
    );
    assert_eq!(&bytes[16..22], &[0x10, 0x32, 0x54, 0x76, 0x53, 0]);
    assert_eq!(&bytes[24..32], &[0x80, 0x1f, 0, 0, 0xbf, 0xff, 0, 0]);
    let x87 = before.x87.as_ref().unwrap();
    for logical in 0..8 {
        let physical = (usize::from(x87.top()) + logical) & 7;
        assert_eq!(
            &bytes[32 + 16 * logical..42 + 16 * logical],
            &x87.registers[physical]
        );
    }
    let sse = before.sse.as_ref().unwrap();
    for register in 0..16 {
        assert_eq!(
            &bytes[160 + 16 * register..176 + 16 * register],
            &sse.xmm[register]
        );
    }
    for range in [5..6, 14..16, 22..24, 32 + 10..32 + 16, 416..512] {
        assert_eq!(&bytes[range.clone()], &vec![0xa5; range.len()]);
    }
}

#[test]
fn s2_39d_literal_fxsave64_fixture_writes_full_pointer_image_without_selectors() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39d_fxsave64_rax_disp8_literal.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: fixture.instruction_budget,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, 0x80693);
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.sse, before.sse);
    assert_eq!(state.x87.as_ref().unwrap().instruction_selector, None);
    assert_eq!(state.x87.as_ref().unwrap().data_selector, None);

    let area = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "data")
        .unwrap();
    let bytes = decode_bytes(&area.bytes_hex).unwrap();
    assert_eq!(
        &bytes[8..24],
        &[
            0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba,
            0xdc, 0xfe,
        ]
    );
    assert_eq!(&bytes[24..32], &[0x80, 0x1f, 0, 0, 0xbf, 0xff, 0, 0]);
    let x87 = before.x87.as_ref().unwrap();
    for logical in 0..8 {
        let physical = (usize::from(x87.top()) + logical) & 7;
        assert_eq!(
            &bytes[32 + 16 * logical..42 + 16 * logical],
            &x87.registers[physical]
        );
    }
    for range in [5..6, 32 + 10..32 + 16, 416..512] {
        assert_eq!(&bytes[range.clone()], &vec![0xa5; range.len()]);
    }
}

#[test]
fn s2_39f_literal_fxsave64_cr0_gate_fixture_is_strict_and_preserves_the_image() {
    let bytes =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_39f_fxsave64_cr0_em_nm.case.json");
    let loaded = crate::v2::Fixture::from_json(bytes).unwrap();
    assert_eq!(
        loaded.fixture.profile,
        crate::v2::Profile::LongFxsave64Cr0GatesV1
    );
    assert_eq!(
        loaded.fixture.fxsave64_cr0_controls,
        Some(crate::v2::Fxsave64Cr0Snapshot {
            cr0_em: true,
            cr0_ts: false,
        })
    );
    assert_eq!(
        loaded.fixture.expected_exit,
        crate::v2::ExpectedExit::Exception
    );
    let mut state = loaded
        .fixture
        .initial_state
        .to_cpu_for(loaded.fixture.profile)
        .unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&loaded.fixture).unwrap();
    let controls = loaded.fixture.fxsave64_cr0_controls.unwrap();
    let result = run_with_fxsave64_cr0_controls(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: loaded.fixture.instruction_budget,
            stop_rip: loaded.fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        Fxsave64Cr0Controls {
            cr0_em: controls.cr0_em,
            cr0_ts: controls.cr0_ts,
        },
        || false,
    );
    assert_eq!(
        result.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 7,
            error_code: None,
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    let memory = memory.snapshot();
    assert_eq!(
        memory[1].bytes_hex,
        loaded.fixture.memory[1].initial_bytes_hex
    );

    let mut extra = serde_json::from_slice::<serde_json::Value>(bytes).unwrap();
    extra["fxsave64_cr0_controls"]["cr4_osfxsr"] = serde_json::json!(false);
    assert!(crate::v2::Fixture::from_json(&serde_json::to_vec(&extra).unwrap()).is_err());

    let mut missing = serde_json::from_slice::<serde_json::Value>(bytes).unwrap();
    missing
        .as_object_mut()
        .unwrap()
        .remove("fxsave64_cr0_controls");
    assert!(crate::v2::Fixture::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());

    let mut wrong_encoding = serde_json::from_slice::<serde_json::Value>(bytes).unwrap();
    wrong_encoding["memory"][0]["initial_bytes_hex"] = serde_json::json!("480fae4b40");
    assert!(crate::v2::Fixture::from_json(&serde_json::to_vec(&wrong_encoding).unwrap()).is_err());
}

#[test]
fn s2_39g_literal_fxrstor64_cr0_fixtures_are_strict_and_restore_staged_image() {
    let nm_bytes =
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_39g_fxrstor64_cr0_em_nm.case.json");
    let nm = crate::v2::Fixture::from_json(nm_bytes).unwrap().fixture;
    assert_eq!(nm.profile, crate::v2::Profile::LongFxrstor64Cr0GatesV1);
    assert_eq!(
        nm.fxrstor64_cr0_controls,
        Some(crate::v2::Fxrstor64Cr0Snapshot {
            cr0_em: true,
            cr0_ts: false,
        })
    );
    let mut state = nm.initial_state.to_cpu_for(nm.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&nm).unwrap();
    let result = run_with_fxrstor64_cr0_controls(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: nm.instruction_budget,
            stop_rip: nm.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        Fxrstor64Cr0Controls {
            cr0_em: true,
            cr0_ts: false,
        },
        || false,
    );
    assert_eq!(
        result.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 7,
            error_code: None,
            linear_address: None,
        })
    );
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    assert_eq!(
        memory.snapshot()[1].bytes_hex,
        nm.memory[1].initial_bytes_hex
    );

    let mut malformed = serde_json::from_slice::<serde_json::Value>(nm_bytes).unwrap();
    malformed["fxrstor64_cr0_controls"]["cr4_osfxsr"] = serde_json::json!(false);
    assert!(crate::v2::Fixture::from_json(&serde_json::to_vec(&malformed).unwrap()).is_err());
    let mut missing = serde_json::from_slice::<serde_json::Value>(nm_bytes).unwrap();
    missing
        .as_object_mut()
        .unwrap()
        .remove("fxrstor64_cr0_controls");
    assert!(crate::v2::Fixture::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut wrong_encoding = serde_json::from_slice::<serde_json::Value>(nm_bytes).unwrap();
    wrong_encoding["memory"][0]["initial_bytes_hex"] = serde_json::json!("480fae4b41");
    assert!(crate::v2::Fixture::from_json(&serde_json::to_vec(&wrong_encoding).unwrap()).is_err());
    let mut invalid_mxcsr = serde_json::from_slice::<serde_json::Value>(nm_bytes).unwrap();
    let image = invalid_mxcsr["memory"][1]["initial_bytes_hex"]
        .as_str()
        .unwrap();
    let mut image_bytes = decode_bytes(image).unwrap();
    image_bytes[24..28].copy_from_slice(&0x0000_1fc0u32.to_le_bytes());
    invalid_mxcsr["memory"][1]["initial_bytes_hex"] = serde_json::json!(encode_bytes(&image_bytes));
    assert!(crate::v2::Fixture::from_json(&serde_json::to_vec(&invalid_mxcsr).unwrap()).is_err());

    let success_bytes = include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39g_fxrstor64_cr0_clear_success.case.json"
    );
    let success = crate::v2::Fixture::from_json(success_bytes)
        .unwrap()
        .fixture;
    assert_eq!(success.profile, crate::v2::Profile::LongFxrstor64Cr0GatesV1);
    let mut state = success.initial_state.to_cpu_for(success.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&success).unwrap();
    let result = run_with_fxrstor64_cr0_controls(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: success.instruction_budget,
            stop_rip: success.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        Fxrstor64Cr0Controls {
            cr0_em: false,
            cr0_ts: false,
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, success.stop_rip.unwrap().get());
    assert_ne!(state.x87, before.x87);
    assert_ne!(state.sse, before.sse);
    assert_eq!(state.x87.as_ref().unwrap().status_word & 1, 1);
    assert_eq!(state.sse.as_ref().unwrap().mxcsr & 0x1f, 5);
    assert_eq!(
        memory.snapshot()[1].bytes_hex,
        success.memory[1].initial_bytes_hex
    );
}

#[test]
fn s2_39f_clear_cr0_fixture_matches_an_independent_literal_fxsave64_image() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39f_fxsave64_cr0_clear_success.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let controls = fixture.fxsave64_cr0_controls.unwrap();
    let result = run_with_fxsave64_cr0_controls(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: fixture.instruction_budget,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        Fxsave64Cr0Controls {
            cr0_em: controls.cr0_em,
            cr0_ts: controls.cr0_ts,
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
    let data = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "data")
        .unwrap();
    let expected = decode_bytes(
        include_str!("../../../tests/fixtures/x86/stage2/s2_39f_fxsave64_expected_image.hex")
            .trim(),
    )
    .unwrap();
    assert_eq!(decode_bytes(&data.bytes_hex).unwrap(), expected);
}

#[test]
fn s2_39d_wrapping_rax_effective_address_targets_canonical_image_at_zero() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_39d_fxsave64_rax_disp8_literal.case.json"
    ))
    .unwrap()
    .fixture;
    fixture.initial_state.gpr[0] = HexU64(u64::MAX - 0x3f);
    fixture.memory[1].guest_address = HexU64(0);
    fixture.validate().unwrap();

    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let data = memory
        .snapshot()
        .into_iter()
        .find(|region| region.region_id == "data")
        .unwrap();
    let bytes = decode_bytes(&data.bytes_hex).unwrap();
    assert_eq!(
        &bytes[8..24],
        &[
            0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba,
            0xdc, 0xfe,
        ]
    );
    assert_eq!(&bytes[464..], &[0xa5; 48]);
}

fn assert_literal_fixture_answers(sidecar: &str, count: usize) {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, ArithmeticExpected> =
        serde_json::from_slice(&std::fs::read(directory.join(sidecar)).unwrap()).unwrap();
    assert_eq!(
        answers.len(),
        count,
        "all selected forms must be materialized"
    );
    for (case_id, answer) in answers {
        assert!(
            case_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let raw = std::fs::read(directory.join(format!("{case_id}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        assert_eq!(input.case_id, case_id);
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let mut state = input.initial_state.to_cpu().unwrap();
        let actual = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        assert_eq!(actual.exit, RunExit::StopAddress, "{case_id}");
        assert_eq!(
            actual.instructions_executed, input.instruction_budget,
            "{case_id}"
        );
        assert_eq!(
            actual.rflags_defined,
            answer
                .rflags_defined
                .map(HexU64::get)
                .unwrap_or(od_isa::INTEGER_FLAGS),
            "{case_id}"
        );
        assert_eq!(state.gpr, answer.gpr.map(HexU64::get), "{case_id}");
        assert_eq!(
            state.rflags & input.rflags_mask.get(),
            answer.rflags.get() & input.rflags_mask.get(),
            "{case_id}"
        );
        assert_eq!(state.rip, input.stop_rip.unwrap().get(), "{case_id}");
        for region in memory.snapshot() {
            let before = input
                .memory
                .iter()
                .find(|r| r.region_id == region.region_id)
                .unwrap();
            let expected = answer
                .memory
                .get(&region.region_id)
                .unwrap_or(&before.initial_bytes_hex);
            assert_eq!(
                &region.bytes_hex, expected,
                "{case_id}: {}",
                region.region_id
            );
        }
    }
}

#[test]
fn arithmetic_cross_region_read_and_write_faults_commit_neither_flags_nor_memory() {
    let directory = arithmetic_fixture_directory();
    for op in ["add", "adc", "sub", "sbb", "inc", "dec", "neg"] {
        let suffix = if matches!(op, "inc" | "dec" | "neg") {
            "mem"
        } else {
            "mem_reg"
        };
        let raw =
            std::fs::read(directory.join(format!("s2_11_{op}_64_{suffix}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        for access in [AccessKind::Read, AccessKind::Write] {
            let fault = ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                linear_address: Some(0x201000),
            };
            for error in [
                MemoryError::Architectural(fault),
                MemoryError::Backend("injected arithmetic backend fault".into()),
            ] {
                let mut memory = FlatMemory::from_fixture(&input).unwrap();
                let before_memory = memory.snapshot();
                memory.inject_once(0x201000, access, error.clone());
                let mut state = input.initial_state.to_cpu().unwrap();
                let before_state = state.clone();
                let actual = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                let expected = match error {
                    MemoryError::Architectural(fault) => RunExit::Fault(fault),
                    error => RunExit::MemoryError(error),
                };
                assert_eq!(actual.exit, expected, "{op}: {access:?}");
                assert_eq!(actual.instructions_executed, 0);
                assert_eq!(state, before_state, "{op}: {access:?}");
                assert_eq!(memory.snapshot(), before_memory, "{op}: {access:?}");
            }
        }
    }
}

#[test]
fn logic_faults_preserve_definedness_and_all_state_until_retirement() {
    let directory = arithmetic_fixture_directory();
    for op in ["and", "or", "xor", "test", "not"] {
        let suffix = if op == "not" { "mem" } else { "mem_reg" };
        let raw =
            std::fs::read(directory.join(format!("s2_12_{op}_64_{suffix}.case.json"))).unwrap();
        let input = Fixture::from_json(&raw).unwrap().fixture;
        for access in [AccessKind::Read, AccessKind::Write] {
            if op == "test" && access == AccessKind::Write {
                continue; // TEST has no write; readonly fixtures check that separately.
            }
            let fault = ArchitecturalFault {
                vector: 14,
                error_code: Some(if access == AccessKind::Write { 6 } else { 4 }),
                linear_address: Some(0x201000),
            };
            for error in [
                MemoryError::Architectural(fault),
                MemoryError::Backend("injected logic backend fault".into()),
            ] {
                let mut memory = FlatMemory::from_fixture(&input).unwrap();
                let before_memory = memory.snapshot();
                memory.inject_once(0x201000, access, error.clone());
                let mut state = input.initial_state.to_cpu().unwrap();
                let before_state = state.clone();
                let actual = run(
                    &mut state,
                    &mut memory,
                    config(1, input.stop_rip.map(HexU64::get)),
                    || false,
                );
                let expected = match error {
                    MemoryError::Architectural(fault) => RunExit::Fault(fault),
                    error => RunExit::MemoryError(error),
                };
                assert_eq!(actual.exit, expected, "{op}: {access:?}");
                assert_eq!(actual.instructions_executed, 0);
                assert_eq!(
                    actual.rflags_defined,
                    od_isa::INTEGER_FLAGS,
                    "{op}: {access:?}"
                );
                assert_eq!(state, before_state, "{op}: {access:?}");
                assert_eq!(memory.snapshot(), before_memory, "{op}: {access:?}");
            }
        }
    }
}

#[test]
fn memory_mov_faults_preserve_precise_state_and_never_partially_store() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
    ))
    .unwrap()
    .fixture;
    for kind in [AccessKind::Read, AccessKind::Write] {
        let fault = ArchitecturalFault {
            vector: 14,
            error_code: Some(if kind == AccessKind::Write { 6 } else { 4 }),
            linear_address: Some(0x1000),
        };
        for error in [
            MemoryError::Architectural(fault.clone()),
            MemoryError::Backend("injected S2-08 backend failure".into()),
        ] {
            let mut mem = FlatMemory::from_fixture(&input).unwrap();
            let ram = mem.snapshot();
            mem.inject_once(0x1000, kind, error.clone());
            let mut state = input.initial_state.to_cpu().unwrap();
            let mut expected = state.clone();
            if kind == AccessKind::Write {
                expected.gpr[0] = 12;
                expected.rflags = 0x406;
                expected.rip = 0x4007;
            }
            let actual = run(
                &mut state,
                &mut mem,
                config(32, input.stop_rip.map(HexU64::get)),
                || false,
            );
            assert_eq!(
                actual.exit,
                if let MemoryError::Architectural(fault) = error {
                    RunExit::Fault(fault)
                } else {
                    RunExit::MemoryError(error)
                }
            );
            assert_eq!(
                actual.instructions_executed,
                if kind == AccessKind::Write { 2 } else { 0 }
            );
            assert_eq!(state, expected);
            assert_eq!(mem.snapshot(), ram);
        }
    }
}

#[test]
fn memory_load_aliases_have_independent_expected_intermediate_values() {
    let input = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/mov_width_aliasing.case.json"
    ))
    .unwrap()
    .fixture;
    let mut mem = FlatMemory::from_fixture(&input).unwrap();
    let mut state = input.initial_state.to_cpu().unwrap();
    for (rax, rip) in [
        (0x1122_3344_5566_7711, 0x4002),
        (0x1122_3344_5566_7711, 0x4005),
        (0x1122_3344_5566_2211, 0x4008),
        (0x1122_3344_5566_2211, 0x400b),
        (0x1122_3344_5566_4433, 0x400f),
        (0x1122_3344_5566_4433, 0x4013),
        (0x0000_0000_8877_6655, 0x4016),
        (0x0000_0000_8877_6655, 0x4019),
    ] {
        assert_eq!(
            step(&mut state, &mut mem, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[0], rax);
        assert_eq!(state.rip, rip);
        assert_eq!(state.rflags, 0x402);
        assert_eq!(
            &state.gpr[1..],
            &input.initial_state.to_cpu().unwrap().gpr[1..]
        );
    }
}

#[test]
fn on_demand_execution_skips_invalid_tail_and_accepts_arbitrary_reached_entry() {
    // JZ skips an unsupported byte. The MOV is the last mapped instruction.
    let code = [0x74, 0x01, 0x90, 0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8];
    let base = 0x1000;
    let mut mem = memory(&code, base);
    let mut state = LongModeState {
        cr3: None,
        cr4: None,
        sti_shadow: None,
        rip: base,
        rflags: 0x42,
        ..LongModeState::default()
    };
    let result = run(
        &mut state,
        &mut mem,
        config(10, Some(base + code.len() as u64)),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(state.gpr[0], 0x0807_0605_0403_0201);
    assert_eq!(state.rflags, 0x42);
    state.rip = base + 3;
    let result = run(&mut state, &mut mem, config(1, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 1);
    // Mapping end is not an implicit CPU stop address.
    let result = run(&mut state, &mut mem, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(result.instructions_executed, 0);
}

#[test]
fn s2_22a_fs_base_fixture_executes_alias_load_and_observes_profile_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22a_fs_base_qword_load_alias.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        config(
            fixture.instruction_budget,
            fixture.stop_rip.map(HexU64::get),
        ),
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.gpr[0], 0x8877_6655_4433_2211);
    assert_eq!(state.gpr[1..], before.gpr[1..]);
    assert_eq!(state.rip, 0x0010_0004);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(state.fs_base, 0xffff_8000_0000_0000);
    assert_eq!(memory.snapshot()[1].guest_address.get(), 0x0040_0000);
}

#[test]
fn s2_22b_gs_base_fixture_executes_and_faults_before_commit() {
    let mut fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_22b_gs_base_qword_load_alias.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    assert_eq!(
        run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get)
            ),
            || false,
        )
        .exit,
        RunExit::StopAddress
    );
    assert_eq!(state.gpr[0], 0x8877_6655_4433_2211);
    assert_eq!(state.gs_base, 0xffff_8000_0000_0000);
    assert_eq!(memory.snapshot()[1].guest_address.get(), 0x0040_0000);

    fixture.initial_state.gpr[0] = HexU64(0x0000_7fff_ffff_fff0);
    fixture.initial_state.gs_base = Some(HexU64(0x1000));
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(matches!(
        result.exit,
        RunExit::Fault(ArchitecturalFault { vector: 13, .. })
    ));
    assert_eq!(state, before);

    fixture.initial_state.gpr[0] = HexU64(0x0000_0000_0050_0000);
    fixture.initial_state.gs_base = Some(HexU64(0x1000));
    fixture.memory[1].guest_address = HexU64(0x0000_0000_0060_0000);
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    memory.inject_once(
        0x0000_0000_0050_1000,
        AccessKind::Read,
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x0000_0000_0050_1000),
        }),
    );
    let result = run(&mut state, &mut memory, config(1, None), || false);
    assert!(
        matches!(
            result.exit,
            RunExit::Fault(ArchitecturalFault { vector: 14, .. })
        ),
        "unexpected GS-base unmapped read exit: {:?}",
        result.exit
    );
    assert_eq!(state, before);
}

#[test]
fn s2_27g_swapgs_literals_exchange_state_and_preserve_second_fault_commit() {
    use od_isa::SwapgsProfile;
    use od_isa::execute::run_with_swapgs_profile;
    use od_isa::memory::PrivilegeLevel;
    for (case_id, retired, final_gs, final_kernel, rip_offset, exit) in [
        (
            "s2_27g_swapgs_once",
            1,
            0x0080_0000,
            0x0040_0000,
            3,
            RunExit::StopAddress,
        ),
        (
            "s2_27g_swapgs_double",
            2,
            0x0040_0000,
            0x0080_0000,
            6,
            RunExit::StopAddress,
        ),
        (
            "s2_27g_swapgs_gs_load_fault",
            2,
            0x0080_0000,
            0x0040_0000,
            7,
            RunExit::StopAddress,
        ),
    ] {
        let path = arithmetic_fixture_directory().join(format!("{case_id}.case.json"));
        let fixture = Fixture::from_json(&std::fs::read(path).unwrap())
            .unwrap()
            .fixture;
        assert_eq!(fixture.profile, crate::v2::Profile::LongSwapgsV1);
        let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let result = run_with_swapgs_profile(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: fixture.instruction_budget,
                stop_rip: fixture.stop_rip.map(HexU64::get),
                context: AccessContext {
                    privilege: PrivilegeLevel::Ring0,
                    ..AccessContext::default()
                },
            },
            &SwapgsProfile::SwapgsV1,
            || false,
        );
        assert_eq!(result.exit, exit, "{case_id}");
        assert_eq!(result.instructions_executed, retired, "{case_id}");
        assert_eq!(state.gs_base, final_gs, "{case_id}");
        assert_eq!(state.kernel_gs_base, Some(final_kernel), "{case_id}");
        assert_eq!(state.rip, 0x0010_0000 + rip_offset, "{case_id}");
        if case_id.ends_with("fault") {
            assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_7788);
        }
    }

    let path = arithmetic_fixture_directory().join("s2_27g_swapgs_gs_load_fault.case.json");
    let fixture = Fixture::from_json(&std::fs::read(path).unwrap())
        .unwrap()
        .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    memory.inject_once(
        0x0080_0000,
        AccessKind::Read,
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x0080_0000),
        }),
    );
    let result = run_with_swapgs_profile(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 2,
            stop_rip: None,
            context: AccessContext {
                privilege: PrivilegeLevel::Ring0,
                ..AccessContext::default()
            },
        },
        &SwapgsProfile::SwapgsV1,
        || false,
    );
    assert_eq!(
        result.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x0080_0000)
        })
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, 0x0010_0003);
    assert_eq!(
        (state.gs_base, state.kernel_gs_base),
        (0x0080_0000, Some(0x0040_0000))
    );
}

#[test]
fn s2_27h_cli_fixture_matrix_clears_if_only_when_cpl_allows() {
    use od_isa::execute::{RunConfig, RunExit, run_with_cli_profile};
    use od_isa::memory::PrivilegeLevel;
    use od_isa::{CliProfile, INTEGER_FLAGS};

    let mut successes = 0;
    let mut faults = 0;
    for if_set in 0..=1 {
        for cpl in 0..=3 {
            for iopl in 0..=3 {
                let case_id = format!("s2_27h_cli_cpl{cpl}_iopl{iopl}_if{if_set}");
                let path = arithmetic_fixture_directory().join(format!("{case_id}.case.json"));
                let fixture = Fixture::from_json(&std::fs::read(path).unwrap())
                    .unwrap()
                    .fixture;
                assert_eq!(fixture.profile, crate::v2::Profile::LongCliV1);
                let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
                let before = state.clone();
                let mut memory = AccessTraceMemory {
                    inner: FlatMemory::from_fixture(&fixture).unwrap(),
                    accesses: Vec::new(),
                    fetch_limit: None,
                };
                let privilege = match cpl {
                    0 => PrivilegeLevel::Ring0,
                    1 => PrivilegeLevel::Ring1,
                    2 => PrivilegeLevel::Ring2,
                    _ => PrivilegeLevel::Ring3,
                };
                let result = run_with_cli_profile(
                    &mut state,
                    &mut memory,
                    RunConfig {
                        instruction_budget: fixture.instruction_budget,
                        stop_rip: fixture.stop_rip.map(HexU64::get),
                        context: AccessContext {
                            privilege,
                            ..AccessContext::default()
                        },
                    },
                    &CliProfile::NonPviLong64V1,
                    || false,
                );
                if cpl <= iopl {
                    successes += 1;
                    assert_eq!(result.exit, RunExit::StopAddress, "{case_id}");
                    assert_eq!(result.instructions_executed, 1, "{case_id}");
                    assert_eq!(state.rip, before.rip + 1, "{case_id}");
                    assert_eq!(state.rflags, before.rflags & !(1 << 9), "{case_id}");
                    let mut expected = before.clone();
                    expected.rip += 1;
                    expected.rflags &= !(1 << 9);
                    assert_eq!(state, expected, "{case_id}");
                } else {
                    faults += 1;
                    assert_eq!(
                        result.exit,
                        RunExit::Fault(ArchitecturalFault {
                            vector: 13,
                            error_code: Some(0),
                            linear_address: None
                        }),
                        "{case_id}"
                    );
                    assert_eq!(result.instructions_executed, 0, "{case_id}");
                    assert_eq!(state, before, "{case_id}");
                }
                assert_eq!(result.rflags_defined, INTEGER_FLAGS, "{case_id}");
                assert_eq!(memory.accesses.len(), 1, "{case_id}");
                assert_eq!(memory.accesses[0].0, AccessKind::Fetch, "{case_id}");
                assert_eq!(memory.accesses[0].1, before.rip, "{case_id}");
            }
        }
    }
    assert_eq!((successes, faults), (20, 12));
}

#[test]
fn s2_27h_cli_encoding_boundary_and_interrupt_eligibility_composition() {
    use od_isa::execute::{
        MaskableRequestRunResult, RunConfig, RunExit, RunResult, run, run_with_cli_profile,
        run_with_maskable_request,
    };
    use od_isa::memory::{AccessContext, PrivilegeLevel};
    use od_isa::{CliProfile, INTEGER_FLAGS};

    let profile = CliProfile::NonPviLong64V1;
    let context = AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    };
    for (bytes, expected) in [
        (
            &[0xf3, 0xfa][..],
            Some(RunExit::Unsupported { opcode: 0xfa }),
        ),
        (
            &[0x66, 0xfa][..],
            Some(RunExit::Unsupported { opcode: 0xfa }),
        ),
        (&[0xfb][..], Some(RunExit::Unsupported { opcode: 0xfb })),
        (
            &[0xf0, 0xfa][..],
            Some(RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })),
        ),
    ] {
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        let before = state.clone();
        let mut guest = memory(bytes, state.rip);
        let result = run_with_cli_profile(
            &mut state,
            &mut guest,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context,
            },
            &profile,
            || false,
        );
        assert_eq!(result.exit, expected.unwrap(), "bytes={bytes:02x?}");
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(state, before);
    }

    let mut state = LongModeState::default();
    state.rip = 0x1000;
    let mut guest = memory(&[0xfa], state.rip);
    let legacy = run(
        &mut state,
        &mut guest,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context,
        },
        || false,
    );
    assert_eq!(legacy.exit, RunExit::Unsupported { opcode: 0xfa });

    let mut state = LongModeState::default();
    state.rip = 0x1000;
    let mut guest = memory(&[0xf0, 0xfa], state.rip);
    let stopped = run_with_cli_profile(
        &mut state,
        &mut guest,
        RunConfig {
            instruction_budget: 0,
            stop_rip: Some(0x1000),
            context,
        },
        &profile,
        || false,
    );
    assert_eq!(stopped.exit, RunExit::StopAddress);
    assert_eq!(state.rip, 0x1000);
    let budgeted = run_with_cli_profile(
        &mut state,
        &mut guest,
        RunConfig {
            instruction_budget: 0,
            stop_rip: None,
            context,
        },
        &profile,
        || false,
    );
    assert_eq!(budgeted.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(state.rip, 0x1000);

    let mut state = LongModeState::default();
    state.rip = 0x1000;
    let mut guest = memory(&[0xf0, 0xfa], state.rip);
    guest.inject_once(
        0x1001,
        AccessKind::Fetch,
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x1001),
        }),
    );
    let fetch_fault = run_with_cli_profile(
        &mut state,
        &mut guest,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context,
        },
        &profile,
        || false,
    );
    assert_eq!(
        fetch_fault.exit,
        RunExit::Fault(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x1001)
        })
    );
    assert_eq!(state.rip, 0x1000);

    // CLI retires at the first fresh boundary; caller then resubmits its request.
    let mut state = LongModeState::default();
    state.rip = 0x2000;
    state.rflags |= 1 << 9;
    let mut guest = memory(&[0xfa, 0x90], state.rip);
    let cli = run_with_cli_profile(
        &mut state,
        &mut guest,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(0x2001),
            context,
        },
        &profile,
        || false,
    );
    assert_eq!(cli.exit, RunExit::StopAddress);
    assert_eq!(cli.instructions_executed, 1);
    assert_eq!(state.rip, 0x2001);
    assert_eq!(state.rflags & (1 << 9), 0);
    let caller_request = Some(0x42);
    let after_cli = run_with_maskable_request(
        &mut state,
        &mut guest,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context,
        },
        INTEGER_FLAGS,
        caller_request,
        || false,
    )
    .unwrap();
    assert_eq!(caller_request, Some(0x42));
    assert!(matches!(
        after_cli,
        MaskableRequestRunResult::Run(RunResult {
            exit: RunExit::InstructionBudgetExhausted,
            instructions_executed: 1,
            ..
        })
    ));
    assert_eq!(state.rip, 0x2002);

    let mut if_set = state.clone();
    if_set.rip = 0x2001;
    if_set.rflags |= 1 << 9;
    let mut eligible_guest = memory(&[0x90], if_set.rip);
    assert_eq!(
        run_with_maskable_request(
            &mut if_set,
            &mut eligible_guest,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context
            },
            INTEGER_FLAGS,
            caller_request,
            || false
        )
        .unwrap(),
        MaskableRequestRunResult::InterruptEligible {
            vector: 0x42,
            rip: 0x2001
        },
    );
}

#[test]
fn backwards_branches_budget_and_stop_have_precise_boundaries() {
    let code = [0x48, 0x2b, 0xc1, 0x75, 0xfb];
    let mut mem = memory(&code, 0);
    let mut state = LongModeState::default();
    state.gpr[0] = 3;
    state.gpr[1] = 1;
    let result = run(&mut state, &mut mem, config(3, Some(5)), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.rip, 3);
    assert_eq!(state.gpr[0], 1);
    let result = run(&mut state, &mut mem, config(3, Some(5)), || false);
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 3);
    assert_eq!(state.gpr[0], 0);
    assert_eq!(state.rip, 5);
    state.rip = 0;
    let before = state.clone();
    let result = run(&mut state, &mut mem, config(0, None), || true);
    assert_eq!(result.exit, RunExit::StopRequested);
    assert_eq!(state, before);
    let result = run(&mut state, &mut mem, config(0, None), || false);
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(state, before);
}

#[test]
fn failed_fetch_and_unsupported_encoding_never_commit_cpu_or_ram() {
    let code = [0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8];
    for length in 1..code.len() {
        let mut mem = memory(&code[..length], 0);
        let mut state = LongModeState {
            cr3: None,
            cr4: None,
            sti_shadow: None,
            gpr: [u64::MAX; 16],
            ..LongModeState::default()
        };
        let before = state.clone();
        let ram = mem.snapshot();
        assert!(matches!(
            step(&mut state, &mut mem, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(state, before);
        assert_eq!(mem.snapshot(), ram);
    }
    for code in [
        &[0x0f, 0x05][..],
        &[0x64, 0x48, 0x8b, 0x03][..],
        &[0xf0, 0x48, 0x03, 0xc4][..],
    ] {
        let mut mem = memory(code, 0);
        let mut state = LongModeState::default();
        let before = state.clone();
        assert!(matches!(
            step(&mut state, &mut mem, AccessContext::default()),
            Ok(StepOutcome::Unsupported { .. })
        ));
        assert_eq!(state, before);
    }
    let fault = ArchitecturalFault {
        vector: 14,
        error_code: Some(4),
        linear_address: Some(5),
    };
    let mut mem = memory(&code, 0);
    mem.inject_once(
        5,
        AccessKind::Fetch,
        MemoryError::Architectural(fault.clone()),
    );
    let mut state = LongModeState::default();
    let before = state.clone();
    let result = run(&mut state, &mut mem, config(10, None), || false);
    assert_eq!(result.exit, RunExit::Fault(fault));
    assert_eq!(result.instructions_executed, 0);
    assert_eq!(state, before);
    state.rip = 0x0000_8000_0000_0000;
    assert!(matches!(
        step(&mut state, &mut mem, AccessContext::default()),
        Ok(StepOutcome::Fault(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            ..
        }))
    ));
}

#[test]
fn old_forms_use_the_same_arithmetic_and_preserve_extra_registers() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/x86");
    for name in [
        "s0_09_mov_imm64",
        "s0_09_add_overflow",
        "s0_09_cmp_jz",
        "s0_09_sub_jnz_loop",
    ] {
        let old = crate::case::Fixture::from_json(
            &std::fs::read(root.join(format!("{name}.case.json"))).unwrap(),
        )
        .unwrap();
        let converted = Fixture::convert_v1(&old).unwrap();
        let code = old.fixture.program.bytes().unwrap();
        let mut state = converted.initial_state.to_cpu().unwrap();
        let mut expected = od_isa::FragmentState::try_from(&state).unwrap();
        let fragment =
            od_isa::interpret(&code, &mut expected, converted.instruction_budget).unwrap();
        let mut mem = FlatMemory::from_fixture(&converted).unwrap();
        state.gpr[15] = u64::MAX;
        let result = run(
            &mut state,
            &mut mem,
            config(
                converted.instruction_budget,
                converted.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, fragment.instructions_executed);
        assert_eq!(&state.gpr[..4], &fragment.state.gpr);
        assert_eq!(state.rip, fragment.state.rip as u64);
        assert_eq!(state.rflags, fragment.state.rflags);
        assert_eq!(state.gpr[15], u64::MAX);
    }
}

#[test]
fn ud2_fetch_boundary_is_exact_across_complete_and_adjacent_executable_regions() {
    let rip = 0x1000;
    for (regions, label) in [
        (vec![("code", rip, &[0x0f, 0x0b][..])], "single region"),
        (
            vec![("code", rip, &[0x0f][..]), ("tail", rip + 1, &[0x0b][..])],
            "adjacent regions",
        ),
    ] {
        let mut state = LongModeState {
            cr3: None,
            cr4: None,
            sti_shadow: None,
            rip,
            gpr: [
                0x1122_3344_5566_7788,
                0xaabb_ccdd_eeff_0011,
                0xdead_beef_fedc_ba98,
                0x0000_0000_0020_0000,
                0x0000_0000_0030_0080,
                0x0000_0000_0030_0080,
                0x0000_0000_0020_0004,
                0x0000_0000_0020_0010,
                0x0123_4567_89ab_cdef,
                0xfedc_ba98_7654_3210,
                0x8877_6655_4433_2211,
                0xaabb_ccdd_eeff_0011,
                0x1122_1122_1122_1122,
                0x3344_3344_3344_3344,
                0x7788_99aa_bbcc_ddee,
                u64::MAX,
            ],
            rflags: 0x08d7,
            ..LongModeState::default()
        };
        let before = state.clone();
        let inner = ud2_memory_regions(&regions);
        let before_memory = inner.snapshot();
        let mut memory = AccessTraceMemory {
            inner,
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            }),
            "{label}"
        );
        assert_eq!(result.instructions_executed, 0, "{label}");
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS, "{label}");
        assert_eq!(state, before, "{label}");
        assert_eq!(memory.inner.snapshot(), before_memory, "{label}");
        assert_eq!(
            memory.accesses,
            [
                (AccessKind::Fetch, rip, 1, AccessContext::default()),
                (AccessKind::Fetch, rip + 1, 1, AccessContext::default()),
            ],
            "{label} must fetch exactly both UD2 bytes and no data"
        );
    }
}

#[test]
fn ud2_second_fetch_errors_preserve_backend_class_payload_and_state() {
    let rip = 0x2000;
    let errors = [
        MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0x25),
            linear_address: Some(0x1234_5678),
        }),
        MemoryError::PermissionDenied {
            address: rip + 1,
            access: AccessKind::Fetch,
        },
    ];
    for error in errors {
        let mut state = LongModeState {
            cr3: None,
            cr4: None,
            sti_shadow: None,
            rip,
            gpr: [0xfeed_face_cafe_beef; 16],
            rflags: 0x08d7,
            ..LongModeState::default()
        };
        let before = state.clone();
        let mut inner = ud2_memory_regions(&[("code", rip, &[0x0f, 0x0b])]);
        let before_memory = inner.snapshot();
        inner.inject_once(rip + 1, AccessKind::Fetch, error.clone());
        let mut memory = AccessTraceMemory {
            inner,
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        match error {
            MemoryError::Architectural(fault) => assert_eq!(result.exit, RunExit::Fault(fault)),
            backend_error => assert_eq!(result.exit, RunExit::MemoryError(backend_error)),
        }
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(state, before);
        assert_eq!(memory.inner.snapshot(), before_memory);
        assert_eq!(
            memory.accesses,
            [
                (AccessKind::Fetch, rip, 1, AccessContext::default()),
                (AccessKind::Fetch, rip + 1, 1, AccessContext::default()),
            ]
        );
    }
}

#[test]
fn ud2_missing_second_byte_and_first_fetch_failure_do_not_become_ud_or_commit() {
    let rip = 0x3000;
    let cases = [
        (
            ud2_memory_regions(&[("code", rip, &[0x0f])]),
            None,
            RunExit::MemoryError(MemoryError::Unmapped {
                address: rip + 1,
                access: AccessKind::Fetch,
            }),
            2,
        ),
        (
            ud2_memory_regions(&[("code", rip, &[0x0f, 0x0b])]),
            Some((
                rip,
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(7),
                    linear_address: Some(rip),
                }),
            )),
            RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(rip),
            }),
            1,
        ),
    ];
    for (mut inner, injected, expected_exit, fetch_count) in cases {
        let mut state = LongModeState {
            cr3: None,
            cr4: None,
            sti_shadow: None,
            rip,
            gpr: [0x0123_4567_89ab_cdef; 16],
            rflags: 0x08d7,
            ..LongModeState::default()
        };
        let before = state.clone();
        let before_memory = inner.snapshot();
        if let Some((address, error)) = injected {
            inner.inject_once(address, AccessKind::Fetch, error);
        }
        let mut memory = AccessTraceMemory {
            inner,
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, expected_exit);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, od_isa::INTEGER_FLAGS);
        assert_eq!(state, before);
        assert_eq!(memory.inner.snapshot(), before_memory);
        assert_eq!(memory.accesses.len(), fetch_count);
        assert!(
            memory
                .accesses
                .iter()
                .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
        );
    }
}

#[test]
fn s2_30ci_test_producers_check_each_full_intermediate_snapshot() {
    for (raw, rax, should_move) in [
        (
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_test_pf0_taken.case.json"
            )
            .as_slice(),
            1u64,
            true,
        ),
        (
            include_bytes!(
                "../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_test_pf1_untaken.case.json"
            )
            .as_slice(),
            3u64,
            false,
        ),
    ] {
        let fixture = Fixture::from_json(raw).unwrap().fixture;
        let base_rip = fixture.initial_state.rip.get();
        let mut state = fixture
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        assert_eq!(
            fixture.memory[0].initial_bytes_hex,
            "d9e8d9ee4885c0dbd9dfe0"
        );
        assert_eq!(fixture.memory[0].length, 11);
        assert_eq!(fixture.instruction_budget, 5);

        let mut expected = state.clone();
        let x87 = expected.x87.as_mut().unwrap();
        x87.registers[7] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
        x87.set_tag(7, od_isa::state::X87Tag::Valid);
        x87.status_word = 7 << 11;
        x87.instruction_pointer = base_rip;
        expected.rip += 2;
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state, expected, "after FLD1");

        let mut expected = state.clone();
        let x87 = expected.x87.as_mut().unwrap();
        x87.registers[6] = [0; 10];
        x87.set_tag(6, od_isa::state::X87Tag::Zero);
        x87.status_word = 6 << 11;
        x87.instruction_pointer = base_rip + 2;
        expected.rip += 2;
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state, expected, "after FLDZ");

        let mut expected = state.clone();
        expected.rflags = 2 | if rax == 3 { 1 << 2 } else { 0 };
        expected.rip += 3;
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state, expected, "after TEST at RIP+4");
        assert_eq!(state.rip, base_rip + 7);

        let mut expected = state.clone();
        let x87 = expected.x87.as_mut().unwrap();
        if should_move {
            x87.registers[6] = x87.registers[7];
            x87.set_tag(6, od_isa::state::X87Tag::Valid);
        }
        x87.instruction_pointer = base_rip + 7;
        expected.rip += 2;
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state, expected, "after FCMOVNU at RIP+7");

        let mut expected = state.clone();
        let status_word = expected.x87.as_ref().unwrap().status_word;
        expected.gpr[0] = (expected.gpr[0] & !0xffff) | u64::from(status_word);
        expected.rip += 2;
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state, expected, "after FNSTSW AX");
        assert_eq!(state.rip, base_rip + 11);
        assert_eq!(
            state.x87.as_ref().unwrap().instruction_pointer,
            base_rip + 7
        );
    }
}

#[test]
fn rep_cmps_wdq_literal_fault_answers_match_the_typed_exit() {
    let directory = arithmetic_fixture_directory();
    let answers: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_slice(
        &std::fs::read(directory.join("s2_19h_rep_cmps_wdq_expected.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(answers.len(), 71);
    let mut checked = 0;
    for (id, answer) in answers {
        let Some(fault) = answer.get("fault") else {
            continue;
        };
        let input =
            Fixture::from_json(&std::fs::read(directory.join(format!("{id}.case.json"))).unwrap())
                .unwrap()
                .fixture;
        let mut state = input.initial_state.to_cpu().unwrap();
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let result = run(
            &mut state,
            &mut memory,
            config(input.instruction_budget, input.stop_rip.map(HexU64::get)),
            || false,
        );
        if fault.get("vector").is_some() {
            assert_eq!(fault["vector"], 13, "{id}");
            assert_eq!(fault["error_code"], 0, "{id}");
            assert!(fault["linear_address"].is_null(), "{id}");
            assert!(
                matches!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    })
                ),
                "{id}: {:?}",
                result.exit
            );
        } else {
            assert_eq!(fault["access"], "read", "{id}");
            let address: HexU64 = serde_json::from_value(fault["address"].clone()).unwrap();
            assert!(
                matches!(
                    result.exit,
                    RunExit::MemoryError(MemoryError::Unmapped {
                        address: actual,
                        access: AccessKind::Read,
                    }) if actual == address.get()
                ),
                "{id}: {:?}",
                result.exit
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 24);
}

#[test]
fn s2_30cj_literal_transfer_rejection_and_repeat_fixtures_match_full_snapshots() {
    for raw in [
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_taken_zero.case.json")
            .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_invalid_source_pf1.case.json"
        )
        .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30ci_fcmovnu_repeat.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i1.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i2.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i3.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i4.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i5.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i6.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_i7.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_wrap_top7_i1.case.json")
            .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_zero_to_normal_i2.case.json"
        )
        .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_self_pf0.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_self_pf1.case.json")
            .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_invalid_source_pf0.case.json"
        )
        .as_slice(),
        include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_invalid_destination.case.json"
        )
        .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_c1.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_es.case.json")
            .as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_b.case.json").as_slice(),
        include_bytes!("../../../tests/fixtures/x86/stage2/s2_30cj_fcmovnu_fcw.case.json")
            .as_slice(),
    ] {
        let fixture = Fixture::from_json(raw).unwrap().fixture;
        let mut state = fixture
            .initial_state
            .to_cpu_for(crate::v2::Profile::LongX87StateV1)
            .unwrap();
        let before = state.clone();
        let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
        let memory_before = memory.snapshot();
        let result = run(
            &mut state,
            &mut memory,
            config(
                fixture.instruction_budget,
                fixture.stop_rip.map(HexU64::get),
            ),
            || false,
        );
        if fixture.expected_exit == ExpectedExit::Unsupported {
            assert_eq!(
                result.exit,
                RunExit::Unsupported { opcode: 0xdb },
                "{}",
                fixture.case_id
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
            assert_eq!(memory.snapshot(), memory_before, "{}", fixture.case_id);
        } else {
            assert_eq!(result.exit, RunExit::StopAddress, "{}", fixture.case_id);
            assert_eq!(
                result.instructions_executed, fixture.instruction_budget,
                "{}",
                fixture.case_id
            );
            assert_eq!(
                state.rip,
                fixture.stop_rip.unwrap().get(),
                "{}",
                fixture.case_id
            );
            let code_hex = &fixture
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .unwrap()
                .initial_bytes_hex;
            let code = crate::v2::decode_bytes(code_hex).unwrap();
            let mut expected = before;
            let mut offset = 0usize;
            while offset < code.len() {
                match code[offset..] {
                    [0xdb, modrm, ..] if (0xd8..=0xdf).contains(&modrm) => {
                        let source_index = modrm & 7;
                        let x87 = expected.x87.as_mut().unwrap();
                        let top = x87.top();
                        let source = (top + source_index) & 7;
                        if expected.rflags & (1 << 2) == 0 {
                            x87.registers[usize::from(top)] =
                                x87.registers[usize::from(source)].clone();
                            let source_tag = (x87.tag_word >> (u32::from(source) * 2)) & 3;
                            let tag_mask = !(3u16 << (u32::from(top) * 2));
                            x87.tag_word =
                                (x87.tag_word & tag_mask) | (source_tag << (u32::from(top) * 2));
                        }
                        x87.instruction_pointer = fixture.initial_state.rip.get() + offset as u64;
                        offset += 2;
                    }
                    [0xdf, 0xe0, ..] => {
                        let status = expected.x87.as_ref().unwrap().status_word;
                        expected.gpr[0] = (expected.gpr[0] & !0xffff) | u64::from(status);
                        offset += 2;
                    }
                    _ => panic!("unexpected S2-30cj fixture stream: {code_hex}"),
                }
            }
            expected.rip = fixture.initial_state.rip.get() + code.len() as u64;
            assert_eq!(state, expected, "{}", fixture.case_id);
            assert_eq!(memory.snapshot(), memory_before, "{}", fixture.case_id);
            if fixture.case_id == "s2_30ci_fcmovnu_taken_zero" {
                assert_eq!(
                    state
                        .x87
                        .as_ref()
                        .unwrap()
                        .tag(state.x87.as_ref().unwrap().top()),
                    Some(od_isa::state::X87Tag::Zero)
                );
            }
        }
    }
}

#[test]
fn local_xorps_register_uses_literal_pattern_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_7a_xorps_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 4);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i] ^ before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_xorpd_register_uses_high_xmm_literal_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_17_xorpd_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i] ^ before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_orpd_register_uses_high_xmm_literal_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_19_orpd_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i] | before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_andnpd_register_uses_high_xmm_literal_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_20_andnpd_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        !before.sse.as_ref().unwrap().xmm[8][i] & before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(
        expected,
        [
            0x0f, 0xe0, 0x0d, 0xc0, 0x0b, 0xa0, 0x09, 0x80, 0x07, 0x60, 0x05, 0x40, 0x03, 0x20,
            0x01, 0x00
        ]
    );
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_pxor_register_uses_literal_pattern_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_1_pxor_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i] ^ before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_pand_register_uses_literal_pattern_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_49_pand_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i] & before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_pandn_register_uses_literal_pattern_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_50_pandn_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        !before.sse.as_ref().unwrap().xmm[8][i] & before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_por_register_uses_literal_pattern_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_51_por_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i] | before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_paddb_register_uses_literal_lanes_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_2_paddb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i].wrapping_add(before.sse.as_ref().unwrap().xmm[9][i])
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);

    let mut state = before;
    let sse = state.sse.as_mut().unwrap();
    sse.xmm[8] = [0xff, 0x7f, 0xff, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    sse.xmm[9] = [0x01, 0x01, 0x01, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    state.rip = fixture.initial_state.rip.get();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[8][..4],
        &[0x00, 0x80, 0x00, 0xff]
    );
}

#[test]
fn local_psubb_register_uses_literal_lanes_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_3_psubb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i].wrapping_sub(before.sse.as_ref().unwrap().xmm[9][i])
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);

    let mut state = before;
    let sse = state.sse.as_mut().unwrap();
    sse.xmm[8] = [0x01, 0x02, 0x00, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    sse.xmm[9] = [0x02, 0x01, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    state.rip = fixture.initial_state.rip.get();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[8][..4],
        &[0xff, 0x01, 0xff, 0xfe]
    );

    // Byte lanes do not borrow from each other.
    {
        let sse = state.sse.as_mut().unwrap();
        sse.xmm[8] = [0; 16];
        sse.xmm[9] = [0; 16];
        sse.xmm[8][1] = 2;
        sse.xmm[9][0] = 1;
        sse.xmm[9][1] = 1;
    }
    state.rip = fixture.initial_state.rip.get();
    run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(&state.sse.as_ref().unwrap().xmm[8][..2], &[0xff, 1]);
}

#[test]
fn local_pshufd_register_uses_literal_control_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_35_pshufd_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 6);
    let source = before.sse.as_ref().unwrap().xmm[9];
    let expected: [u8; 16] = std::array::from_fn(|index| {
        let lane = index / 4;
        let selected = usize::from((0x1b_u8 >> (lane * 2)) & 3);
        source[selected * 4 + index % 4]
    });
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_paddusb_register_uses_literal_lanes_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_4_paddusb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i]
            .saturating_add(before.sse.as_ref().unwrap().xmm[9][i])
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);

    let mut state = before;
    {
        let sse = state.sse.as_mut().unwrap();
        sse.xmm[8] = [
            0xff, 0x7f, 0x80, 0x00, 0xfe, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        sse.xmm[9] = [
            0x01, 0x01, 0x80, 0xff, 0x01, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
    }
    state.rip = fixture.initial_state.rip.get();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[8][..6],
        &[0xff, 0x80, 0xff, 0xff, 0xff, 0xff]
    );

    // Saturation is lane-local.
    {
        let sse = state.sse.as_mut().unwrap();
        sse.xmm[8] = [0; 16];
        sse.xmm[9] = [0; 16];
        sse.xmm[8][0] = 0xff;
        sse.xmm[9][0] = 1;
        sse.xmm[8][1] = 1;
        sse.xmm[9][1] = 1;
    }
    state.rip = fixture.initial_state.rip.get();
    run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(&state.sse.as_ref().unwrap().xmm[8][..2], &[0xff, 2]);
}
#[test]
fn local_psubusb_register_uses_literal_lanes_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_5_psubusb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i]
            .saturating_sub(before.sse.as_ref().unwrap().xmm[9][i])
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);

    let mut state = before;
    {
        let sse = state.sse.as_mut().unwrap();
        sse.xmm[8] = [
            0x00, 0x02, 0x01, 0xff, 0x80, 0x7f, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        sse.xmm[9] = [
            0x01, 0x01, 0x02, 0x01, 0x7f, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
    }
    state.rip = fixture.initial_state.rip.get();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[8][..7],
        &[0x00, 0x01, 0x00, 0xfe, 0x01, 0x00, 0xff]
    );

    // Saturating subtraction is unsigned and lane-local.
    {
        let sse = state.sse.as_mut().unwrap();
        sse.xmm[8] = [0; 16];
        sse.xmm[9] = [0; 16];
        sse.xmm[8][1] = 2;
        sse.xmm[9][0] = 1;
        sse.xmm[9][1] = 1;
    }
    state.rip = fixture.initial_state.rip.get();
    run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(&state.sse.as_ref().unwrap().xmm[8][..2], &[0, 1]);

    {
        let sse = state.sse.as_mut().unwrap();
        sse.xmm[8] = [0x5a; 16];
        sse.xmm[9] = [0x5a; 16];
    }
    state.rip = fixture.initial_state.rip.get();
    run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], [0; 16]);
}

#[test]
fn local_paddw_register_uses_literal_word_lanes_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_6_paddw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let source = before.sse.as_ref().unwrap().xmm[9];
    let expected: [u8; 16] = std::array::from_fn(|i| {
        if i % 2 == 0 {
            let a = u16::from_le_bytes([
                before.sse.as_ref().unwrap().xmm[8][i],
                before.sse.as_ref().unwrap().xmm[8][i + 1],
            ]);
            let b = u16::from_le_bytes([source[i], source[i + 1]]);
            a.wrapping_add(b).to_le_bytes()[0]
        } else {
            let a = u16::from_le_bytes([
                before.sse.as_ref().unwrap().xmm[8][i - 1],
                before.sse.as_ref().unwrap().xmm[8][i],
            ]);
            let b = u16::from_le_bytes([source[i - 1], source[i]]);
            a.wrapping_add(b).to_le_bytes()[1]
        }
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
    let sse = state.sse.as_mut().unwrap();
    sse.xmm[8] = [
        0xff, 0x00, 1, 0, 0xff, 0xff, 1, 0, 0, 0x80, 0, 0x80, 0xff, 0x7f, 0, 0,
    ];
    sse.xmm[9] = [1, 0, 1, 0, 1, 0, 1, 0, 0, 0x80, 0, 0x80, 1, 0, 0, 0];
    state.rip = before.rip;
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[8][..14],
        &[0, 1, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0x80]
    );
}

#[test]
fn paddw_exhausts_every_word_lhs_against_boundary_addends() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_6_paddw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let base = fixture.initial_state.rip.get();
    for lhs in 0..=u16::MAX {
        for rhs in [0, 1, 0x7fff, 0x8000, u16::MAX] {
            state.rip = base;
            let sse = state.sse.as_mut().unwrap();
            sse.xmm[8][0..2].copy_from_slice(&lhs.to_le_bytes());
            sse.xmm[9][0..2].copy_from_slice(&rhs.to_le_bytes());
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            assert_eq!(
                &state.sse.as_ref().unwrap().xmm[8][0..2],
                &lhs.wrapping_add(rhs).to_le_bytes(),
                "{lhs:04x}+{rhs:04x}"
            );
        }
    }
}

#[test]
fn paddw_interpreter_covers_all_register_pairs_and_preserves_other_state() {
    for destination in 0..16u8 {
        for source in 0..16u8 {
            let rex = 0x40 | ((destination >> 3) << 2) | (source >> 3);
            let mut code = vec![0x66];
            if destination >= 8 || source >= 8 {
                code.push(rex);
            }
            code.extend([0x0f, 0xfd, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let mut memory = memory(&code, 0x1000);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.x87 = Some(od_isa::state::X87State::fixture_seed());
            state.sse = Some(od_isa::state::SseState {
                xmm: [[0; 16]; 16],
                mxcsr: 0x1f80,
            });
            for r in 0..16 {
                for lane in 0..8 {
                    let a = (r as u16)
                        .wrapping_mul(1021)
                        .wrapping_add((lane as u16).wrapping_mul(8191));
                    let b = (r as u16)
                        .wrapping_mul(313)
                        .wrapping_add((lane as u16).wrapping_mul(12347));
                    state.sse.as_mut().unwrap().xmm[r][lane * 2..lane * 2 + 2]
                        .copy_from_slice(&a.to_le_bytes());
                    state.sse.as_mut().unwrap().xmm[r][lane * 2 + 1] = b.to_le_bytes()[1];
                }
            }
            state.rflags = 0x8d7 | (1 << 10);
            let before = state.clone();
            let memory_before = memory.snapshot();
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            let mut expected = before.clone();
            expected.rip += code.len() as u64;
            for lane in 0..8 {
                let a = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2 + 1],
                ]);
                let b = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2 + 1],
                ]);
                expected.sse.as_mut().unwrap().xmm[destination as usize][lane * 2..lane * 2 + 2]
                    .copy_from_slice(&a.wrapping_add(b).to_le_bytes());
            }
            assert_eq!(state, expected, "dest={destination} source={source}");
            assert_eq!(
                memory.snapshot(),
                memory_before,
                "data access for {destination},{source}"
            );
        }
    }
}

#[test]
fn paddw_missing_state_truncation_stop_and_budget_are_atomic() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_6_paddw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let code = [0x66, 0x0f, 0xfd, 0xc1];
    for missing_x87 in [true, false] {
        let mut candidate = state.clone();
        if missing_x87 {
            candidate.x87 = None;
        } else {
            candidate.sse = None;
        }
        let before = candidate.clone();
        let mut memory = memory(&code, fixture.initial_state.rip.get());
        let memory_before = memory.snapshot();
        assert!(matches!(
            step(&mut candidate, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xfd })
        ));
        assert_eq!(candidate, before);
        assert_eq!(memory.snapshot(), memory_before);
    }
    let mut truncated_memory = memory(&code[..3], fixture.initial_state.rip.get());
    state.rip = fixture.initial_state.rip.get();
    let before = state.clone();
    let memory_before = truncated_memory.snapshot();
    assert!(step(&mut state, &mut truncated_memory, AccessContext::default()).is_err());
    assert_eq!(state, before);
    assert_eq!(truncated_memory.snapshot(), memory_before);
    for (budget, stop, expected) in [
        (0, None, RunExit::InstructionBudgetExhausted),
        (1, Some(before.rip), RunExit::StopAddress),
    ] {
        let mut candidate = before.clone();
        let mut memory = memory(&code, candidate.rip);
        let memory_before = memory.snapshot();
        let result = run(
            &mut candidate,
            &mut memory,
            RunConfig {
                instruction_budget: budget,
                stop_rip: stop,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, expected);
        assert_eq!(candidate, before);
        assert_eq!(memory.snapshot(), memory_before);
    }
    let mut cancelled = before.clone();
    let mut memory = memory(&code, cancelled.rip);
    let memory_before = memory.snapshot();
    let result = run(
        &mut cancelled,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || true,
    );
    assert_eq!(result.exit, RunExit::StopRequested);
    assert_eq!(cancelled, before);
    assert_eq!(memory.snapshot(), memory_before);
}
#[test]
fn local_psubw_register_uses_literal_word_lanes_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_7_psubw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let source = before.sse.as_ref().unwrap().xmm[9];
    let expected: [u8; 16] = std::array::from_fn(|i| {
        if i % 2 == 0 {
            let a = u16::from_le_bytes([
                before.sse.as_ref().unwrap().xmm[8][i],
                before.sse.as_ref().unwrap().xmm[8][i + 1],
            ]);
            let b = u16::from_le_bytes([source[i], source[i + 1]]);
            a.wrapping_sub(b).to_le_bytes()[0]
        } else {
            let a = u16::from_le_bytes([
                before.sse.as_ref().unwrap().xmm[8][i - 1],
                before.sse.as_ref().unwrap().xmm[8][i],
            ]);
            let b = u16::from_le_bytes([source[i - 1], source[i]]);
            a.wrapping_sub(b).to_le_bytes()[1]
        }
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
    let sse = state.sse.as_mut().unwrap();
    sse.xmm[8] = [
        0x00, 0x01, 0, 0, 2, 0, 1, 0, 2, 0, 0, 0x80, 0xff, 0x7f, 0x34, 0x12,
    ];
    sse.xmm[9] = [1, 0, 1, 0, 1, 0, 2, 0, 1, 0, 1, 0, 0xff, 0xff, 0x34, 0x12];
    state.rip = before.rip;
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[8][..14],
        &[
            0xff, 0x00, 0xff, 0xff, 1, 0, 0xff, 0xff, 1, 0, 0xff, 0x7f, 0, 0x80
        ]
    );
}

#[test]
fn psubw_exhausts_every_word_lhs_against_boundary_addends() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_7_psubw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let base = fixture.initial_state.rip.get();
    for lhs in 0..=u16::MAX {
        for rhs in [0, 1, 0x7fff, 0x8000, u16::MAX] {
            state.rip = base;
            let sse = state.sse.as_mut().unwrap();
            sse.xmm[8][0..2].copy_from_slice(&lhs.to_le_bytes());
            sse.xmm[9][0..2].copy_from_slice(&rhs.to_le_bytes());
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            assert_eq!(
                &state.sse.as_ref().unwrap().xmm[8][0..2],
                &lhs.wrapping_sub(rhs).to_le_bytes(),
                "{lhs:04x}+{rhs:04x}"
            );
        }
    }
}

#[test]
fn psubw_interpreter_covers_all_register_pairs_and_preserves_other_state() {
    for destination in 0..16u8 {
        for source in 0..16u8 {
            let rex = 0x40 | ((destination >> 3) << 2) | (source >> 3);
            let mut code = vec![0x66];
            if destination >= 8 || source >= 8 {
                code.push(rex);
            }
            code.extend([0x0f, 0xf9, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let mut memory = memory(&code, 0x1000);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.x87 = Some(od_isa::state::X87State::fixture_seed());
            state.sse = Some(od_isa::state::SseState {
                xmm: [[0; 16]; 16],
                mxcsr: 0x1f80,
            });
            for r in 0..16 {
                for lane in 0..8 {
                    let a = (r as u16)
                        .wrapping_mul(1021)
                        .wrapping_sub((lane as u16).wrapping_mul(8191));
                    let b = (r as u16)
                        .wrapping_mul(313)
                        .wrapping_sub((lane as u16).wrapping_mul(12347));
                    state.sse.as_mut().unwrap().xmm[r][lane * 2..lane * 2 + 2]
                        .copy_from_slice(&a.to_le_bytes());
                    state.sse.as_mut().unwrap().xmm[r][lane * 2 + 1] = b.to_le_bytes()[1];
                }
            }
            state.rflags = 0x8d7 | (1 << 10);
            let before = state.clone();
            let memory_before = memory.snapshot();
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            let mut expected = before.clone();
            expected.rip += code.len() as u64;
            for lane in 0..8 {
                let a = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2 + 1],
                ]);
                let b = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2 + 1],
                ]);
                expected.sse.as_mut().unwrap().xmm[destination as usize][lane * 2..lane * 2 + 2]
                    .copy_from_slice(&a.wrapping_sub(b).to_le_bytes());
            }
            assert_eq!(state, expected, "dest={destination} source={source}");
            assert_eq!(
                memory.snapshot(),
                memory_before,
                "data access for {destination},{source}"
            );
        }
    }
}

#[test]
fn psubw_missing_state_truncation_stop_and_budget_are_atomic() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_7_psubw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let code = [0x66, 0x0f, 0xf9, 0xc1];
    for missing_x87 in [true, false] {
        let mut candidate = state.clone();
        if missing_x87 {
            candidate.x87 = None;
        } else {
            candidate.sse = None;
        }
        let before = candidate.clone();
        let mut memory = memory(&code, fixture.initial_state.rip.get());
        let memory_before = memory.snapshot();
        assert!(matches!(
            step(&mut candidate, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xf9 })
        ));
        assert_eq!(candidate, before);
        assert_eq!(memory.snapshot(), memory_before);
    }
    let mut truncated_memory = memory(&code[..3], fixture.initial_state.rip.get());
    state.rip = fixture.initial_state.rip.get();
    let before = state.clone();
    let memory_before = truncated_memory.snapshot();
    assert!(step(&mut state, &mut truncated_memory, AccessContext::default()).is_err());
    assert_eq!(state, before);
    assert_eq!(truncated_memory.snapshot(), memory_before);
    for (budget, stop, expected) in [
        (0, None, RunExit::InstructionBudgetExhausted),
        (1, Some(before.rip), RunExit::StopAddress),
    ] {
        let mut candidate = before.clone();
        let mut memory = memory(&code, candidate.rip);
        let memory_before = memory.snapshot();
        let result = run(
            &mut candidate,
            &mut memory,
            RunConfig {
                instruction_budget: budget,
                stop_rip: stop,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, expected);
        assert_eq!(candidate, before);
        assert_eq!(memory.snapshot(), memory_before);
    }
    let mut cancelled = before.clone();
    let mut memory = memory(&code, cancelled.rip);
    let memory_before = memory.snapshot();
    let result = run(
        &mut cancelled,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || true,
    );
    assert_eq!(result.exit, RunExit::StopRequested);
    assert_eq!(cancelled, before);
    assert_eq!(memory.snapshot(), memory_before);
}
#[test]
fn local_paddusw_register_uses_literal_word_lanes_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_8_paddusw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let source = before.sse.as_ref().unwrap().xmm[9];
    let expected: [u8; 16] = std::array::from_fn(|i| {
        if i % 2 == 0 {
            let a = u16::from_le_bytes([
                before.sse.as_ref().unwrap().xmm[8][i],
                before.sse.as_ref().unwrap().xmm[8][i + 1],
            ]);
            let b = u16::from_le_bytes([source[i], source[i + 1]]);
            a.saturating_add(b).to_le_bytes()[0]
        } else {
            let a = u16::from_le_bytes([
                before.sse.as_ref().unwrap().xmm[8][i - 1],
                before.sse.as_ref().unwrap().xmm[8][i],
            ]);
            let b = u16::from_le_bytes([source[i - 1], source[i]]);
            a.saturating_add(b).to_le_bytes()[1]
        }
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
    let sse = state.sse.as_mut().unwrap();
    sse.xmm[8] = [
        0x00, 0x01, 0, 0, 2, 0, 1, 0, 2, 0, 0, 0x80, 0xff, 0x7f, 0x34, 0x12,
    ];
    sse.xmm[9] = [1, 0, 1, 0, 1, 0, 2, 0, 1, 0, 1, 0, 0xff, 0xff, 0x34, 0x12];
    state.rip = before.rip;
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[8][..16],
        &[
            1, 1, 1, 0, 3, 0, 3, 0, 3, 0, 1, 0x80, 0xff, 0xff, 0x68, 0x24
        ]
    );
}

#[test]
fn paddusw_saturates_unsigned_words_independently_and_keeps_raw_little_endian_lanes() {
    let mut state = LongModeState::default();
    state.rip = 0x1000;
    state.x87 = Some(od_isa::state::X87State::fixture_seed());
    state.sse = Some(od_isa::state::SseState {
        xmm: [[0; 16]; 16],
        mxcsr: 0x1f80,
    });
    let code = [0x66, 0x0f, 0xdd, 0xc1];
    let mut sim_memory = memory(&code, 0x1000);
    let vectors: [([u16; 8], [u16; 8]); 2] = [
        (
            [0xffff, 1, 0x7fff, 0x8000, 0x00ff, 0xfffe, 0xffff, 0],
            [1, 1, 1, 0x8000, 1, 2, 0, 0],
        ),
        (
            [0x7fff, 0x8000, 0xfffe, 0xffff, 0, 0, 0x1234, 0xffff],
            [1, 0x8000, 1, 1, 0xffff, 1, 0xedcb, 1],
        ),
    ];
    let results: [[u16; 8]; 2] = [
        [0xffff, 2, 0x8000, 0xffff, 0x0100, 0xffff, 0xffff, 0],
        [0x8000, 0xffff, 0xffff, 0xffff, 0xffff, 1, 0xffff, 0xffff],
    ];
    for (lhs, rhs) in vectors.into_iter().zip(results) {
        let sse = state.sse.as_mut().unwrap();
        for lane in 0..8 {
            sse.xmm[0][lane * 2..lane * 2 + 2].copy_from_slice(&lhs.0[lane].to_le_bytes());
            sse.xmm[1][lane * 2..lane * 2 + 2].copy_from_slice(&lhs.1[lane].to_le_bytes());
        }
        assert_eq!(
            step(&mut state, &mut sim_memory, AccessContext::default()).unwrap(),
            StepOutcome::Advanced
        );
        let output = &state.sse.as_ref().unwrap().xmm[0];
        for lane in 0..8 {
            assert_eq!(&output[lane * 2..lane * 2 + 2], &rhs[lane].to_le_bytes());
        }
        state.rip = 0x1000;
    }
    // Aliased source/destination covers the two relevant self-add thresholds.
    let mut alias_state = LongModeState::default();
    alias_state.rip = 0x2000;
    alias_state.x87 = Some(od_isa::state::X87State::fixture_seed());
    alias_state.sse = Some(od_isa::state::SseState {
        xmm: [[0; 16]; 16],
        mxcsr: 0x1f80,
    });
    let alias_code = [0x66, 0x0f, 0xdd, 0xc0];
    let mut alias_memory = memory(&alias_code, 0x2000);
    let xmm0 = &mut alias_state.sse.as_mut().unwrap().xmm[0];
    xmm0[0..2].copy_from_slice(&0x7fffu16.to_le_bytes());
    xmm0[2..4].copy_from_slice(&0x8000u16.to_le_bytes());
    assert_eq!(
        step(
            &mut alias_state,
            &mut alias_memory,
            AccessContext::default()
        )
        .unwrap(),
        StepOutcome::Advanced
    );
    assert_eq!(
        &alias_state.sse.as_ref().unwrap().xmm[0][0..4],
        &[0xfe, 0xff, 0xff, 0xff]
    );
}

#[test]
fn paddusw_exhausts_every_word_lhs_against_boundary_addends() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_8_paddusw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let base = fixture.initial_state.rip.get();
    for lhs in 0..=u16::MAX {
        for rhs in [0, 1, 0x7fff, 0x8000, u16::MAX] {
            state.rip = base;
            let sse = state.sse.as_mut().unwrap();
            sse.xmm[8][0..2].copy_from_slice(&lhs.to_le_bytes());
            sse.xmm[9][0..2].copy_from_slice(&rhs.to_le_bytes());
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            assert_eq!(
                &state.sse.as_ref().unwrap().xmm[8][0..2],
                &lhs.saturating_add(rhs).to_le_bytes(),
                "{lhs:04x}+{rhs:04x}"
            );
        }
    }
}

#[test]
fn paddusw_interpreter_covers_all_register_pairs_and_preserves_other_state() {
    for destination in 0..16u8 {
        for source in 0..16u8 {
            let rex = 0x40 | ((destination >> 3) << 2) | (source >> 3);
            let mut code = vec![0x66];
            if destination >= 8 || source >= 8 {
                code.push(rex);
            }
            code.extend([0x0f, 0xdd, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let mut memory = memory(&code, 0x1000);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.x87 = Some(od_isa::state::X87State::fixture_seed());
            state.sse = Some(od_isa::state::SseState {
                xmm: [[0; 16]; 16],
                mxcsr: 0x1f80,
            });
            for r in 0..16 {
                for lane in 0..8 {
                    let a = (r as u16)
                        .wrapping_mul(1021)
                        .saturating_add((lane as u16).wrapping_mul(8191));
                    let b = (r as u16)
                        .wrapping_mul(313)
                        .saturating_add((lane as u16).wrapping_mul(12347));
                    state.sse.as_mut().unwrap().xmm[r][lane * 2..lane * 2 + 2]
                        .copy_from_slice(&a.to_le_bytes());
                    state.sse.as_mut().unwrap().xmm[r][lane * 2 + 1] = b.to_le_bytes()[1];
                }
            }
            state.rflags = 0x8d7 | (1 << 10);
            let before = state.clone();
            let memory_before = memory.snapshot();
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            let mut expected = before.clone();
            expected.rip += code.len() as u64;
            for lane in 0..8 {
                let a = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2 + 1],
                ]);
                let b = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2 + 1],
                ]);
                expected.sse.as_mut().unwrap().xmm[destination as usize][lane * 2..lane * 2 + 2]
                    .copy_from_slice(&a.saturating_add(b).to_le_bytes());
            }
            assert_eq!(state, expected, "dest={destination} source={source}");
            assert_eq!(
                memory.snapshot(),
                memory_before,
                "data access for {destination},{source}"
            );
        }
    }
}

#[test]
fn paddusw_missing_state_truncation_stop_and_budget_are_atomic() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_8_paddusw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let code = [0x66, 0x0f, 0xdd, 0xc1];
    for missing_x87 in [true, false] {
        let mut candidate = state.clone();
        if missing_x87 {
            candidate.x87 = None;
        } else {
            candidate.sse = None;
        }
        let before = candidate.clone();
        let mut memory = memory(&code, fixture.initial_state.rip.get());
        let memory_before = memory.snapshot();
        assert!(matches!(
            step(&mut candidate, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdd })
        ));
        assert_eq!(candidate, before);
        assert_eq!(memory.snapshot(), memory_before);
    }
    let mut truncated_memory = memory(&code[..3], fixture.initial_state.rip.get());
    state.rip = fixture.initial_state.rip.get();
    let before = state.clone();
    let memory_before = truncated_memory.snapshot();
    assert!(step(&mut state, &mut truncated_memory, AccessContext::default()).is_err());
    assert_eq!(state, before);
    assert_eq!(truncated_memory.snapshot(), memory_before);
    for (budget, stop, expected) in [
        (0, None, RunExit::InstructionBudgetExhausted),
        (1, Some(before.rip), RunExit::StopAddress),
    ] {
        let mut candidate = before.clone();
        let mut memory = memory(&code, candidate.rip);
        let memory_before = memory.snapshot();
        let result = run(
            &mut candidate,
            &mut memory,
            RunConfig {
                instruction_budget: budget,
                stop_rip: stop,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, expected);
        assert_eq!(candidate, before);
        assert_eq!(memory.snapshot(), memory_before);
    }
    let mut cancelled = before.clone();
    let mut memory = memory(&code, cancelled.rip);
    let memory_before = memory.snapshot();
    let result = run(
        &mut cancelled,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || true,
    );
    assert_eq!(result.exit, RunExit::StopRequested);
    assert_eq!(cancelled, before);
    assert_eq!(memory.snapshot(), memory_before);
}
#[test]
fn local_psubb_register_covers_all_byte_pairs_and_wraps_without_carry_or_saturation() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_3_psubb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: AccessContext::default(),
    };
    for lhs in 0..=u8::MAX {
        for rhs in 0..=u8::MAX {
            let sse = state.sse.as_mut().unwrap();
            sse.xmm[8].fill(lhs);
            sse.xmm[9].fill(rhs);
            state.rip = fixture.initial_state.rip.get();
            let result = run(&mut state, &mut memory, config, || false);
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert!(
                state.sse.as_ref().unwrap().xmm[8]
                    .iter()
                    .all(|&byte| byte == lhs.wrapping_sub(rhs))
            );
            assert_eq!(state.sse.as_ref().unwrap().xmm[9], [rhs; 16]);
        }
    }
    // 0x01 - 0x02 wraps to 0xff; subtracting 0x01 from 0x02 produces 0x01.
    assert_eq!(1u8.wrapping_sub(2), 0xff);
    assert_eq!(2u8.wrapping_sub(1), 1);
}

#[test]
fn local_paddusb_register_covers_all_byte_pairs_with_unsigned_saturation() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_4_paddusb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: AccessContext::default(),
    };
    for lhs in 0..=u8::MAX {
        for rhs in 0..=u8::MAX {
            let sse = state.sse.as_mut().unwrap();
            sse.xmm[8].fill(lhs);
            sse.xmm[9].fill(rhs);
            state.rip = fixture.initial_state.rip.get();
            let result = run(&mut state, &mut memory, config, || false);
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert!(
                state.sse.as_ref().unwrap().xmm[8]
                    .iter()
                    .all(|&byte| byte == lhs.saturating_add(rhs))
            );
            assert_eq!(state.sse.as_ref().unwrap().xmm[9], [rhs; 16]);
        }
    }
    // 0xff+0x01 clamps at 0xff; 0x7f+0x01 remains 0x80 (unsigned saturation).
    assert_eq!(254u8.saturating_add(1), 255);
    assert_eq!(127u8.saturating_add(1), 128);
}

#[test]
fn local_andps_register_uses_literal_pattern_and_preserves_architectural_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_7b_andps_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 4);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        before.sse.as_ref().unwrap().xmm[8][i] & before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        expected,
        [
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d,
            0x8e, 0x8f
        ]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_psubusb_register_covers_all_byte_pairs_with_unsigned_saturation() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_5_psubusb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: AccessContext::default(),
    };
    for lhs in 0..=u8::MAX {
        for rhs in 0..=u8::MAX {
            let sse = state.sse.as_mut().unwrap();
            sse.xmm[8].fill(lhs);
            sse.xmm[9].fill(rhs);
            state.rip = fixture.initial_state.rip.get();
            let result = run(&mut state, &mut memory, config, || false);
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert!(
                state.sse.as_ref().unwrap().xmm[8]
                    .iter()
                    .all(|&byte| byte == lhs.saturating_sub(rhs))
            );
            assert_eq!(state.sse.as_ref().unwrap().xmm[9], [rhs; 16]);
        }
    }
    assert_eq!(0u8.saturating_sub(1), 0);
    assert_eq!(2u8.saturating_sub(1), 1);
    assert_eq!(1u8.saturating_sub(2), 0);
    assert_eq!(255u8.saturating_sub(1), 254);
    assert_eq!(128u8.saturating_sub(127), 1);
    assert_eq!(127u8.saturating_sub(128), 0);
}

#[test]
fn local_andnps_register_uses_original_destination_and_literal_result() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_7d_andnps_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 4);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        !before.sse.as_ref().unwrap().xmm[8][i] & before.sse.as_ref().unwrap().xmm[9][i]
    });
    assert_eq!(state.sse.as_ref().unwrap().xmm[8], expected);
    assert_eq!(
        expected,
        [
            0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10,
            0x10, 0x10
        ]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn andps_bytewise_truth_table_covers_every_input_pair() {
    for left in 0..=u8::MAX {
        for right in 0..=u8::MAX {
            let mut state = LongModeState::default();
            state.x87 = Some(od_isa::state::X87State::fixture_seed());
            state.sse = Some(od_isa::state::SseState {
                xmm: [[0; 16]; 16],
                mxcsr: 0x1f80,
            });
            state.sse.as_mut().unwrap().xmm[0].fill(left);
            state.sse.as_mut().unwrap().xmm[1].fill(right);
            let mut memory = memory(&[0x0f, 0x54, 0xc1], 0x1000);
            state.rip = 0x1000;
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert_eq!(state.sse.as_ref().unwrap().xmm[0], [left & right; 16]);
        }
    }
}

#[test]
fn andnps_bytewise_truth_table_covers_every_input_pair() {
    for left in 0..=u8::MAX {
        for right in 0..=u8::MAX {
            let mut state = LongModeState::default();
            state.x87 = Some(od_isa::state::X87State::fixture_seed());
            state.sse = Some(od_isa::state::SseState {
                xmm: [[0; 16]; 16],
                mxcsr: 0x1f80,
            });
            state.sse.as_mut().unwrap().xmm[0].fill(left);
            state.sse.as_mut().unwrap().xmm[1].fill(right);
            let mut memory = memory(&[0x0f, 0x55, 0xc1], 0x1000);
            state.rip = 0x1000;
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert_eq!(state.sse.as_ref().unwrap().xmm[0], [!left & right; 16]);
        }
    }
}

#[test]
fn local_movups_register_copies_all_128_raw_bits_and_preserves_non_sse_state() {
    let loaded = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_2_movups_register.case.json"
    ))
    .unwrap();
    let fixture = loaded.fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 4);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);

    let mut self_copy_fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_2_movups_register.case.json"
    ))
    .unwrap()
    .fixture;
    self_copy_fixture.memory[0].length = 3;
    self_copy_fixture.memory[0].initial_bytes_hex = "0f10c0".into();
    self_copy_fixture.memory[0].comparison_mask_hex = "ffffff".into();
    self_copy_fixture.stop_rip = Some(HexU64(self_copy_fixture.initial_state.rip.get() + 3));
    self_copy_fixture.validate().unwrap();
    let mut self_state = self_copy_fixture
        .initial_state
        .to_cpu_for(self_copy_fixture.profile)
        .unwrap();
    let self_before = self_state.clone();
    let mut self_memory = FlatMemory::from_fixture(&self_copy_fixture).unwrap();
    let self_result = run(
        &mut self_state,
        &mut self_memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: self_copy_fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(self_result.exit, RunExit::StopAddress);
    assert_eq!(self_result.instructions_executed, 1);
    assert_eq!(self_state.rip, self_before.rip + 3);
    assert_eq!(self_state.sse, self_before.sse);
    assert_eq!(self_state.x87, self_before.x87);
    assert_eq!(self_state.gpr, self_before.gpr);
    assert_eq!(self_state.rflags, self_before.rflags);
}

#[test]
fn local_movaps_register_fixture_copies_all_raw_bytes_and_preserves_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_11_movaps_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 4);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
}

#[test]
fn local_movaps_reverse_register_fixture_copies_xmm8_to_xmm9_and_preserves_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_12_movaps_reverse_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 4);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[8]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8],
        before.sse.as_ref().unwrap().xmm[8]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[..8],
        before.sse.as_ref().unwrap().xmm[..8]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[10..],
        before.sse.as_ref().unwrap().xmm[10..]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_movapd_register_fixture_copies_literal_raw_xmm_and_preserves_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_13_movapd_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8],
        [
            0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d,
            0x9e, 0x9f,
        ]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        before.sse.as_ref().unwrap().xmm[9]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8][..8],
        [0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8][8..],
        [0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_movapd_reverse_register_fixture_copies_literal_raw_xmm_and_preserves_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_34_14_movapd_reverse_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(state.rip, before.rip + 5);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9],
        [
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d,
            0x8e, 0x8f,
        ]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8],
        before.sse.as_ref().unwrap().xmm[8]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9][..8],
        [0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[9][8..],
        [0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().mxcsr,
        before.sse.as_ref().unwrap().mxcsr
    );
    assert_eq!(state.x87, before.x87);
    assert_eq!(state.gpr, before.gpr);
    assert_eq!(state.rflags, before.rflags);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_psubusw_register_uses_literal_lanes_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_9_psubusw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[8],
        [
            0, 0, 1, 0, 0, 0, 0xfe, 0xff, 0xfe, 0xff, 1, 0, 0, 0, 0xff, 0
        ]
    );
    let mut expected = before;
    expected.rip = fixture.stop_rip.unwrap().get();
    expected.sse.as_mut().unwrap().xmm[8] = [
        0, 0, 1, 0, 0, 0, 0xfe, 0xff, 0xfe, 0xff, 1, 0, 0, 0, 0xff, 0,
    ];
    assert_eq!(state, expected);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn psubusw_saturates_unsigned_words_independently_and_keeps_little_endian_lanes() {
    let mut state = LongModeState::default();
    state.rip = 0x1000;
    state.x87 = Some(od_isa::state::X87State::fixture_seed());
    state.sse = Some(od_isa::state::SseState {
        xmm: [[0; 16]; 16],
        mxcsr: 0x1f80,
    });
    let code = [0x66, 0x0f, 0xd9, 0xc1];
    let mut sim_memory = memory(&code, 0x1000);
    let lhs: [u16; 8] = [
        0x0000, 0x0002, 0x0001, 0xffff, 0xffff, 0x8000, 0x7fff, 0x0100,
    ];
    let rhs: [u16; 8] = [
        0x0001, 0x0001, 0x0002, 0x0001, 0x0001, 0x7fff, 0x8000, 0x0001,
    ];
    for lane in 0..8 {
        state.sse.as_mut().unwrap().xmm[0][lane * 2..lane * 2 + 2]
            .copy_from_slice(&lhs[lane].to_le_bytes());
        state.sse.as_mut().unwrap().xmm[1][lane * 2..lane * 2 + 2]
            .copy_from_slice(&rhs[lane].to_le_bytes());
    }
    assert_eq!(
        step(&mut state, &mut sim_memory, AccessContext::default()).unwrap(),
        StepOutcome::Advanced
    );
    assert_eq!(
        &state.sse.as_ref().unwrap().xmm[0],
        &[
            0, 0, 1, 0, 0, 0, 0xfe, 0xff, 0xfe, 0xff, 1, 0, 0, 0, 0xff, 0
        ]
    );
    assert_eq!(
        state.sse.as_ref().unwrap().xmm[1],
        [1, 0, 1, 0, 2, 0, 1, 0, 1, 0, 0xff, 0x7f, 0, 0x80, 1, 0]
    );
    assert_eq!(state.rflags, 2);

    // Equality and self-subtraction produce zero in each lane.
    let alias_code = [0x66, 0x0f, 0xd9, 0xc0];
    let mut alias_memory = memory(&alias_code, 0x2000);
    state.rip = 0x2000;
    assert_eq!(
        step(&mut state, &mut alias_memory, AccessContext::default()).unwrap(),
        StepOutcome::Advanced
    );
    assert_eq!(state.sse.as_ref().unwrap().xmm[0], [0; 16]);
}

#[test]
fn psubusw_exhausts_every_word_lhs_against_boundary_subtrahends() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_9_psubusw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let base = fixture.initial_state.rip.get();
    for lhs in 0..=u16::MAX {
        for rhs in [0, 1, 0x7fff, 0x8000, u16::MAX] {
            state.rip = base;
            let sse = state.sse.as_mut().unwrap();
            sse.xmm[8][0..2].copy_from_slice(&lhs.to_le_bytes());
            sse.xmm[9][0..2].copy_from_slice(&rhs.to_le_bytes());
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            assert_eq!(
                &state.sse.as_ref().unwrap().xmm[8][0..2],
                &lhs.saturating_sub(rhs).to_le_bytes(),
                "{lhs:04x}-{rhs:04x}"
            );
        }
    }
}

#[test]
fn psubusw_interpreter_covers_all_register_pairs_and_preserves_other_state() {
    for destination in 0..16u8 {
        for source in 0..16u8 {
            let rex = 0x40 | ((destination >> 3) << 2) | (source >> 3);
            let mut code = vec![0x66];
            if destination >= 8 || source >= 8 {
                code.push(rex);
            }
            code.extend([0x0f, 0xd9, 0xc0 | ((destination & 7) << 3) | (source & 7)]);
            let mut memory = memory(&code, 0x1000);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.x87 = Some(od_isa::state::X87State::fixture_seed());
            state.sse = Some(od_isa::state::SseState {
                xmm: [[0; 16]; 16],
                mxcsr: 0x1f80,
            });
            for register in 0..16 {
                for lane in 0..8 {
                    let lhs = (register as u16)
                        .wrapping_mul(1021)
                        .wrapping_add((lane as u16).wrapping_mul(8191));
                    let rhs = (register as u16)
                        .wrapping_mul(313)
                        .wrapping_add((lane as u16).wrapping_mul(12347));
                    state.sse.as_mut().unwrap().xmm[register][lane * 2..lane * 2 + 2]
                        .copy_from_slice(&lhs.to_le_bytes());
                    state.sse.as_mut().unwrap().xmm[register][lane * 2 + 1] = rhs.to_le_bytes()[1];
                }
            }
            state.rflags = 0x8d7 | (1 << 10);
            let before = state.clone();
            let memory_before = memory.snapshot();
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            let mut expected = before.clone();
            expected.rip += code.len() as u64;
            for lane in 0..8 {
                let a = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[destination as usize][lane * 2 + 1],
                ]);
                let b = u16::from_le_bytes([
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2],
                    before.sse.as_ref().unwrap().xmm[source as usize][lane * 2 + 1],
                ]);
                expected.sse.as_mut().unwrap().xmm[destination as usize][lane * 2..lane * 2 + 2]
                    .copy_from_slice(&a.saturating_sub(b).to_le_bytes());
            }
            assert_eq!(state, expected, "dest={destination} source={source}");
            assert_eq!(
                memory.snapshot(),
                memory_before,
                "data access for {destination},{source}"
            );
        }
    }
}

#[test]
fn psubusw_missing_state_truncation_stop_and_budget_are_atomic() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_9_psubusw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let code = [0x66, 0x0f, 0xd9, 0xc1];
    for missing_x87 in [true, false] {
        let mut candidate = state.clone();
        if missing_x87 {
            candidate.x87 = None;
        } else {
            candidate.sse = None;
        }
        let before = candidate.clone();
        let mut memory = memory(&code, fixture.initial_state.rip.get());
        let memory_before = memory.snapshot();
        assert!(matches!(
            step(&mut candidate, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd9 })
        ));
        assert_eq!(candidate, before);
        assert_eq!(memory.snapshot(), memory_before);
    }
    let mut truncated = memory(&code[..3], fixture.initial_state.rip.get());
    state.rip = fixture.initial_state.rip.get();
    let before = state.clone();
    let memory_before = truncated.snapshot();
    assert!(step(&mut state, &mut truncated, AccessContext::default()).is_err());
    assert_eq!(state, before);
    assert_eq!(truncated.snapshot(), memory_before);
    let mut candidate = before.clone();
    let mut budget_memory = memory(&code, candidate.rip);
    let result = run(
        &mut candidate,
        &mut budget_memory,
        RunConfig {
            instruction_budget: 0,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(candidate, before);
    let mut stopped = before.clone();
    let mut stop_memory = memory(&code, stopped.rip);
    let stopped_rip = stopped.rip;
    let result = run(
        &mut stopped,
        &mut stop_memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(stopped_rip),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(stopped, before);
}

#[test]
fn paddsb_literal_fixture_uses_signed_saturation_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_10_paddsb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        (before.sse.as_ref().unwrap().xmm[8][i] as i8)
            .saturating_add(before.sse.as_ref().unwrap().xmm[9][i] as i8) as u8
    });
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn psubsb_literal_fixture_uses_signed_saturation_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_11_psubsb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let expected: [u8; 16] = std::array::from_fn(|i| {
        (before.sse.as_ref().unwrap().xmm[8][i] as i8)
            .saturating_sub(before.sse.as_ref().unwrap().xmm[9][i] as i8) as u8
    });
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = expected;
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_packuswb_literal_fixture_uses_signed_words_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_33_packuswb_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = [
        0, 0, 0, 0, 1, 0x7f, 0x80, 0xff, 0xff, 0xff, 0, 0x80, 0, 0, 0, 1,
    ];
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn local_packssdw_literal_fixture_uses_signed_dwords_and_preserves_side_state() {
    let fixture = Fixture::from_json(include_bytes!(
        "../../../tests/fixtures/x86/stage2/s2_35_34_packssdw_register.case.json"
    ))
    .unwrap()
    .fixture;
    let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
    let before = state.clone();
    let mut memory = FlatMemory::from_fixture(&fixture).unwrap();
    let memory_before = memory.snapshot();
    let result = run(
        &mut state,
        &mut memory,
        RunConfig {
            instruction_budget: 1,
            stop_rip: fixture.stop_rip.map(HexU64::get),
            context: AccessContext::default(),
        },
        || false,
    );
    assert_eq!(result.exit, RunExit::StopAddress);
    assert_eq!(result.instructions_executed, 1);
    let mut expected_state = before.clone();
    expected_state.rip = fixture.stop_rip.unwrap().get();
    expected_state.sse.as_mut().unwrap().xmm[8] = [
        0xff, 0x7f, 0, 0x80, 0xff, 0x7f, 0, 0x80, 0xfe, 0x7f, 1, 0x80, 0xff, 0x7f, 0, 0x80,
    ];
    assert_eq!(state, expected_state);
    assert_eq!(memory.snapshot(), memory_before);
}

#[test]
fn movupd_memory_base_uses_one_data_callback_and_commits_only_after_success() {
    for (fixture_bytes, failed_access) in [
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_23_movupd_memory_base_load_r9_disp32.case.json").as_slice(), Some(AccessKind::Read)),
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_23_movupd_memory_base_store_r8_disp8.case.json").as_slice(), Some(AccessKind::Write)),
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_24_movupd_rip_load.case.json").as_slice(), Some(AccessKind::Read)),
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_24_movupd_rip_store.case.json").as_slice(), Some(AccessKind::Write)),
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_load_r9_r10_scale2.case.json").as_slice(), Some(AccessKind::Read)),
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_store_rax_r12_scale8.case.json").as_slice(), Some(AccessKind::Write)),
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_nobase_load.case.json").as_slice(), Some(AccessKind::Read)),
        (include_bytes!("../../../tests/fixtures/x86/stage2/s2_34_25_movupd_sib_nobase_store.case.json").as_slice(), Some(AccessKind::Write)),
    ] {
        let base = Fixture::from_json(fixture_bytes).unwrap().fixture;
        for fail in [false, true] {
            let fixture = base.clone();
            let mut state = fixture.initial_state.to_cpu_for(fixture.profile).unwrap();
            let before = state.clone();
            let mut inner = FlatMemory::from_fixture(&fixture).unwrap();
            if fail {
                inner.inject_once(
                    0x0020_0001,
                    failed_access.unwrap(),
                    MemoryError::Backend("injected MOVUPD memory-base failure".into()),
                );
            }
            let mut memory = AccessTraceMemory { inner, accesses: Vec::new(), fetch_limit: None };
            let result = step(&mut state, &mut memory, AccessContext::default());
            let data_accesses: Vec<_> = memory.accesses.iter().filter(|(kind, ..)| *kind == failed_access.unwrap()).collect();
            assert_eq!(data_accesses.len(), 1, "{} fail={fail}", fixture.case_id);
            assert_eq!(data_accesses[0].1, 0x0020_0001, "{} fail={fail}", fixture.case_id);
            assert_eq!(data_accesses[0].2, 16, "{} fail={fail}", fixture.case_id);
            if fail {
                assert!(matches!(result, Err(MemoryError::Backend(_))), "{result:?}");
                assert_eq!(state, before);
            } else {
                assert_eq!(result, Ok(StepOutcome::Advanced));
                assert_eq!(state.rip, fixture.stop_rip.unwrap().get());
            }
        }
    }
}

#[test]
fn cvtdq2ps_prerequisites_fetch_order_and_cpl_matrix_make_no_data_callbacks() {
    let code = [0x0f, 0x5b, 0xc1];
    let address = 0x9a00;
    for privilege in [
        PrivilegeLevel::Ring0,
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        for bits in 0u8..16 {
            let controls = Cvtdq2psControls {
                sse2_available: bits & 1 != 0,
                cr0_em: bits & 2 != 0,
                cr0_ts: bits & 4 != 0,
                cr4_osfxsr: bits & 8 != 0,
                cr4_osxmmexcpt: bits & 16 != 0,
            };
            let mut state = LongModeState::default();
            state.rip = address;
            let mut xmm = [[0x5a; 16]; 16];
            xmm[1] = [
                0xff, 0xff, 0xff, 0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x80,
            ];
            state.sse = Some(SseState { xmm, mxcsr: 0x1f80 });
            let before = state.clone();
            let mut trace = AccessTraceMemory {
                inner: memory(&code, address),
                accesses: Vec::new(),
                fetch_limit: None,
            };
            let result = run_with_cvtdq2ps_register_controls(
                &mut state,
                &mut trace,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(address + code.len() as u64),
                    context: AccessContext {
                        privilege,
                        ..AccessContext::default()
                    },
                },
                controls,
                || false,
            );
            let vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
                Some(6)
            } else if controls.cr0_ts {
                Some(7)
            } else {
                None
            };
            if let Some(vector) = vector {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector,
                        error_code: None,
                        linear_address: None
                    })
                );
                assert_eq!(state, before);
                assert_eq!(result.instructions_executed, 0);
            } else {
                assert_eq!(result.exit, RunExit::StopAddress);
                assert_eq!(result.instructions_executed, 1);
                assert_eq!(state.rip, address + 3);
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][..4],
                    0xbf80_0000u32.to_le_bytes()
                );
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][4..8],
                    0x4000_0000u32.to_le_bytes()
                );
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][8..12],
                    0u32.to_le_bytes()
                );
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][12..],
                    0xcf00_0000u32.to_le_bytes()
                );
            }
            assert!(
                trace
                    .accesses
                    .iter()
                    .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
            );
        }
    }

    // Even when a prerequisite would fault, the complete instruction fetch is first.
    let mut state = LongModeState::default();
    state.rip = address;
    state.sse = Some(SseState {
        xmm: [[0x3c; 16]; 16],
        mxcsr: 0x1f80,
    });
    let before = state.clone();
    let mut trace = AccessTraceMemory {
        inner: memory(&code, address),
        accesses: Vec::new(),
        fetch_limit: Some(address + 2),
    };
    let result = run_with_cvtdq2ps_register_controls(
        &mut state,
        &mut trace,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(address + 3),
            context: AccessContext::default(),
        },
        Cvtdq2psControls {
            sse2_available: false,
            cr0_em: false,
            cr0_ts: false,
            cr4_osfxsr: true,
            cr4_osxmmexcpt: true,
        },
        || false,
    );
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(state, before);
    assert_eq!(result.instructions_executed, 0);
    assert!(
        trace
            .accesses
            .iter()
            .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
    );
}

#[test]
fn cvtps2dq_prerequisites_fetch_order_cpl_stop_and_budget_make_no_data_callbacks() {
    let code = [0x66, 0x0f, 0x5b, 0xc1];
    let address = 0x9b00;
    for privilege in [
        PrivilegeLevel::Ring0,
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        for bits in 0u8..32 {
            let controls = Cvtps2dqControls {
                sse2_available: bits & 1 != 0,
                cr0_em: bits & 2 != 0,
                cr0_ts: bits & 4 != 0,
                cr4_osfxsr: bits & 8 != 0,
                cr4_osxmmexcpt: bits & 16 != 0,
            };
            let mut state = LongModeState::default();
            state.rip = address;
            let mut xmm = [[0x5a; 16]; 16];
            xmm[1] = [
                0x00, 0x00, 0xc0, 0x3f, // 1.5, valid and inexact
                0x00, 0x00, 0x00, 0x4f, // +2^31, invalid
                0x00, 0x00, 0x00, 0xcf, // exact -2^31
                0x01, 0x00, 0x00, 0x00, // subnormal, no DE
            ];
            state.sse = Some(SseState { xmm, mxcsr: 0x1f80 });
            let before = state.clone();
            let mut trace = AccessTraceMemory {
                inner: memory(&code, address),
                accesses: Vec::new(),
                fetch_limit: None,
            };
            let result = run_with_cvtps2dq_register_controls(
                &mut state,
                &mut trace,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(address + code.len() as u64),
                    context: AccessContext {
                        privilege,
                        ..AccessContext::default()
                    },
                },
                controls,
                || false,
            );
            let vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
                Some(6)
            } else if controls.cr0_ts {
                Some(7)
            } else {
                None
            };
            if let Some(vector) = vector {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector,
                        error_code: None,
                        linear_address: None,
                    })
                );
                assert_eq!(state, before);
                assert_eq!(result.instructions_executed, 0);
            } else {
                assert_eq!(result.exit, RunExit::StopAddress);
                assert_eq!(result.instructions_executed, 1);
                assert_eq!(state.rip, address + 4);
                assert_eq!(state.sse.as_ref().unwrap().xmm[0][..4], 2i32.to_le_bytes());
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][4..8],
                    i32::MIN.to_le_bytes()
                );
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][8..12],
                    i32::MIN.to_le_bytes()
                );
                assert_eq!(state.sse.as_ref().unwrap().mxcsr & 0x21, 0x21);
            }
            assert!(
                trace
                    .accesses
                    .iter()
                    .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
            );
        }
    }

    let controls = Cvtps2dqControls {
        sse2_available: false,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
        cr4_osxmmexcpt: true,
    };
    let mut state = LongModeState::default();
    state.rip = address;
    state.sse = Some(SseState {
        xmm: [[0x3c; 16]; 16],
        mxcsr: 0x1f80,
    });
    let before = state.clone();
    let mut truncated = AccessTraceMemory {
        inner: memory(&code, address),
        accesses: Vec::new(),
        fetch_limit: Some(address + 3),
    };
    let result = run_with_cvtps2dq_register_controls(
        &mut state,
        &mut truncated,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(address + 4),
            context: AccessContext::default(),
        },
        controls,
        || false,
    );
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(state, before);
    assert_eq!(result.instructions_executed, 0);
    assert!(
        truncated
            .accesses
            .iter()
            .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
    );

    for (budget, stop, expected) in [
        (0, false, RunExit::InstructionBudgetExhausted),
        (1, true, RunExit::StopRequested),
    ] {
        let mut state = before.clone();
        let mut trace = AccessTraceMemory {
            inner: memory(&code, address),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run_with_cvtps2dq_register_controls(
            &mut state,
            &mut trace,
            RunConfig {
                instruction_budget: budget,
                stop_rip: Some(address + 4),
                context: AccessContext::default(),
            },
            Cvtps2dqControls {
                sse2_available: false,
                ..controls
            },
            || stop,
        );
        assert_eq!(result.exit, expected);
        assert_eq!(state, before);
        assert!(trace.accesses.is_empty());
    }
}

#[test]
fn cvttps2dq_prerequisites_fetch_order_cpl_stop_and_budget_make_no_data_callbacks() {
    let code = [0xf3, 0x0f, 0x5b, 0xc1];
    let address = 0x9b00;
    for privilege in [
        PrivilegeLevel::Ring0,
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        for bits in 0u8..32 {
            let controls = Cvttps2dqControls {
                sse2_available: bits & 1 != 0,
                cr0_em: bits & 2 != 0,
                cr0_ts: bits & 4 != 0,
                cr4_osfxsr: bits & 8 != 0,
                cr4_osxmmexcpt: bits & 16 != 0,
            };
            let mut state = LongModeState::default();
            state.rip = address;
            let mut xmm = [[0x5a; 16]; 16];
            xmm[1] = [
                0x00, 0x00, 0xc0, 0x3f, // 1.5, valid and inexact
                0x00, 0x00, 0x00, 0x4f, // +2^31, invalid
                0x00, 0x00, 0x00, 0xcf, // exact -2^31
                0x01, 0x00, 0x00, 0x00, // subnormal, no DE
            ];
            state.sse = Some(SseState { xmm, mxcsr: 0x1f80 });
            let before = state.clone();
            let mut trace = AccessTraceMemory {
                inner: memory(&code, address),
                accesses: Vec::new(),
                fetch_limit: None,
            };
            let result = run_with_cvttps2dq_register_controls(
                &mut state,
                &mut trace,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(address + code.len() as u64),
                    context: AccessContext {
                        privilege,
                        ..AccessContext::default()
                    },
                },
                controls,
                || false,
            );
            let vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
                Some(6)
            } else if controls.cr0_ts {
                Some(7)
            } else {
                None
            };
            if let Some(vector) = vector {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector,
                        error_code: None,
                        linear_address: None,
                    })
                );
                assert_eq!(state, before);
                assert_eq!(result.instructions_executed, 0);
            } else {
                assert_eq!(result.exit, RunExit::StopAddress);
                assert_eq!(result.instructions_executed, 1);
                assert_eq!(state.rip, address + 4);
                assert_eq!(state.sse.as_ref().unwrap().xmm[0][..4], 1i32.to_le_bytes());
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][4..8],
                    i32::MIN.to_le_bytes()
                );
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][8..12],
                    i32::MIN.to_le_bytes()
                );
                assert_eq!(state.sse.as_ref().unwrap().mxcsr & 0x21, 0x21);
            }
            assert!(
                trace
                    .accesses
                    .iter()
                    .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
            );
        }
    }

    let controls = Cvttps2dqControls {
        sse2_available: false,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
        cr4_osxmmexcpt: true,
    };
    let mut state = LongModeState::default();
    state.rip = address;
    state.sse = Some(SseState {
        xmm: [[0x3c; 16]; 16],
        mxcsr: 0x1f80,
    });
    let before = state.clone();
    let mut truncated = AccessTraceMemory {
        inner: memory(&code, address),
        accesses: Vec::new(),
        fetch_limit: Some(address + 3),
    };
    let result = run_with_cvttps2dq_register_controls(
        &mut state,
        &mut truncated,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(address + 4),
            context: AccessContext::default(),
        },
        controls,
        || false,
    );
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(state, before);
    assert_eq!(result.instructions_executed, 0);
    assert!(
        truncated
            .accesses
            .iter()
            .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
    );

    for (budget, stop, expected) in [
        (0, false, RunExit::InstructionBudgetExhausted),
        (1, true, RunExit::StopRequested),
    ] {
        let mut state = before.clone();
        let mut trace = AccessTraceMemory {
            inner: memory(&code, address),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run_with_cvttps2dq_register_controls(
            &mut state,
            &mut trace,
            RunConfig {
                instruction_budget: budget,
                stop_rip: Some(address + 4),
                context: AccessContext::default(),
            },
            Cvttps2dqControls {
                sse2_available: false,
                ..controls
            },
            || stop,
        );
        assert_eq!(result.exit, expected);
        assert_eq!(state, before);
        assert!(trace.accesses.is_empty());
    }
}

#[test]
fn cvtpd2dq_prerequisites_fetch_order_cpl_stop_and_budget_make_no_data_callbacks() {
    let code = [0xf2, 0x0f, 0xe6, 0xc1];
    let address = 0x9b80;
    let controls = Cvtpd2dqControls {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
        cr4_osxmmexcpt: true,
    };
    for privilege in [
        PrivilegeLevel::Ring0,
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        for bits in 0u8..32 {
            let controls = Cvtpd2dqControls {
                sse2_available: bits & 1 != 0,
                cr0_em: bits & 2 != 0,
                cr0_ts: bits & 4 != 0,
                cr4_osfxsr: bits & 8 != 0,
                cr4_osxmmexcpt: bits & 16 != 0,
            };
            let mut state = LongModeState::default();
            state.rip = address;
            let mut xmm = [[0x5a; 16]; 16];
            xmm[1] = [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0x3f, // 1.5
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x41, // +2^31
            ];
            state.sse = Some(SseState { xmm, mxcsr: 0x1f80 });
            let before = state.clone();
            let mut trace = AccessTraceMemory {
                inner: memory(&code, address),
                accesses: Vec::new(),
                fetch_limit: None,
            };
            let result = run_with_cvtpd2dq_register_controls(
                &mut state,
                &mut trace,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(address + code.len() as u64),
                    context: AccessContext {
                        privilege,
                        ..AccessContext::default()
                    },
                },
                controls,
                || false,
            );
            let vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
                Some(6)
            } else if controls.cr0_ts {
                Some(7)
            } else {
                None
            };
            if let Some(vector) = vector {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector,
                        error_code: None,
                        linear_address: None,
                    })
                );
                assert_eq!(state, before);
                assert_eq!(result.instructions_executed, 0);
            } else {
                assert_eq!(result.exit, RunExit::StopAddress);
                assert_eq!(result.instructions_executed, 1);
                assert_eq!(state.rip, address + 4);
                assert_eq!(state.sse.as_ref().unwrap().xmm[0][..4], 2i32.to_le_bytes());
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][4..8],
                    i32::MIN.to_le_bytes()
                );
                assert_eq!(state.sse.as_ref().unwrap().xmm[0][8..], [0; 8]);
                assert_eq!(state.sse.as_ref().unwrap().mxcsr & 0x21, 0x21);
            }
            assert!(
                trace
                    .accesses
                    .iter()
                    .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
            );
        }
    }

    let mut state = LongModeState::default();
    state.rip = address;
    state.sse = Some(SseState {
        xmm: [[0x3c; 16]; 16],
        mxcsr: 0x1f80,
    });
    let before = state.clone();
    let mut truncated = AccessTraceMemory {
        inner: memory(&code, address),
        accesses: Vec::new(),
        fetch_limit: Some(address + 3),
    };
    let result = run_with_cvtpd2dq_register_controls(
        &mut state,
        &mut truncated,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(address + 4),
            context: AccessContext::default(),
        },
        Cvtpd2dqControls {
            sse2_available: false,
            ..controls
        },
        || false,
    );
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(state, before);
    assert_eq!(result.instructions_executed, 0);
    assert!(
        truncated
            .accesses
            .iter()
            .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
    );

    for (budget, stop, expected) in [
        (0, false, RunExit::InstructionBudgetExhausted),
        (1, true, RunExit::StopRequested),
    ] {
        let mut state = before.clone();
        let mut trace = AccessTraceMemory {
            inner: memory(&code, address),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run_with_cvtpd2dq_register_controls(
            &mut state,
            &mut trace,
            RunConfig {
                instruction_budget: budget,
                stop_rip: Some(address + 4),
                context: AccessContext::default(),
            },
            Cvtpd2dqControls {
                sse2_available: false,
                ..controls
            },
            || stop,
        );
        assert_eq!(result.exit, expected);
        assert_eq!(state, before);
        assert!(trace.accesses.is_empty());
    }
}

#[test]
fn cvttpd2dq_prerequisites_fetch_order_cpl_stop_and_budget_make_no_data_callbacks() {
    let code = [0x66, 0x0f, 0xe6, 0xc1];
    let address = 0x9b80;
    let controls = Cvttpd2dqControls {
        sse2_available: true,
        cr0_em: false,
        cr0_ts: false,
        cr4_osfxsr: true,
        cr4_osxmmexcpt: true,
    };
    for privilege in [
        PrivilegeLevel::Ring0,
        PrivilegeLevel::Ring1,
        PrivilegeLevel::Ring2,
        PrivilegeLevel::Ring3,
    ] {
        for bits in 0u8..32 {
            let controls = Cvttpd2dqControls {
                sse2_available: bits & 1 != 0,
                cr0_em: bits & 2 != 0,
                cr0_ts: bits & 4 != 0,
                cr4_osfxsr: bits & 8 != 0,
                cr4_osxmmexcpt: bits & 16 != 0,
            };
            let mut state = LongModeState::default();
            state.rip = address;
            let mut xmm = [[0x5a; 16]; 16];
            xmm[1] = [
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0x3f, // 1.5
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x41, // +2^31
            ];
            state.sse = Some(SseState { xmm, mxcsr: 0x1f80 });
            let before = state.clone();
            let mut trace = AccessTraceMemory {
                inner: memory(&code, address),
                accesses: Vec::new(),
                fetch_limit: None,
            };
            let result = run_with_cvttpd2dq_register_controls(
                &mut state,
                &mut trace,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(address + code.len() as u64),
                    context: AccessContext {
                        privilege,
                        ..AccessContext::default()
                    },
                },
                controls,
                || false,
            );
            let vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
                Some(6)
            } else if controls.cr0_ts {
                Some(7)
            } else {
                None
            };
            if let Some(vector) = vector {
                assert_eq!(
                    result.exit,
                    RunExit::Fault(ArchitecturalFault {
                        vector,
                        error_code: None,
                        linear_address: None,
                    })
                );
                assert_eq!(state, before);
                assert_eq!(result.instructions_executed, 0);
            } else {
                assert_eq!(result.exit, RunExit::StopAddress);
                assert_eq!(result.instructions_executed, 1);
                assert_eq!(state.rip, address + 4);
                assert_eq!(state.sse.as_ref().unwrap().xmm[0][..4], 1i32.to_le_bytes());
                assert_eq!(
                    state.sse.as_ref().unwrap().xmm[0][4..8],
                    i32::MIN.to_le_bytes()
                );
                assert_eq!(state.sse.as_ref().unwrap().xmm[0][8..], [0; 8]);
                assert_eq!(state.sse.as_ref().unwrap().mxcsr & 0x21, 0x21);
            }
            assert!(
                trace
                    .accesses
                    .iter()
                    .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
            );
        }
    }

    let mut state = LongModeState::default();
    state.rip = address;
    state.sse = Some(SseState {
        xmm: [[0x3c; 16]; 16],
        mxcsr: 0x1f80,
    });
    let before = state.clone();
    let mut truncated = AccessTraceMemory {
        inner: memory(&code, address),
        accesses: Vec::new(),
        fetch_limit: Some(address + 3),
    };
    let result = run_with_cvttpd2dq_register_controls(
        &mut state,
        &mut truncated,
        RunConfig {
            instruction_budget: 1,
            stop_rip: Some(address + 4),
            context: AccessContext::default(),
        },
        Cvttpd2dqControls {
            sse2_available: false,
            ..controls
        },
        || false,
    );
    assert!(matches!(
        result.exit,
        RunExit::MemoryError(MemoryError::Unmapped {
            access: AccessKind::Fetch,
            ..
        })
    ));
    assert_eq!(state, before);
    assert_eq!(result.instructions_executed, 0);
    assert!(
        truncated
            .accesses
            .iter()
            .all(|(kind, _, _, _)| *kind == AccessKind::Fetch)
    );

    for (budget, stop, expected) in [
        (0, false, RunExit::InstructionBudgetExhausted),
        (1, true, RunExit::StopRequested),
    ] {
        let mut state = before.clone();
        let mut trace = AccessTraceMemory {
            inner: memory(&code, address),
            accesses: Vec::new(),
            fetch_limit: None,
        };
        let result = run_with_cvttpd2dq_register_controls(
            &mut state,
            &mut trace,
            RunConfig {
                instruction_budget: budget,
                stop_rip: Some(address + 4),
                context: AccessContext::default(),
            },
            Cvttpd2dqControls {
                sse2_available: false,
                ..controls
            },
            || stop,
        );
        assert_eq!(result.exit, expected);
        assert_eq!(state, before);
        assert!(trace.accesses.is_empty());
    }
}
