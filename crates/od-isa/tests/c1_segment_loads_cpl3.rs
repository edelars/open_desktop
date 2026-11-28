//! S7-32, ADR-0023 slice C1, plan step C1-5d (question V1: done in C1): the segment-register loads
//! of the set at CPL3 and CPL0 against a model of Intel SDM Vol. 2A "MOV—Move", Operation, protected
//! mode and 64-bit mode (the SDM text of the MOV page):
//!
//! - SS: a null selector is #GP(0) at CPL3 (64-bit mode allows it only at CPL < 3 with RPL = CPL);
//!   an index beyond the table limit, RPL ≠ CPL, not a writable data segment or DPL ≠ CPL is
//!   #GP(selector); not present is #SS(selector).
//! - DS, ES, FS, GS: a null selector loads (the register becomes unusable); beyond the limit, not a
//!   data or readable code segment, or (data or nonconforming code) with RPL > DPL or CPL > DPL is
//!   #GP(selector); not present is #NP(selector).
//! - A load sets the Accessed bit of the descriptor (an implicit supervisor write).
//!
//! The forms are `MOV Sreg, m16` (`8E /r` with `[rdi]`) and, at CPL3, the register forms of the
//! profile decoder (`8E D8`, `8E E0`, ...), which the CPL3 arm of the set routes the same way.

mod cpl3_sweep;

use cpl3_sweep::{CR4, Cells, GDT, machine_cached, run_exit, run_on};
use od_isa::execute::RunExit;
use od_isa::{ArchitecturalFault, LongModeState, Register};

const SLOT: u16 = 0x38;

#[derive(Debug, PartialEq)]
enum Expected {
    Loaded,
    Fault(u8, u32),
}

/// ES 0, SS 2, DS 3, FS 4, GS 5.
const SREGS: [u8; 5] = [0, 2, 3, 4, 5];

fn model(sreg: u8, selector: u16, descriptor: Option<u64>, cpl: u8) -> Expected {
    let gp = Expected::Fault(13, u32::from(selector & 0xfffc));
    if selector & 0xfffc == 0 {
        if sreg == 2 && (cpl == 3 || u8::try_from(selector & 3).unwrap() != cpl) {
            return Expected::Fault(13, 0);
        }
        return Expected::Loaded;
    }
    let Some(descriptor) = descriptor else {
        return gp;
    };
    let access = (descriptor >> 40) as u8;
    let segment = access & 0x10 != 0;
    let code = access & 0x08 != 0;
    let readable_or_writable = access & 0x02 != 0;
    let conforming = code && access & 0x04 != 0;
    let dpl = access >> 5 & 3;
    let rpl = (selector & 3) as u8;
    let present = access & 0x80 != 0;
    if sreg == 2 {
        if rpl != cpl || !segment || code || !readable_or_writable || dpl != cpl {
            return gp;
        }
        if !present {
            return Expected::Fault(12, u32::from(selector & 0xfffc));
        }
        return Expected::Loaded;
    }
    if !segment || (code && !readable_or_writable) {
        return gp;
    }
    if !conforming && (rpl > dpl || cpl > dpl) {
        return gp;
    }
    if !present {
        return Expected::Fault(11, u32::from(selector & 0xfffc));
    }
    Expected::Loaded
}

fn selector_of(state: &LongModeState, sreg: u8) -> u16 {
    match sreg {
        0 => state.es_selector,
        2 => state.ss_selector.unwrap(),
        3 => state.ds_selector,
        4 => state.fs_selector,
        _ => state.gs_selector,
    }
}

/// Run `mov sreg, [rdi]` with the selector at [rdi] and `descriptor` in GDT slot 0x38.
fn check(sreg: u8, selector: u16, descriptor: u64, cpl: u8, register_form: bool) -> (bool, u8) {
    let (mut state, mut memory) = machine_cached(cpl, 0, CR4);
    memory.put_u64(GDT + u64::from(SLOT), descriptor);
    let rdi = state.gpr[Register::Rdi as usize];
    memory.put(rdi, &selector.to_le_bytes());
    state.gpr[Register::Rax as usize] = 0xffff_0000 | u64::from(selector);
    let code = if register_form {
        // `8E /r` with AX (the profile decoder's bare forms).
        vec![0x8e, 0xc0 | (sreg << 3)]
    } else {
        vec![0x8e, (sreg << 3) | 7]
    };
    let before = state.clone();
    let run = run_on(state, memory, Cells::new(), &code, 1);
    let in_table = u64::from(selector & !7) + 7 <= 0x57;
    let expected = model(
        sreg,
        selector,
        in_table.then_some(descriptor_at(selector, descriptor, &before)),
        cpl,
    );
    let name = format!(
        "sreg {sreg} selector {selector:#x} descriptor {descriptor:#018x} CPL{cpl} register {register_form}"
    );
    let exit = run_exit(run.result.as_ref().unwrap()).clone();
    let tally = match expected {
        Expected::Loaded => (true, 0),
        Expected::Fault(vector, _) => (false, vector),
    };
    match expected {
        Expected::Loaded => {
            assert_eq!(exit, RunExit::InstructionBudgetExhausted, "{name}");
            assert_eq!(selector_of(&run.state, sreg), selector, "{name}");
            if selector & 0xfffc == SLOT {
                let after = run.memory.peek_u64(GDT + u64::from(SLOT)).unwrap();
                assert_eq!(after, descriptor | (1 << 40), "{name}: the Accessed bit");
            }
        }
        Expected::Fault(vector, code) => {
            assert_eq!(
                exit,
                RunExit::Fault(ArchitecturalFault {
                    vector,
                    error_code: Some(code),
                    linear_address: None,
                }),
                "{name}"
            );
            assert_eq!(run.state, before, "{name}");
            assert!(run.memory.writes.is_empty(), "{name}");
        }
    }
    tally
}

