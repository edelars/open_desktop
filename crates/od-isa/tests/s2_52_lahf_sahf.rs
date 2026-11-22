//! S2-52: LAHF (`9F`) and SAHF (`9E`) in 64-bit mode.
//!
//! Every expected value comes from the Intel SDM text of LAHF (Vol. 2A, p. 3-532) and SAHF (Vol. 2B,
//! pp. 4-601..4-602), combined SDM order numbers 253665/325383/325384/335592, read for this task:
//!
//! * LAHF: AH := RFLAGS(SF:ZF:0:AF:0:PF:1:CF), i.e. bits 7, 6, 4, 2, 0 of AH are SF, ZF, AF, PF, CF,
//!   bit 1 reads 1 and bits 5 and 3 read 0, whatever RFLAGS holds in its reserved bits. No flag is
//!   affected.
//! * SAHF: SF, ZF, AF, PF, CF := AH bits 7, 6, 4, 2, 0; AH bits 1, 3, 5 are ignored and the
//!   RFLAGS reserved bits 1, 3, 5 are unaffected (they keep the value they had). OF, DF, IF and the
//!   other flags are not touched.
//! * 64-bit mode: #UD if the LOCK prefix is used. AH is implicit (no ModRM), so a REX prefix does
//!   not redirect it to SPL.
//!
//! The SDM also lists #UD in 64-bit mode when CPUID.80000001H:ECX.LAHF_SAHF_64 = 0. The interpreter
//! does not gate on that bit (it is declared by S2-57 only), so no case below covers it.
//! The hardware oracle for these tables is work/stage8/s2_52_53_oracle (2072 cases, 2009 matched
//! byte for byte; the 63 others are forms this slice refuses as Unsupported, listed below).

use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cr0Profile, Cr4Profile,
    RunConfig, RunExit, StepOutcome, run_with_cpl0_profile_set, run_with_rflags_defined,
};
use od_isa::{
    AccessContext, AccessKind, ArchitecturalFault, CpuActivity, GuestMemory, LongModeState,
    MemoryError, PrivilegeLevel, Register, StiShadow,
};

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const DF: u64 = 1 << 10;
const IF: u64 = 1 << 9;
/// The five flags LAHF reads and SAHF loads.
const STATUS: u64 = CF | PF | AF | ZF | SF;
/// The reserved bits 1, 3, 5.
const RESERVED: u64 = 0x2 | 0x8 | 0x20;

const CODE: u64 = 0x2000;

fn ring0() -> AccessContext {
    AccessContext {
        privilege: PrivilegeLevel::Ring0,
        ..AccessContext::default()
    }
}

/// Code only; every data access is an error and is counted (LAHF and SAHF touch no memory).
struct Ram {
    bytes: Vec<u8>,
    data_accesses: usize,
}

impl Ram {
    fn with_code(code: &[u8]) -> Self {
        let mut bytes = vec![0; 0x4000];
        bytes[CODE as usize..CODE as usize + code.len()].copy_from_slice(code);
        Self {
            bytes,
            data_accesses: 0,
        }
    }
}

