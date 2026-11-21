//! S2-33: the MMX register model (MM0..MM7 are the low 64 bits of the physical x87 data registers R0..R7), the
//! effect of an MMX instruction and of EMMS on the x87 TOP, tag word and exponent fields, the transition
//! between MMX and x87 code, and the gates that precede every MMX instruction.
//!
//! Sources: Intel SDM combined 325462-093US (cdrdv2.intel.com/v1/dl/getContent/671200, through pdftotext):
//! Vol. 1 §9.2.2 "MMX Registers" (aliased to the FPU data register stack R0..R7), §9.5.1 and §9.6.2 (after
//! each MMX instruction other than EMMS TOS is 0, every tag is valid, and the exponent field and sign of a
//! register that the instruction writes are all ones; "any x87 FPU state prior to the execution of the MMX
//! instruction is essentially lost"), §9.6.3 (EMMS); Vol. 2A p. 3-282 EMMS ("x87FPUTagWord := FFFFH"; #UD for
//! CR0.EM = 1 or LOCK, #NM for CR0.TS = 1, #MF for a pending FPU exception); Vol. 3A §15.2 and Table 15-2 (MMX
//! reads and writes set every tag valid and TOS to 000B, other x87 state unchanged, bits 64..79 of a written
//! register set to ones; EMMS sets every tag empty and TOS to 000B); Vol. 2A Table 2-21 (Exceptions Type 4:
//! CR4.OSFXSR is listed for "Legacy SSE instruction" only, so the MMX forms do not depend on it). The
//! physical oracle (Core i5-3210M, 7483 cases, see the stage 8 report) confirms Table 15-2, including TOS = 0
//! after EMMS, and gives #MF for a pending unmasked exception on every MMX form and on EMMS.
//!
//! Not asserted here because the SDM text does not define it or the model does not carry it: the CPUID.MMX
//! gate (S2-57), the effect of FXSAVE/FNSAVE images (the FXSAVE forms of the model do not take the MMX
//! state yet, only the shared register file; the oracle compares the FXSAVE image byte for byte), CR0.NE = 0
//! with a pending exception (FERR#, refused as Unsupported).

mod mmx_common;

use mmx_common::*;
use od_isa::execute::{RunExit, StepOutcome};
use od_isa::long_decode::{self, Operation};
use od_isa::state::X87State;
use od_isa::{AccessContext, LongModeState, PrivilegeLevel, Register};

const EMMS: [u8; 2] = [0x0f, 0x77];
/// FISTTP dword [rax] (`DB /1`), the x87 instruction of the transition tests (S2-54 slice C).
const FISTTP_DWORD: [u8; 2] = [0xdb, 0x08];

fn abs_bytes(value: u64) -> u64 {
    u64::from_le_bytes(value.to_le_bytes().map(|byte| (byte as i8).unsigned_abs()))
}

fn run_one(
    code: &[u8],
    cpu: &mut LongModeState,
) -> (Result<StepOutcome, od_isa::MemoryError>, Ram) {
    let mut ram = Ram::new(code);
    let outcome = step(cpu, &mut ram, PrivilegeLevel::Ring0);
    (outcome, ram)
}

// ---- the register view ----