/// The descriptor a selector names in the machine (slot 0x38 holds the case's descriptor).
fn descriptor_at(selector: u16, slot: u64, _: &LongModeState) -> u64 {
    if selector & !7 == SLOT {
        return slot;
    }
    let (_, memory) = machine_cached(0, 0, CR4);
    memory.peek_u64(GDT + u64::from(selector & !7)).unwrap()
}

fn descriptor(segment: bool, kind: u8, dpl: u8, present: bool) -> u64 {
    let access = u64::from(kind)
        | (u64::from(segment) << 4)
        | (u64::from(dpl) << 5)
        | (u64::from(present) << 7);
    0x00cf_0000_0000_ffff | (access << 40)
}

#[test]
fn segment_loads_match_the_sdm_model_at_cpl0_and_cpl3() {
    let mut cases = 0;
    let mut outcomes = std::collections::BTreeMap::new();
    for cpl in [0u8, 3] {
        for sreg in SREGS {
            // The descriptor matrix in slot 0x38 with every RPL.
            for segment in [false, true] {
                for kind in 0..16u8 {
                    for dpl in 0..4u8 {
                        for present in [false, true] {
                            for rpl in 0..4u16 {
                                // Every accessed bit clear, so a load writes it.
                                let descriptor = descriptor(segment, kind & !1, dpl, present);
                                *outcomes
                                    .entry((cpl, check(sreg, SLOT | rpl, descriptor, cpl, false)))
                                    .or_insert(0) += 1;
                                cases += 1;
                            }
                        }
                    }
                }
            }
            // The machine's own selectors, null and beyond the limit.
            for selector in [0u16, 1, 3, 0x2b, 0x33, 0x10, 0x08, 0x13, 0x53, 0x63, 0x60] {
                let _ = check(sreg, selector, descriptor(true, 2, 3, true), cpl, false);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 2 * 5 * (2 * 16 * 4 * 2 * 4 + 11));
    // Every outcome of the model occurs at both CPLs: loads, #GP(sel), #NP(sel), #SS(sel).
    for cpl in [0u8, 3] {
        for outcome in [(true, 0), (false, 13), (false, 11), (false, 12)] {
            assert!(
                outcomes.get(&(cpl, outcome)).is_some_and(|n| *n > 0),
                "CPL{cpl} {outcome:?}: {outcomes:?}"
            );
        }
    }
}

#[test]
fn the_register_forms_at_cpl3_take_the_same_rules() {
    for sreg in SREGS {
        for selector in [0u16, 3, 0x2b, 0x33, 0x10, 0x63, SLOT | 3] {
            for descriptor in [
                descriptor(true, 2, 3, true),
                descriptor(true, 2, 0, true),
                descriptor(true, 0xa, 3, true),
            ] {
                let _ = check(sreg, selector, descriptor, 3, true);
            }
        }
    }
}

#[test]
fn a_null_ss_at_cpl3_is_gp0_and_a_null_ds_loads() {
    let (state, memory) = machine_cached(3, 0, CR4);
    let mut state_ds = state.clone();
    state_ds.gpr[Register::Rax as usize] = 0;
    let run = run_on(state_ds, memory.clone(), Cells::new(), &[0x8e, 0xd8], 1);
    assert_eq!(
        run_exit(run.result.as_ref().unwrap()),
        &RunExit::InstructionBudgetExhausted
    );
    assert_eq!(run.state.ds_selector, 0);
    assert!(run.state.ds_unusable);
    let mut state_ss = state;
    state_ss.gpr[Register::Rax as usize] = 3;
    let before = state_ss.clone();
    let run = run_on(state_ss, memory, Cells::new(), &[0x8e, 0xd0], 1);
    assert_eq!(run_exit(run.result.as_ref().unwrap()), &cpl3_sweep::GP0);
    assert_eq!(run.state, before);
}

/// Review A3: `MOV SS` at CPL3 opens the one-instruction interrupt shadow (SDM Vol. 3A "Interrupt
/// shadow"); with IF = 1 the next instruction runs in the set with and without the STI member, and the
/// shadow ends after it.
#[test]
fn mov_ss_at_cpl3_with_if_set_runs_the_next_instruction_with_and_without_the_sti_member() {
    use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet};
    for with_sti in [true, false] {
        let cr4 = if with_sti {
            CR4
        } else {
            CR4 & !((1 << 3) | (1 << 6))
        };
        let (mut state, mut memory) = machine_cached(3, 0, cr4);
        state.gpr[Register::Rax as usize] = 0x2b;
        assert_ne!(state.rflags & (1 << 9), 0, "IF is set");
        memory.put(cpl3_sweep::RIP, &[0x8e, 0xd0, 0x90]);
        let cells = Cells::new();
        let bare = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
        let set = if with_sti {
            cpl3_sweep::set(&cells)
        } else {
            bare
        };
        if !with_sti {
            state.cr3 = None;
        }
        let result = cpl3_sweep::run_cpl3_with(&mut state, &mut memory, &set, 2, None).unwrap();
        assert_eq!(
            run_exit(&result),
            &RunExit::InstructionBudgetExhausted,
            "STI member {with_sti}"
        );
        assert_eq!(result.instructions_executed, 2, "STI member {with_sti}");
        assert_eq!(state.rip, cpl3_sweep::RIP + 3);
        assert_eq!(state.sti_shadow, Some(od_isa::StiShadow::Clear));
        assert_eq!(state.ss_selector, Some(0x2b));
    }
}

/// Review R1: a CPL3 REP string under a `Blocking` shadow (IF = 1) ends the shadow when it retires,
/// as the set at CPL0 does after the one retired step of the shared REP executor; across the budget
/// boundary of a partial string (not retired) the shadow is kept. With and without the STI member.
#[test]
fn a_cpl3_rep_string_ends_the_shadow_when_it_retires() {
    use od_isa::StiShadow::{Blocking, Clear};
    use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet};
    const REP: &[u8] = &[0xf3, 0xa4, 0x90];
    const MOV_SS_REP: &[u8] = &[0x8e, 0xd0, 0xf3, 0xa4, 0x90];
    // (name, code, shadow at entry, RCX at entry, budget, retired, RIP offset, RCX, shadow after)
    type Case = (
        &'static str,
        &'static [u8],
        bool,
        u64,
        u64,
        u64,
        u64,
        u64,
        od_isa::StiShadow,
    );
    let cases: &[Case] = &[
        (
            "shadow + REP, budget 1 (partial)",
            REP,
            true,
            2,
            1,
            1,
            0,
            1,
            Blocking,
        ),
        ("shadow + REP, budget 2", REP, true, 2, 2, 2, 2, 0, Clear),
        (
            "shadow + REP + NOP, budget 3",
            REP,
            true,
            2,
            3,
            3,
            3,
            0,
            Clear,
        ),
        (
            "shadow + REP with RCX = 0, budget 1",
            REP,
            true,
            0,
            1,
            1,
            2,
            0,
            Clear,
        ),
        (
            "MOV SS + REP, budget 2 (partial)",
            MOV_SS_REP,
            false,
            2,
            2,
            2,
            2,
            1,
            Blocking,
        ),
        (
            "MOV SS + REP, budget 3",
            MOV_SS_REP,
            false,
            2,
            3,
            3,
            4,
            0,
            Clear,
        ),
        (
            "MOV SS + REP + NOP, budget 4",
            MOV_SS_REP,
            false,
            2,
            4,
            4,
            5,
            0,
            Clear,
        ),
    ];
    for with_sti in [true, false] {
        for &(name, code, shadow, count, budget, retired, offset, rcx, after) in cases {
            let cr4 = if with_sti {
                CR4
            } else {
                CR4 & !((1 << 3) | (1 << 6))
            };
            let (mut state, mut memory) = machine_cached(3, 0, cr4);
            state.gpr[Register::Rax as usize] = 0x2b;
            state.gpr[Register::Rcx as usize] = count;
            state.gpr[Register::Rsi as usize] = cpl3_sweep::DATA + 0x100;
            state.gpr[Register::Rdi as usize] = cpl3_sweep::DATA + 0x200;
            state.rflags &= !(1 << 10);
            assert_ne!(state.rflags & (1 << 9), 0, "IF is set");
            if shadow {
                state.sti_shadow = Some(Blocking);
            }
            memory.put(cpl3_sweep::RIP, code);
            let cells = Cells::new();
            let set = if with_sti {
                cpl3_sweep::set(&cells)
            } else {
                state.cr3 = None;
                Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            };
            let result =
                cpl3_sweep::run_cpl3_with(&mut state, &mut memory, &set, budget, None).unwrap();
            let what = format!("{name}, STI member {with_sti}");
            assert_eq!(
                run_exit(&result),
                &RunExit::InstructionBudgetExhausted,
                "{what}"
            );
            assert_eq!(result.instructions_executed, retired, "{what}");
            assert_eq!(state.rip, cpl3_sweep::RIP + offset, "{what}");
            assert_eq!(state.gpr[Register::Rcx as usize], rcx, "{what}");
            assert_eq!(state.sti_shadow, Some(after), "{what}");
        }
    }
}