impl GuestMemory for Ram {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(address as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        self.data_accesses += 1;
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        self.data_accesses += 1;
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// Distinct non-zero values in every register so that an unwanted write is visible.
fn state(rflags: u64, rax: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags,
        ..LongModeState::default()
    };
    for (index, slot) in state.gpr.iter_mut().enumerate() {
        *slot = 0x0101_0101_0101_0101 * (index as u64 + 0x11);
    }
    state.gpr[Register::Rax as usize] = rax;
    state
}

fn step(code: &[u8], cpu: &mut LongModeState) -> (Result<StepOutcome, MemoryError>, Ram) {
    let mut ram = Ram::with_code(code);
    let result = execute::step(cpu, &mut ram, ring0());
    (result, ram)
}

fn ah(cpu: &LongModeState) -> u8 {
    (cpu.gpr[Register::Rax as usize] >> 8) as u8
}

/// Flag bits of "combination" (bit 0 CF, 1 PF, 2 AF, 3 ZF, 4 SF).
fn combination(index: u64) -> u64 {
    [CF, PF, AF, ZF, SF]
        .iter()
        .enumerate()
        .filter(|(bit, _)| index >> bit & 1 == 1)
        .fold(0, |flags, (_, mask)| flags | mask)
}

const REX_VARIANTS: [&[u8]; 6] = [&[], &[0x40], &[0x41], &[0x44], &[0x48], &[0x4f]];

fn with_prefix(prefix: &[u8], opcode: u8) -> Vec<u8> {
    let mut code = prefix.to_vec();
    code.push(opcode);
    code
}

// ---- LAHF: the table of all 32 SF/ZF/AF/PF/CF combinations ----

#[test]
fn lahf_loads_all_32_status_combinations_with_bit1_set_and_bits_3_and_5_clear() {
    let untouched_flags = [0, OF, DF, IF, OF | DF | IF];
    let reserved = [0, 0x2, 0x8, 0x20, 0x8 | 0x20, RESERVED];
    for index in 0..32 {
        let status = combination(index);
        for extra in untouched_flags {
            for reserved_bits in reserved {
                for rax in [0u64, u64::MAX, 0x1234_5678_9abc_def0] {
                    let rflags = status | extra | reserved_bits;
                    let mut cpu = state(rflags, rax);
                    let before = cpu.clone();
                    let (result, ram) = step(&[0x9f], &mut cpu);
                    assert_eq!(result, Ok(StepOutcome::Advanced), "{rflags:#x}");
                    // AH := SF:ZF:0:AF:0:PF:1:CF; the same positions as in the low flags byte.
                    let expected_ah = (status as u8) | 0x02;
                    assert_eq!(ah(&cpu), expected_ah, "{rflags:#x}");
                    // Only bits 15:8 of RAX change.
                    assert_eq!(
                        cpu.gpr[Register::Rax as usize],
                        (rax & !0xff00) | (u64::from(expected_ah) << 8),
                        "{rflags:#x}"
                    );
                    // "Flags Affected: None": RFLAGS, every other register and RIP+1.
                    assert_eq!(cpu.rflags, before.rflags, "{rflags:#x}");
                    assert_eq!(cpu.rip, CODE + 1);
                    let mut expected = before.clone();
                    expected.gpr[Register::Rax as usize] = cpu.gpr[Register::Rax as usize];
                    expected.rip = CODE + 1;
                    assert_eq!(cpu, expected, "{rflags:#x}");
                    assert_eq!(ram.data_accesses, 0);
                }
            }
        }
    }
}

// ---- SAHF: all 256 AH values ----

#[test]
fn sahf_loads_the_five_status_flags_from_every_ah_value_and_keeps_all_other_bits() {
    let initial_flags = [
        0x2,
        0x2 | STATUS,
        0x2 | STATUS | OF | DF | IF,
        0x2 | OF | DF | IF,
        // Reserved bits that are not in their usual state stay as they are ("unaffected").
        0x8 | 0x20,
        0x8 | 0x20 | STATUS,
        0,
    ];
    for ah_value in 0..=255u64 {
        for initial in initial_flags {
            for low in [0u64, 0xffff_ffff_ffff_00ff, 0x0123_4567_89ab_cdef] {
                let rax = (low & !0xff00) | (ah_value << 8);
                let mut cpu = state(initial, rax);
                let before = cpu.clone();
                let (result, ram) = step(&[0x9e], &mut cpu);
                assert_eq!(result, Ok(StepOutcome::Advanced), "{ah_value:#x}");
                // SF:ZF:AF:PF:CF := AH(7, 6, 4, 2, 0); AH bits 1, 3, 5 are ignored.
                let expected = (initial & !STATUS) | (ah_value & STATUS);
                assert_eq!(
                    cpu.rflags, expected,
                    "ah={ah_value:#x} initial={initial:#x}"
                );
                // Bits 1, 3, 5, OF, DF, IF and the rest keep their value.
                assert_eq!(
                    cpu.rflags & !STATUS,
                    initial & !STATUS,
                    "ah={ah_value:#x} initial={initial:#x}"
                );
                // RAX (AH included) and every other register are untouched; RIP+1.
                let mut expected_cpu = before.clone();
                expected_cpu.rflags = expected;
                expected_cpu.rip = CODE + 1;
                assert_eq!(cpu, expected_cpu, "ah={ah_value:#x}");
                assert_eq!(ram.data_accesses, 0);
            }
        }
    }
}

#[test]
fn lahf_then_sahf_round_trips_the_status_flags_and_the_reserved_bit_stays_set() {
    for index in 0..32 {
        let status = combination(index);
        let mut cpu = state(0x2 | status | OF, 0);
        let mut ram = Ram::with_code(&[0x9f, 0x9e]);
        execute::step(&mut cpu, &mut ram, ring0()).unwrap();
        cpu.rflags = 0x2 | OF; // wipe the status flags, keep an unrelated one
        assert_eq!(
            execute::step(&mut cpu, &mut ram, ring0()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(cpu.rflags, 0x2 | OF | status, "{index:#x}");
    }
}

// ---- REX does not redirect the implicit AH ----

#[test]
fn rex_prefixes_do_not_turn_the_implicit_ah_into_spl_or_change_the_result() {
    for prefix in REX_VARIANTS {
        // LAHF: AH is written, SPL (the byte register encoding 4 under REX) is not.
        let mut cpu = state(0x2 | SF | PF | OF, 0x5555_5555_5555_5555);
        let before = cpu.clone();
        let code = with_prefix(prefix, 0x9f);
        let (result, _) = step(&code, &mut cpu);
        assert_eq!(result, Ok(StepOutcome::Advanced), "{prefix:02x?}");
        assert_eq!(ah(&cpu), 0x80 | 0x04 | 0x02, "{prefix:02x?}");
        assert_eq!(
            cpu.gpr[Register::Rsp as usize],
            before.gpr[Register::Rsp as usize],
            "{prefix:02x?}"
        );
        assert_eq!(cpu.rip, CODE + code.len() as u64);
        let mut expected = before.clone();
        expected.gpr[Register::Rax as usize] = 0x5555_5555_5555_8655;
        expected.rip = CODE + code.len() as u64;
        assert_eq!(cpu, expected, "{prefix:02x?}");

        // SAHF: reads AH (0xd5 = every status flag) and not SPL, which holds 0x00 here.
        let mut cpu = state(0x2, 0xffff_ffff_ffff_d5ff);
        cpu.gpr[Register::Rsp as usize] = 0x3f00;
        let before = cpu.clone();
        let code = with_prefix(prefix, 0x9e);
        let (result, _) = step(&code, &mut cpu);
        assert_eq!(result, Ok(StepOutcome::Advanced), "{prefix:02x?}");
        assert_eq!(cpu.rflags, 0x2 | STATUS, "{prefix:02x?}");
        let mut expected = before.clone();
        expected.rflags = 0x2 | STATUS;
        expected.rip = CODE + code.len() as u64;
        assert_eq!(cpu, expected, "{prefix:02x?}");
    }
}

#[test]
fn operand_size_address_size_segment_and_stale_rex_prefixes_change_nothing() {
    // The oracle shows the hardware ignores them; they are decoded into the same instruction.
    let prefixes: &[&[u8]] = &[
        &[0x66],
        &[0x67],
        &[0x2e],
        &[0x64],
        &[0x65],
        &[0x66, 0x48],
        &[0x48, 0x66],
        &[0x4f, 0x66],
        &[0x40, 0x40],
        &[0x40, 0x48],
    ];
    for prefix in prefixes {
        for (opcode, rax, flags, want_flags, want_ah) in [
            (0x9fu8, 0u64, 0x2 | ZF | CF, 0x2 | ZF | CF, Some(0x43u8)),
            (0x9e, 0x8100, 0x2 | OF, 0x2 | OF | SF | CF, None),
        ] {
            let mut cpu = state(flags, rax);
            let code = with_prefix(prefix, opcode);
            let (result, ram) = step(&code, &mut cpu);
            assert_eq!(result, Ok(StepOutcome::Advanced), "{prefix:02x?}");
            assert_eq!(cpu.rflags, want_flags, "{prefix:02x?}");
            if let Some(want) = want_ah {
                assert_eq!(ah(&cpu), want, "{prefix:02x?}");
            }
            assert_eq!(cpu.rip, CODE + code.len() as u64, "{prefix:02x?}");
            assert_eq!(ram.data_accesses, 0);
        }
    }
}

// ---- LOCK is #UD (SDM 64-bit mode exceptions); REP/REPNE are not modeled ----

#[test]
fn lock_is_ud_and_changes_nothing() {
    let ud = StepOutcome::Fault(ArchitecturalFault {
        vector: 6,
        error_code: None,
        linear_address: None,
    });
    for opcode in [0x9fu8, 0x9e] {
        for code in [
            vec![0xf0, opcode],
            vec![0xf0, 0x48, opcode],
            vec![0x48, 0xf0, opcode],
            vec![0x66, 0xf0, opcode],
        ] {
            let mut cpu = state(0x2 | STATUS | OF, 0xaaaa_aaaa_aaaa_aaaa);
            let before = cpu.clone();
            let (result, ram) = step(&code, &mut cpu);
            assert_eq!(result, Ok(ud.clone()), "{code:02x?}");
            assert_eq!(cpu, before, "{code:02x?}");
            assert_eq!(ram.data_accesses, 0);
        }
    }
}

#[test]
fn rep_and_repne_prefixes_stay_unsupported_without_change() {
    for opcode in [0x9fu8, 0x9e] {
        for code in [
            vec![0xf3, opcode],
            vec![0xf2, opcode],
            vec![0xf3, 0x48, opcode],
            vec![0xf0, 0xf3, opcode],
        ] {
            let mut cpu = state(0x2 | STATUS, 0xaaaa_aaaa_aaaa_aaaa);
            let before = cpu.clone();
            let (result, _) = step(&code, &mut cpu);
            assert_eq!(
                result,
                Ok(StepOutcome::Unsupported { opcode }),
                "{code:02x?}"
            );
            assert_eq!(cpu, before, "{code:02x?}");
        }
    }
}

// ---- flag definedness, as for the existing flag-writing integer instructions ----

#[test]
fn sahf_defines_the_five_status_flags_and_lahf_changes_no_definedness() {
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ring0(),
    };
    // Only the always-defined bit 1 is defined at the start (as after a reset of the tracker).
    let mut cpu = state(0x2, 0xd500);
    let mut ram = Ram::with_code(&[0x9e]);
    let result = run_with_rflags_defined(&mut cpu, &mut ram, config, 0x02, || false).unwrap();
    assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
    assert_eq!(result.rflags_defined, 0x02 | STATUS);

    // LAHF only reads: a partially defined tracker is returned unchanged (OF, DF stay as before).
    for initial in [0x02, 0x02 | OF, 0x02 | STATUS | DF] {
        let mut cpu = state(0x2, 0);
        let mut ram = Ram::with_code(&[0x9f]);
        let result =
            run_with_rflags_defined(&mut cpu, &mut ram, config, initial, || false).unwrap();
        assert_eq!(result.rflags_defined, initial);
    }
}

// ---- the CPL0 profile set (Ring0) executes both forms; the bare run is the same ----

fn cpl0_state(rflags: u64, rax: u64) -> LongModeState {
    LongModeState {
        cr0: Some(Cr0Profile::BASE),
        cr4: Some(Cr4Profile::BASE),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        rip: CODE,
        rflags,
        gpr: {
            let mut gpr = [0; 16];
            gpr[Register::Rax as usize] = rax;
            gpr
        },
        ..LongModeState::default()
    }
}

#[test]
fn the_cpl0_profile_set_executes_both_forms_at_ring0() {
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let config = RunConfig {
        instruction_budget: 1,
        stop_rip: None,
        context: ring0(),
    };
    let run = |code: &[u8], cpu: &mut LongModeState| {
        let mut ram = Ram::with_code(code);
        let result = run_with_cpl0_profile_set(
            cpu,
            &mut ram,
            config,
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || false,
        )
        .unwrap();
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted),
            "{code:02x?}"
        );
        assert_eq!(result.instructions_executed, 1);
    };
    let mut cpu = cpl0_state(0x2 | SF | AF | OF, 0x77);
    run(&[0x9f], &mut cpu);
    assert_eq!(cpu.gpr[Register::Rax as usize], 0x77 | (0x92 << 8));
    assert_eq!(cpu.rflags, 0x2 | SF | AF | OF);
    assert_eq!(cpu.rip, CODE + 1);

    let mut cpu = cpl0_state(0x2 | OF, 0x4100);
    run(&[0x9e], &mut cpu);
    // AH = 0x41: ZF and CF.
    assert_eq!(cpu.rflags, 0x2 | OF | ZF | CF);
    assert_eq!(cpu.rip, CODE + 1);
}