#[test]
fn the_mmx_view_is_the_low_quadword_of_the_x87_data_register_with_no_second_copy() {
    let mut x87 = x87_seed();
    for index in 0..8u8 {
        let raw = x87.registers[usize::from(index)];
        assert_eq!(x87.mmx_register(index), Some(raw[..8].try_into().unwrap()));
    }
    assert_eq!(x87.mmx_register(8), None);
    let before = x87.clone();
    assert!(!x87.write_mmx_register(8, [0; 8]));
    assert_eq!(x87, before, "an index above 7 changes nothing");
    // A write to MM3 is visible in physical register 3 at once, with all ones in bits 79:64; no other
    // register, tag or control field is touched by the register view itself.
    assert!(x87.write_mmx_register(3, [1, 2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(x87.registers[3], [1, 2, 3, 4, 5, 6, 7, 8, 0xff, 0xff]);
    for index in [0, 1, 2, 4, 5, 6, 7] {
        assert_eq!(x87.registers[index], before.registers[index]);
    }
    assert_eq!(
        (x87.status_word, x87.tag_word, x87.control_word),
        (before.status_word, before.tag_word, before.control_word)
    );
    // The reverse direction: an x87 write to the data register (its significand) is the MMX value.
    x87.registers[5][..8].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
    assert_eq!(
        x87.mmx_register(5),
        Some(0x1122_3344_5566_7788u64.to_le_bytes())
    );
}

#[test]
fn an_mmx_instruction_sets_top_to_zero_all_tags_valid_and_the_exponent_ones_of_the_destination_only()
 {
    // PABSB mm3, mm6 on the x87 seed (TOP = 5, tags 0xa5a5, cw 0x027f, sw 0x2d41, pointers set).
    let code = reg_code(kind_named("pabsb"), 3, 6, 0);
    let mut cpu = cpu();
    let before = cpu.clone();
    let source = mm(&cpu, 6);
    let (outcome, ram) = run_one(&code, &mut cpu);
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    let mut expected = before.clone();
    {
        let x87 = expected.x87.as_mut().unwrap();
        x87.registers[3][..8].copy_from_slice(&abs_bytes(source).to_le_bytes());
        x87.registers[3][8] = 0xff;
        x87.registers[3][9] = 0xff;
        // TOS (bits 13:11) cleared, bits 0..10 and 14..15 (flags, condition codes, busy) kept.
        assert_eq!(x87.status_word, 0x2d41);
        x87.status_word = 0x2d41 & !0x3800;
        x87.tag_word = 0x0000;
    }
    expected.rip = CODE + code.len() as u64;
    assert_eq!(cpu, expected);
    // The source register keeps its payload and its exponent (Table 15-2: "Read from MMX register").
    assert_eq!(exponent(&cpu, 6), 0x3ff6);
    assert_eq!(cpu.x87.as_ref().unwrap().top(), 0);
    assert!(ram.reads.is_empty() && ram.writes.is_empty());
}

#[test]
fn mmn_is_physical_register_n_whatever_top_was() {
    for top in 0..8u16 {
        let code = reg_code(kind_named("pabsb"), 1, 2, 0);
        let mut cpu = cpu();
        cpu.x87.as_mut().unwrap().status_word = (top << 11) | 0x0041;
        let source = mm(&cpu, 2);
        let others: Vec<[u8; 10]> = (0..8)
            .filter(|index| *index != 1)
            .map(|index| cpu.x87.as_ref().unwrap().registers[index])
            .collect();
        let (outcome, _) = run_one(&code, &mut cpu);
        assert_eq!(outcome, Ok(StepOutcome::Advanced));
        assert_eq!(
            mm(&cpu, 1),
            abs_bytes(source),
            "TOP {top}: MM2 is R2, not ST(2)"
        );
        assert_eq!(exponent(&cpu, 1), 0xffff);
        let after: Vec<[u8; 10]> = (0..8)
            .filter(|index| *index != 1)
            .map(|index| cpu.x87.as_ref().unwrap().registers[index])
            .collect();
        assert_eq!(after, others, "TOP {top}: no other register changes");
        assert_eq!(cpu.x87.as_ref().unwrap().status_word, 0x0041, "TOP {top}");
        assert_eq!(cpu.x87.as_ref().unwrap().tag_word, 0);
    }
}

#[test]
fn every_mmx_form_has_the_same_x87_effect_for_register_and_memory_sources_and_for_every_tag_pattern()
 {
    for kind in &KINDS {
        for tag_word in [0x0000u16, 0xffff, 0xa5a5, 0x5a5a, 0x1be4, 0x00ff] {
            for (code, what) in [
                (reg_code(kind, 7, 0, 4), "register"),
                (mem_code(kind, 7, 4), "memory"),
            ] {
                let mut cpu = cpu();
                {
                    let x87 = cpu.x87.as_mut().unwrap();
                    x87.tag_word = tag_word;
                    x87.status_word = 0xb9c3;
                }
                let (outcome, _) = run_one(&code, &mut cpu);
                assert_eq!(outcome, Ok(StepOutcome::Advanced), "{} {what}", kind.name);
                let x87 = cpu.x87.as_ref().unwrap();
                assert_eq!(x87.tag_word, 0, "{} {what} {tag_word:#x}", kind.name);
                assert_eq!(x87.status_word, 0xb9c3 & !0x3800, "{} {what}", kind.name);
                assert_eq!(x87.control_word, 0x027f);
                assert_eq!(x87.instruction_pointer, 0x0123_4567_89ab_cdef);
                assert_eq!(x87.data_pointer, 0xfedc_ba98_7654_3210);
                assert_eq!(x87.last_opcode, 0x05aa);
                assert_eq!(exponent(&cpu, 7), 0xffff);
            }
        }
    }
}

// ---- EMMS ----

#[test]
fn emms_empties_every_tag_and_clears_top_and_keeps_the_registers_and_everything_else() {
    for top in 0..8u16 {
        for tag_word in [0x0000u16, 0xa5a5, 0xffff, 0x1be4] {
            let mut cpu = cpu();
            {
                let x87 = cpu.x87.as_mut().unwrap();
                x87.status_word = (top << 11) | 0x04c5;
                x87.tag_word = tag_word;
            }
            let before = cpu.clone();
            let (outcome, ram) = run_one(&EMMS, &mut cpu);
            assert_eq!(outcome, Ok(StepOutcome::Advanced), "TOP {top}");
            let mut expected = before;
            {
                let x87 = expected.x87.as_mut().unwrap();
                x87.tag_word = 0xffff;
                // Vol. 3A Table 15-2: TOS is 000B after EMMS as well; the other status bits are kept.
                x87.status_word = 0x04c5;
            }
            expected.rip = CODE + 2;
            assert_eq!(cpu, expected, "TOP {top} tags {tag_word:#x}");
            assert!(ram.reads.is_empty() && ram.writes.is_empty());
        }
    }
}

#[test]
fn emms_after_mmx_empties_the_tags_and_an_mmx_instruction_after_emms_makes_them_valid_again() {
    let pabsb = reg_code(kind_named("pabsb"), 0, 1, 0);
    let code = [&pabsb[..], &EMMS, &pabsb].concat();
    let mut cpu = cpu();
    let mut ram = Ram::new(&code);
    let tags = |cpu: &LongModeState| cpu.x87.as_ref().unwrap().tag_word;
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(tags(&cpu), 0x0000, "after PABSB: every tag valid");
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(tags(&cpu), 0xffff, "after EMMS: every tag empty");
    // The MMX data survives EMMS: the registers keep value and exponent.
    assert_eq!(exponent(&cpu, 0), 0xffff);
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(
        tags(&cpu),
        0x0000,
        "after the next MMX instruction: valid again"
    );
    assert_eq!(cpu.rip, CODE + code.len() as u64);
}

#[test]
fn emms_does_not_touch_flags_registers_or_sse_state() {
    let mut cpu = cpu();
    let before = cpu.clone();
    let (outcome, _) = run_one(&EMMS, &mut cpu);
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    assert_eq!(cpu.gpr, before.gpr);
    assert_eq!(cpu.rflags, before.rflags);
    assert_eq!(cpu.sse, before.sse);
}

// ---- transitions between MMX and x87 code ----

/// An x87 state in which no exception is pending and TOP = 7, as after `FNINIT` with TOP moved.
fn clean_x87() -> X87State {
    let mut x87 = x87_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0x3800;
    x87.tag_word = 0xffff;
    x87
}

#[test]
fn an_x87_instruction_after_an_mmx_instruction_sees_the_exponent_ones_register_as_invalid_data() {
    // PABSB mm0, mm0 writes MM0 (exponent all ones, an invalid x87 operand: NaN or infinity), sets TOP = 0
    // and every tag valid. FISTTP dword [rax] then converts ST(0) = R0: an invalid operation with the
    // exception masked stores the integer indefinite 0x80000000, sets IE and pops (TOP = 1, R0 empty).
    let pabsb = reg_code(kind_named("pabsb"), 0, 0, 0);
    let code = [&pabsb[..], &FISTTP_DWORD].concat();
    let mut cpu = cpu();
    cpu.x87 = Some(clean_x87());
    let mut ram = Ram::new(&code);
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(ram.get(DATA, 4), 0x8000_0000u32.to_le_bytes());
    let x87 = cpu.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, 0x0801, "TOP = 1 after the pop, IE set");
    assert_eq!(
        x87.tag_word, 0x0003,
        "R0 popped (empty), R1..R7 still valid from the MMX instruction"
    );
    assert_eq!(exponent(&cpu, 0), 0xffff);
}

#[test]
fn an_x87_instruction_after_emms_finds_an_empty_stack() {
    // EMMS sets TOP = 0 and every tag empty: FISTTP of the empty ST(0) is a stack underflow (IE and SF);
    // masked, it stores the integer indefinite and pops (TOP = 1).
    let code = [&EMMS[..], &FISTTP_DWORD].concat();
    let mut cpu = cpu();
    cpu.x87 = Some(clean_x87());
    let mut ram = Ram::new(&code);
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(cpu.x87.as_ref().unwrap().top(), 0);
    assert_eq!(
        step(&mut cpu, &mut ram, PrivilegeLevel::Ring0),
        Ok(StepOutcome::Advanced)
    );
    assert_eq!(ram.get(DATA, 4), 0x8000_0000u32.to_le_bytes());
    let x87 = cpu.x87.as_ref().unwrap();
    assert_eq!(x87.status_word, 0x0841, "TOP = 1, SF and IE");
    assert_eq!(x87.tag_word, 0xffff);
}

#[test]
fn the_xmm_forms_do_not_touch_the_x87_state() {
    // Vol. 1 §9.6.3: EMMS is not required between MMX and SSE code, because the SSE forms (66 prefix) work
    // on XMM registers and leave TOP, tags and registers alone.
    for kind in &KINDS {
        let mut code = vec![0x66];
        code.extend_from_slice(&reg_code(kind, 1, 2, 3));
        let mut cpu = cpu();
        let x87 = cpu.x87.clone();
        let (outcome, _) = run_one(&code, &mut cpu);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{}", kind.name);
        assert_eq!(cpu.x87, x87, "{}", kind.name);
    }
}

// ---- gates ----

fn fp_init_run(
    code: &[u8],
    cr0: u64,
    cr4: u64,
    prepare: impl FnOnce(&mut LongModeState),
) -> (RunExit, LongModeState, LongModeState, Ram) {
    let mut cpu = fp_init_cpu(cr0, cr4);
    prepare(&mut cpu);
    let before = cpu.clone();
    let mut ram = Ram::new(code);
    let (result, cpu) = run_fp_init(cpu, &mut ram);
    (result.exit, before, cpu, ram)
}

fn gated_codes() -> Vec<(Vec<u8>, &'static str)> {
    vec![
        (EMMS.to_vec(), "emms"),
        (reg_code(kind_named("pshufb"), 1, 2, 0), "pshufb mm1, mm2"),
        (
            mem_code(kind_named("palignr"), 1, 3),
            "palignr mm1, [rax], 3",
        ),
    ]
}

#[test]
fn cr0_em_and_ts_gate_every_mmx_instruction_and_cr4_osfxsr_does_not() {
    // #UD for CR0.EM = 1 (before #NM), #NM for CR0.TS = 1; CR4.OSFXSR = 0 is not a condition of an MMX
    // instruction (Table 2-21 names it for "Legacy SSE instruction" only). A gate fault changes nothing and
    // precedes any address check: [rax] is non-canonical in the `bad_address` rows.
    let cases: [(u64, u64, Option<u8>); 8] = [
        (CR0_BASE, CR4_PAE | CR4_OSFXSR, None),
        (CR0_BASE, CR4_PAE, None),
        (CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE | 4, CR4_PAE, Some(6)),
        (CR0_BASE | 8, CR4_PAE | CR4_OSFXSR, Some(7)),
        (CR0_BASE | 8, CR4_PAE, Some(7)),
        (CR0_BASE | 4 | 8, CR4_PAE | CR4_OSFXSR, Some(6)),
        (CR0_BASE | 4 | 8, CR4_PAE, Some(6)),
    ];
    for (code, name) in gated_codes() {
        for (cr0, cr4, vector) in cases {
            for bad_address in [false, true] {
                let label = format!("{name} cr0={cr0:#x} cr4={cr4:#x} bad_address={bad_address}");
                let (exit, before, after, ram) = fp_init_run(&code, cr0, cr4, |cpu| {
                    if bad_address {
                        cpu.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
                    }
                });
                match vector {
                    Some(vector) => {
                        assert_eq!(
                            exit,
                            RunExit::Fault(od_isa::ArchitecturalFault {
                                vector,
                                error_code: None,
                                linear_address: None
                            }),
                            "{label}"
                        );
                        assert_eq!(after, before, "{label}");
                        assert!(ram.reads.is_empty(), "{label}");
                    }
                    None if bad_address && name.contains('[') => {
                        assert_eq!(
                            exit,
                            RunExit::Fault(od_isa::ArchitecturalFault {
                                vector: 13,
                                error_code: Some(0),
                                linear_address: None
                            }),
                            "{label}"
                        );
                        assert_eq!(after, before, "{label}");
                    }
                    None => {
                        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{label}");
                        assert_eq!(
                            after.x87.as_ref().unwrap().tag_word & 0x3,
                            if name == "emms" { 3 } else { 0 }
                        );
                    }
                }
            }
        }
    }
}

/// A state with a pending unmasked exception: IE flagged and unmasked (FCW.IM = 0), ES set.
fn pending(cpu: &mut LongModeState) {
    let x87 = cpu.x87.as_mut().unwrap();
    x87.control_word = 0x037e;
    x87.status_word |= 0x0081;
}

#[test]
fn a_pending_unmasked_x87_exception_is_mf_for_every_mmx_instruction_after_the_em_and_ts_gates() {
    for (code, name) in gated_codes() {
        // Pending, CR0.NE = 1: #MF, nothing changes, no memory access (also with a non-canonical address:
        // #MF precedes the address checks, as on the physical oracle).
        for bad_address in [false, true] {
            let (exit, before, after, ram) =
                fp_init_run(&code, CR0_BASE, CR4_PAE | CR4_OSFXSR, |cpu| {
                    pending(cpu);
                    if bad_address {
                        cpu.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
                    }
                });
            assert_eq!(
                exit,
                RunExit::Fault(od_isa::ArchitecturalFault {
                    vector: 16,
                    error_code: None,
                    linear_address: None
                }),
                "{name} bad_address={bad_address}"
            );
            assert_eq!(after, before, "{name}");
            assert!(ram.reads.is_empty(), "{name}");
        }
        // #UD (EM) and #NM (TS) come first.
        for (cr0, vector) in [(CR0_BASE | 4, 6u8), (CR0_BASE | 8, 7), (CR0_BASE | 12, 6)] {
            let (exit, before, after, _) = fp_init_run(&code, cr0, CR4_PAE | CR4_OSFXSR, pending);
            assert_eq!(
                exit,
                RunExit::Fault(od_isa::ArchitecturalFault {
                    vector,
                    error_code: None,
                    linear_address: None
                }),
                "{name} cr0={cr0:#x}"
            );
            assert_eq!(after, before);
        }
        // CR0.NE = 0 would signal through FERR#, which the model does not have: refused, unchanged.
        let (exit, before, after, _) =
            fp_init_run(&code, CR0_BASE & !0x20, CR4_PAE | CR4_OSFXSR, pending);
        assert!(
            matches!(exit, RunExit::Unsupported { .. }),
            "{name}: {exit:?}"
        );
        assert_eq!(after, before);
        // The summary bit set while the flagged exception is masked is not pending: the instruction runs.
        let (exit, _, after, _) = fp_init_run(&code, CR0_BASE, CR4_PAE | CR4_OSFXSR, |cpu| {
            let x87 = cpu.x87.as_mut().unwrap();
            x87.control_word = 0x037f;
            x87.status_word |= 0x0081;
        });
        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{name} masked");
        assert_eq!(after.rip, CODE + code.len() as u64);
        // No exception flagged at all: runs.
        let (exit, _, _, _) = fp_init_run(&code, CR0_BASE, CR4_PAE | CR4_OSFXSR, |cpu| {
            cpu.x87.as_mut().unwrap().status_word &= !0x00ff;
        });
        assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{name} clean");
    }
}

#[test]
fn the_mf_check_works_without_control_registers_in_the_state_too() {
    // A bare run (no CR0/CR4 in the state) assumes CR0.NE = 1 and EM = TS = 0.
    for (code, name) in gated_codes() {
        let mut cpu = cpu();
        pending(&mut cpu);
        let before = cpu.clone();
        let (outcome, _) = run_one(&code, &mut cpu);
        assert_eq!(outcome, Ok(mf()), "{name}");
        assert_eq!(cpu, before);
    }
}

// ---- encodings of EMMS ----

#[test]
fn emms_decodes_with_rex_67_and_any_segment_override_and_refuses_the_prefixes_that_are_not_np() {
    let decode =
        |code: &[u8]| long_decode::decode(&mut Ram::new(code), CODE, AccessContext::default());
    let plain = decode(&EMMS).unwrap();
    assert_eq!(plain.operation, Operation::Emms);
    assert_eq!(plain.length, 2);
    for prefix in [
        &[0x40u8][..],
        &[0x48],
        &[0x4f],
        &[0x67],
        &[0x2e],
        &[0x3e],
        &[0x26],
        &[0x36],
        &[0x64],
        &[0x65],
    ] {
        let code = [prefix, &EMMS[..]].concat();
        let mut cpu = cpu();
        let before = cpu.clone();
        let (outcome, _) = run_one(&code, &mut cpu);
        assert_eq!(outcome, Ok(StepOutcome::Advanced), "{prefix:02x?}");
        assert_eq!(cpu.x87.as_ref().unwrap().tag_word, 0xffff);
        assert_eq!(cpu.rip, CODE + code.len() as u64);
        assert_eq!(cpu.gpr, before.gpr);
    }
    // LOCK is #UD, also with a REX byte and with the other prefixes around it.
    for code in [
        &[0xf0u8, 0x0f, 0x77][..],
        &[0xf0, 0x48, 0x0f, 0x77],
        &[0x2e, 0xf0, 0x0f, 0x77],
    ] {
        let mut cpu = cpu();
        let before = cpu.clone();
        let (outcome, _) = run_one(code, &mut cpu);
        assert_eq!(outcome, Ok(ud()), "{code:02x?}");
        assert_eq!(cpu, before);
    }
    // 66 / F2 / F3 (the SDM defines only the no-prefix form; the physical oracle gives #UD) and repeated
    // prefixes are refused, not guessed.
    for code in [
        &[0x66u8, 0x0f, 0x77][..],
        &[0xf2, 0x0f, 0x77],
        &[0xf3, 0x0f, 0x77],
        &[0x67, 0x67, 0x0f, 0x77],
        &[0x40, 0x40, 0x0f, 0x77],
    ] {
        let mut cpu = cpu();
        let before = cpu.clone();
        let (outcome, _) = run_one(code, &mut cpu);
        assert!(
            matches!(outcome, Ok(StepOutcome::Unsupported { opcode: 0x77 })),
            "{code:02x?}: {outcome:?}"
        );
        assert_eq!(cpu, before);
    }
}

#[test]
fn emms_needs_the_x87_state_and_runs_without_the_sse_state() {
    let mut cpu_a = cpu();
    cpu_a.x87 = None;
    let before = cpu_a.clone();
    let (outcome, _) = run_one(&EMMS, &mut cpu_a);
    assert!(matches!(
        outcome,
        Ok(StepOutcome::Unsupported { opcode: 0x77 })
    ));
    assert_eq!(cpu_a, before);
    let mut cpu_b = cpu();
    cpu_b.sse = None;
    let (outcome, _) = run_one(&EMMS, &mut cpu_b);
    assert_eq!(outcome, Ok(StepOutcome::Advanced));
    // The same under the FP-init CR0/CR4 pair: no x87 state is refused before any gate is read.
    for (code, name) in gated_codes() {
        let (exit, before, after, _) =
            fp_init_run(&code, CR0_BASE | 4, CR4_PAE | CR4_OSFXSR, |cpu| {
                cpu.x87 = None;
            });
        assert!(
            matches!(exit, RunExit::Unsupported { .. }),
            "{name}: {exit:?}"
        );
        assert_eq!(after, before, "{name}");
    }
}
