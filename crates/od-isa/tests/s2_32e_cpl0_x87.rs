//! S2-32e (CPL0 set): the bounded x87 forms of S2-32a/b/c in the CPL0 profile set with the FP member. The x87
//! half of the member's cell is moved into the CPU state for the length of one instruction and the bounded profile
//! of the bare run (`step`, S2-32a/b/c) executes it unchanged; the state is stored back afterwards. The expected
//! values are the results of the same instruction in the bare run on the same x87 state, the literal images of
//! S2-32c and the gate rules of Intel SDM Vol. 2A (FSIN, FBLD, FNSTENV, FNSAVE and FRSTOR "Protected Mode
//! Exceptions": #NM when CR0.EM[bit 2] or CR0.TS[bit 3] is set; CR4.OSFXSR is not among them) and Vol. 3A 2.5.
//!
//! What the set does NOT change (no FCS/FDS bypass): the CPL0 set admits only a cell with both selectors
//! represented (the plain FXSAVE image carries them), and the bounded profile refuses every x87 form that would
//! update FIP/FCS/FDP/FDS from the instruction while a selector is represented. So in the set the ten
//! transcendental forms of S2-32a and FBLD/FBSTP of S2-32b are gated (#NM) and then refused with no change, exactly
//! like the bare run with represented selectors; FNSTENV, FNSAVE and FRSTOR define the selectors themselves and
//! run. Every other x87 form stays refused in the set.

mod mmx_common;

use mmx_common::*;
use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    GuestFpState, RunConfig, RunExit, StepOutcome, run_with_cpl0_profile_set,
};
use od_isa::state::{X87State, X87Tag};
use od_isa::{LongModeState, PrivilegeLevel, Register, cpuid::CpuidProfile};
use std::cell::RefCell;

const CR4_FP: u64 = CR4_PAE | CR4_OSFXSR | (1 << 10);
const FNSTENV: [u8; 2] = [0xd9, 0x30];
const FNSAVE: [u8; 2] = [0xdd, 0x30];
const FRSTOR: [u8; 2] = [0xdd, 0x20];

const TRANSCENDENTAL: &[(&str, [u8; 2])] = &[
    ("f2xm1", [0xd9, 0xf0]),
    ("fyl2x", [0xd9, 0xf1]),
    ("fptan", [0xd9, 0xf2]),
    ("fpatan", [0xd9, 0xf3]),
    ("fxtract", [0xd9, 0xf4]),
    ("fyl2xp1", [0xd9, 0xf9]),
    ("fsincos", [0xd9, 0xfb]),
    ("fscale", [0xd9, 0xfd]),
    ("fsin", [0xd9, 0xfe]),
    ("fcos", [0xd9, 0xff]),
];
const BCD: &[(&str, [u8; 2])] = &[("fbld [rax]", [0xdf, 0x20]), ("fbstp [rax]", [0xdf, 0x30])];
const ENVIRONMENT: &[(&str, [u8; 2])] = &[
    ("fnstenv [rax]", FNSTENV),
    ("fnsave [rax]", FNSAVE),
    ("frstor [rax]", FRSTOR),
];

fn all_forms() -> Vec<(&'static str, [u8; 2])> {
    [TRANSCENDENTAL, BCD, ENVIRONMENT].concat()
}

/// The S2-39 x87 seed (TOP = 5, mixed tags, all-masked control word) with known FCS/FDS, which the FP member requires.
fn seed() -> X87State {
    let mut x87 = x87_seed();
    x87.instruction_selector = Some(0x2b);
    x87.data_selector = Some(0x53);
    x87
}

/// An x87 state after FNINIT with the given selectors.
fn initial(selectors: Option<u16>) -> X87State {
    let mut x87 = X87State::fixture_seed();
    x87.control_word = 0x037f;
    x87.status_word = 0;
    x87.tag_word = 0xffff;
    x87.instruction_pointer = 0;
    x87.data_pointer = 0;
    x87.last_opcode = 0;
    x87.instruction_selector = selectors;
    x87.data_selector = selectors;
    x87
}

/// TOP = 2 with ST(0) = `st0` and ST(1) = 1.0 valid: every transcendental form has its operands and a free slot.
fn ready(st0: [u8; 10], selectors: Option<(u16, u16)>) -> X87State {
    let mut x87 = initial(None);
    x87.status_word = 0x1000;
    x87.registers[2] = st0;
    x87.registers[3] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f];
    x87.set_tag(2, X87Tag::Valid);
    x87.set_tag(3, X87Tag::Valid);
    if let Some((code, data)) = selectors {
        x87.instruction_selector = Some(code);
        x87.data_selector = Some(data);
    }
    x87
}

const TWO: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x40];
const HALF: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfe, 0x3f];
const QUARTER: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfd, 0x3f];

/// An operand inside the range of each form (F2XM1: |x| <= 1, FYL2XP1: |x| <= 1 - sqrt(2)/2).
fn st0_of(name: &str) -> [u8; 10] {
    match name {
        "f2xm1" => HALF,
        "fyl2xp1" => QUARTER,
        _ => TWO,
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Observed {
    exit: String,
    rip: u64,
    rflags: u64,
    gpr: [u64; 16],
    x87: X87State,
    memory: Vec<u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, usize)>,
}

fn base_state(cr0: u64, cr4: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: FLAGS,
        cr0: Some(cr0),
        cr4: Some(cr4),
        activity: Some(od_isa::CpuActivity::Running),
        sti_shadow: Some(od_isa::StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr = cpu().gpr;
    state
}

fn observe_memory(ram: &Ram) -> Vec<u8> {
    ram.bytes[DATA as usize..DATA as usize + 0x400].to_vec()
}

/// The bare reference: the plain `step` (bounded x87 profile) on a state that carries `x87`.
fn bare(code: &[u8], x87: &X87State, prepare: &dyn Fn(&mut LongModeState, &mut Ram)) -> Observed {
    let mut state = cpu();
    state.x87 = Some(x87.clone());
    let mut ram = Ram::new(code);
    prepare(&mut state, &mut ram);
    let mut exit = String::from("InstructionBudgetExhausted");
    let mut position = CODE;
    for _ in 0..count_of(code) {
        state.rip = position;
        match execute::step(&mut state, &mut ram, ctx(PrivilegeLevel::Ring0)) {
            Ok(StepOutcome::Advanced) => position = state.rip,
            other => {
                exit = format!("{other:?}");
                break;
            }
        }
    }
    Observed {
        exit,
        rip: state.rip,
        rflags: state.rflags,
        gpr: state.gpr,
        x87: state.x87.clone().unwrap(),
        memory: observe_memory(&ram),
        reads: ram.reads.clone(),
        writes: ram.writes.clone(),
    }
}

/// The number of two-byte instructions of a test program.
fn count_of(code: &[u8]) -> usize {
    code.len() / 2
}

fn make_set<'a>(cell: &'a RefCell<GuestFpState>) -> Cpl0ProfileSet<'a> {
    Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
        .with_fp_state(cell)
}

fn run_config(count: u64) -> RunConfig {
    RunConfig {
        instruction_budget: count,
        stop_rip: None,
        context: ctx(PrivilegeLevel::Ring0),
    }
}

fn in_set(
    code: &[u8],
    cr0: u64,
    cr4: u64,
    x87: &X87State,
    prepare: &dyn Fn(&mut LongModeState, &mut Ram),
) -> (Observed, LongModeState, GuestFpState, GuestFpState) {
    let mut state = base_state(cr0, cr4);
    let mut ram = Ram::new(code);
    prepare(&mut state, &mut ram);
    let cell = RefCell::new(GuestFpState {
        x87: x87.clone(),
        sse: cpu().sse.unwrap(),
    });
    let before = cell.borrow().clone();
    let set = make_set(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        run_config(count_of(code) as u64),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .expect("the case is admitted by the set");
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("unexpected set exit {:?}", result.exit)
    };
    let after = cell.borrow().clone();
    assert!(
        state.x87.is_none() && state.sse.is_none(),
        "the set hands the state back without FP"
    );
    let observed = Observed {
        exit: format!("{exit:?}"),
        rip: state.rip,
        rflags: state.rflags,
        gpr: state.gpr,
        x87: after.x87.clone(),
        memory: observe_memory(&ram),
        reads: ram.reads.clone(),
        writes: ram.writes.clone(),
    };
    (observed, state, before, after)
}

fn data(state: &mut LongModeState, ram: &mut Ram) {
    state.gpr[Register::Rax as usize] = DATA;
    ram.put(DATA, &[0xa5; 0x200]);
}

/// The 108-byte image that FNSAVE of `x87` stores, taken from the bare run.
fn image_of(x87: &X87State) -> Vec<u8> {
    bare(&FNSAVE, x87, &data).memory[..108].to_vec()
}

fn states() -> Vec<(&'static str, X87State)> {
    let mut top3 = seed();
    top3.status_word = 0x1800 | 0x0041; // TOP = 3, IE and PE sticky
    top3.control_word = 0x0f7f;
    top3.registers[3] = [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f]; // infinity
    top3.registers[4] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]; // denormal
    let mut fcs_zero = seed();
    fcs_zero.instruction_selector = Some(0);
    fcs_zero.data_selector = Some(0x1234);
    vec![
        ("seed", seed()),
        ("init", initial(Some(0))),
        ("top3", top3),
        ("zero fcs", fcs_zero),
        ("ready", ready(TWO, Some((0x10, 0x18)))),
    ]
}

#[test]
fn fnstenv_and_fnsave_in_the_set_give_the_image_of_the_bare_run() {
    for (label, x87) in states() {
        for (name, code) in ENVIRONMENT.iter().take(2) {
            let reference = bare(code, &x87, &data);
            let (observed, _, before, after) = in_set(code, CR0_BASE, CR4_FP, &x87, &data);
            assert_eq!(
                reference.exit, "InstructionBudgetExhausted",
                "{name} {label}"
            );
            assert_eq!(observed, reference, "{name} {label}");
            assert_eq!(observed.writes.len(), 1, "{name} {label}: one write");
            assert_eq!(
                before.sse, after.sse,
                "{name} {label}: the SSE half is untouched"
            );
            if *name == "fnstenv [rax]" {
                // FNSTENV masks all exceptions: the all-masked control word is kept.
                assert_eq!(after.x87.control_word, x87.control_word, "{label}");
            } else {
                assert_eq!(
                    after.x87.control_word, 0x037f,
                    "{label}: FNSAVE re-initialises"
                );
            }
        }
    }
}

#[test]
fn frstor_in_the_set_loads_the_image_like_the_bare_run() {
    let images: Vec<Vec<u8>> = states().iter().map(|(_, x87)| image_of(x87)).collect();
    for (label, x87) in states() {
        for (index, image) in images.iter().enumerate() {
            let put = |state: &mut LongModeState, ram: &mut Ram| {
                data(state, ram);
                ram.put(DATA, image);
            };
            let reference = bare(&FRSTOR, &x87, &put);
            let (observed, _, _, after) = in_set(&FRSTOR, CR0_BASE, CR4_FP, &x87, &put);
            assert_eq!(
                reference.exit, "InstructionBudgetExhausted",
                "{label} <- {index}"
            );
            assert_eq!(observed, reference, "{label} <- {index}");
            assert_eq!(observed.reads, vec![(DATA, 108)], "{label} <- {index}");
            assert!(observed.writes.is_empty(), "{label} <- {index}");
            // The selectors of the image are loaded as represented values.
            assert_eq!(
                after.x87.data_selector,
                Some(u16::from_le_bytes([image[24], image[25]])),
                "{label} <- {index}"
            );
        }
    }
}

#[test]
fn a_program_of_environment_forms_in_the_set_equals_the_bare_steps() {
    // FNSAVE [rax] ; FRSTOR [rax] ; FNSTENV [rax] ; FNSAVE [rax] ; FRSTOR [rax]
    let code: Vec<u8> = [FNSAVE, FRSTOR, FNSTENV, FNSAVE, FRSTOR].concat();
    for (label, x87) in states() {
        let reference = bare(&code, &x87, &data);
        let (observed, ..) = in_set(&code, CR0_BASE, CR4_FP, &x87, &data);
        assert_eq!(reference.exit, "InstructionBudgetExhausted", "{label}");
        assert_eq!(observed, reference, "{label}");
        assert_eq!(observed.writes.len(), 3, "{label}");
        assert_eq!(observed.reads.len(), 2, "{label}");
    }
}

#[test]
fn a_literal_environment_image_and_the_state_after_fnsave_in_the_set() {
    let x87 = seed();
    let (observed, _, before, after) = in_set(&FNSTENV, CR0_BASE, CR4_FP, &x87, &data);
    let image = &observed.memory[..28];
    assert_eq!(&image[0..2], &0x027f_u16.to_le_bytes(), "FCW");
    assert_eq!(&image[2..4], &[0xff, 0xff], "reserved");
    assert_eq!(&image[4..6], &0x2d41_u16.to_le_bytes(), "FSW");
    assert_eq!(
        &image[12..16],
        &0x89ab_cdef_u32.to_le_bytes(),
        "FIP low 32 bits"
    );
    assert_eq!(&image[16..18], &0x2b_u16.to_le_bytes(), "FCS");
    assert_eq!(
        &image[20..24],
        &0x7654_3210_u32.to_le_bytes(),
        "FDP low 32 bits"
    );
    assert_eq!(&image[24..26], &0x53_u16.to_le_bytes(), "FDS");
    assert_eq!(
        &observed.memory[28..32],
        &[0xa5; 4],
        "nothing beyond 28 bytes"
    );
    assert_eq!(after, before, "FNSTENV keeps the cell");

    let (observed, _, _, after) = in_set(&FNSAVE, CR0_BASE, CR4_FP, &x87, &data);
    assert_eq!(observed.writes, vec![(DATA, 108)]);
    assert_eq!(after.x87.control_word, 0x037f);
    assert_eq!(after.x87.status_word, 0);
    assert_eq!(after.x87.tag_word, 0xffff);
    assert_eq!(after.x87.instruction_selector, Some(0), "known stays known");
    assert_eq!(after.x87.data_selector, Some(0));
    assert_eq!(after.x87.instruction_pointer, 0);
    // ST(0) of the image is the register at TOP = 5.
    assert_eq!(&observed.memory[28..38], &x87.registers[5]);
}

#[test]
fn fwait_before_fnstenv_stores_the_same_image_in_the_set() {
    let plain = in_set(&FNSTENV, CR0_BASE, CR4_FP, &seed(), &data).0;
    let code = [0x9b, 0xd9, 0x30];
    let mut state = base_state(CR0_BASE, CR4_FP);
    let mut ram = Ram::new(&code);
    data(&mut state, &mut ram);
    let cell = RefCell::new(GuestFpState {
        x87: seed(),
        sse: cpu().sse.unwrap(),
    });
    let set = make_set(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        run_config(2),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap();
    assert!(matches!(
        result.exit,
        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
    ));
    assert_eq!(observe_memory(&ram), plain.memory);
}

/// Run one form in the set under the given control registers and return the exit and the three states.
fn gate_case(code: &[u8; 2], cr0: u64, cr4: u64) -> (String, bool, bool, bool, bool) {
    let x87 = seed();
    let mut state = base_state(cr0, cr4);
    let mut ram = Ram::new(code);
    data(&mut state, &mut ram);
    let cell = RefCell::new(GuestFpState {
        x87: x87.clone(),
        sse: cpu().sse.unwrap(),
    });
    let before = cell.borrow().clone();
    let state_before = state.clone();
    let memory_before = observe_memory(&ram);
    let set = make_set(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        run_config(1),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap();
    let Cpl0ProfileSetExit::Run(exit) = result.exit else {
        panic!("unexpected set exit {:?}", result.exit)
    };
    (
        format!("{exit:?}"),
        *cell.borrow() == before,
        state == state_before,
        observe_memory(&ram) == memory_before,
        ram.reads.is_empty() && ram.writes.is_empty(),
    )
}

#[test]
fn em_and_ts_raise_nm_for_every_connected_form_and_change_nothing() {
    for (name, code) in all_forms() {
        for (label, cr0) in [
            ("EM", CR0_BASE | 1 << 2),
            ("TS", CR0_BASE | 1 << 3),
            ("EM+TS", CR0_BASE | 1 << 2 | 1 << 3),
        ] {
            let (exit, cell, state, memory, no_access) = gate_case(&code, cr0, CR4_FP);
            assert!(
                exit.starts_with("Fault(") && exit.contains("vector: 7"),
                "{name} {label}: {exit}"
            );
            assert!(
                cell && state && memory && no_access,
                "{name} {label}: nothing changes"
            );
        }
    }
}

#[test]
fn osfxsr_is_not_a_gate_of_the_x87_forms() {
    // Without CR4.OSFXSR (and without it and PAE) the outcome is the one with it: an environment form runs, the
    // transcendental and BCD forms are refused by the selector rule, never #UD or #NM.
    for cr4 in [CR4_FP, CR4_PAE, CR4_PAE | 1 << 10] {
        for (name, code) in all_forms() {
            let (with_gate, ..) = in_set(&code, CR0_BASE, CR4_FP, &seed(), &data);
            let (observed, ..) = in_set(&code, CR0_BASE, cr4, &seed(), &data);
            assert_eq!(observed, with_gate, "{name}: CR4 = {cr4:#x}");
        }
    }
    let (observed, ..) = in_set(&FNSTENV, CR0_BASE, CR4_PAE, &seed(), &data);
    assert_eq!(observed.exit, "InstructionBudgetExhausted");
}

#[test]
fn transcendental_and_bcd_forms_are_refused_by_the_selector_rule_exactly_like_the_bare_run() {
    for (name, code) in TRANSCENDENTAL.iter().chain(BCD) {
        let bcd = *name == "fbld [rax]";
        let put = |state: &mut LongModeState, ram: &mut Ram| {
            data(state, ram);
            if bcd {
                ram.put(DATA, &[0x45, 0x23, 0x01, 0, 0, 0, 0, 0, 0, 0]);
            }
        };
        let with_selectors = ready(st0_of(name), Some((0x2b, 0x53)));
        let without = ready(st0_of(name), None);

        // Control: with no selector represented the bare run completes the form, so the refusal below is the rule
        // about a represented FCS/FDS and not the operand, the status word or the encoding.
        let completes = bare(code, &without, &put);
        assert_eq!(
            completes.exit, "InstructionBudgetExhausted",
            "{name}: control"
        );
        assert_ne!(
            completes.x87, without,
            "{name}: control changes the x87 state"
        );

        let reference = bare(code, &with_selectors, &put);
        let (observed, state, before, after) =
            in_set(code, CR0_BASE, CR4_FP, &with_selectors, &put);
        assert!(
            reference.exit.starts_with("Ok(Unsupported"),
            "{name}: {}",
            reference.exit
        );
        assert!(
            observed.exit.starts_with("Unsupported"),
            "{name}: {}",
            observed.exit
        );
        // Same state, memory and accesses as the bare refusal.
        assert_eq!(observed.x87, reference.x87, "{name}");
        assert_eq!(observed.memory, reference.memory, "{name}");
        assert_eq!(observed.reads, reference.reads, "{name}");
        assert_eq!(observed.writes, reference.writes, "{name}");
        assert_eq!(after, before, "{name}: the cell is unchanged");
        assert_eq!(state.rip, CODE, "{name}: nothing retired");
    }
}

#[test]
fn the_set_does_not_admit_a_cell_without_selectors_so_there_is_no_fcs_fds_bypass() {
    let cell = RefCell::new(GuestFpState {
        x87: ready(TWO, None),
        sse: cpu().sse.unwrap(),
    });
    let mut state = base_state(CR0_BASE, CR4_FP);
    let mut ram = Ram::new(&[0xd9, 0xfe]);
    let set = make_set(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        run_config(1),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    );
    assert_eq!(
        result.unwrap_err(),
        Cpl0ProfileSetInputError::FpStateNotAdmitted
    );
}

#[test]
fn faults_and_refusals_of_the_environment_forms_in_the_set_equal_the_bare_run() {
    let good = image_of(&seed());
    let mut unmasked = good.clone();
    unmasked[0..2].copy_from_slice(&0x037e_u16.to_le_bytes());
    let mut summary = good.clone();
    summary[4..6].copy_from_slice(&0x0080_u16.to_le_bytes());

    let mut unmasked_cw = seed();
    unmasked_cw.control_word = 0x037e;
    let mut es_set = seed();
    es_set.status_word |= 0x0080;
    let mut busy = seed();
    busy.status_word |= 0x8000;

    type Prepare = Box<dyn Fn(&mut LongModeState, &mut Ram)>;
    let cases: Vec<(&str, [u8; 2], X87State, Prepare, &str)> = vec![
        (
            "fnstenv unmasked CW",
            FNSTENV,
            unmasked_cw.clone(),
            Box::new(data),
            "Unsupported",
        ),
        (
            "fnsave unmasked CW",
            FNSAVE,
            unmasked_cw,
            Box::new(data),
            "Unsupported",
        ),
        ("fnstenv ES", FNSTENV, es_set, Box::new(data), "Unsupported"),
        ("fnsave B", FNSAVE, busy, Box::new(data), "Unsupported"),
        (
            "frstor unmasked image",
            FRSTOR,
            seed(),
            Box::new({
                let image = unmasked;
                move |state, ram| {
                    data(state, ram);
                    ram.put(DATA, &image);
                }
            }),
            "Unsupported",
        ),
        (
            "frstor ES image",
            FRSTOR,
            seed(),
            Box::new({
                let image = summary;
                move |state, ram| {
                    data(state, ram);
                    ram.put(DATA, &image);
                }
            }),
            "Unsupported",
        ),
        (
            "fnstenv #PF",
            FNSTENV,
            seed(),
            Box::new(|state, ram| {
                data(state, ram);
                ram.fault_page = Some(DATA);
            }),
            "Fault",
        ),
        (
            "fnsave #PF",
            FNSAVE,
            seed(),
            Box::new(|state, ram| {
                data(state, ram);
                ram.fault_page = Some(DATA);
            }),
            "Fault",
        ),
        (
            "frstor #PF",
            FRSTOR,
            seed(),
            Box::new(|state, ram| {
                data(state, ram);
                ram.fault_page = Some(DATA);
            }),
            "Fault",
        ),
        (
            "fnstenv non-canonical",
            FNSTENV,
            seed(),
            Box::new(|state, _| state.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000),
            "Fault",
        ),
        (
            "frstor non-canonical",
            FRSTOR,
            seed(),
            Box::new(|state, _| state.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000),
            "Fault",
        ),
    ];
    for (name, code, x87, prepare, kind) in &cases {
        let reference = bare(code, x87, prepare.as_ref());
        let (observed, state, before, after) =
            in_set(code, CR0_BASE, CR4_FP, x87, prepare.as_ref());
        assert!(
            observed.exit.starts_with(kind) && reference.exit.contains(kind),
            "{name}: set {} / bare {}",
            observed.exit,
            reference.exit
        );
        assert_eq!(observed.x87, reference.x87, "{name}");
        assert_eq!(observed.memory, reference.memory, "{name}");
        assert_eq!(observed.reads, reference.reads, "{name}");
        assert_eq!(observed.writes, reference.writes, "{name}");
        assert_eq!(after, before, "{name}: the cell is unchanged");
        assert_eq!(state.rip, CODE, "{name}: nothing retired");
        assert_eq!(state.gpr, observed.gpr, "{name}");
    }
}

#[test]
fn other_x87_forms_stay_refused_in_the_set_with_no_change() {
    for (name, code) in [
        ("fld dword [rax]", [0xd9u8, 0x00]),
        ("fldcw [rax]", [0xd9, 0x28]),
        ("fnstcw [rax]", [0xd9, 0x38]),
        ("fsqrt", [0xd9, 0xfa]),
        ("fprem", [0xd9, 0xf8]),
        ("fld1", [0xd9, 0xe8]),
        ("fst qword [rax]", [0xdd, 0x10]),
        ("fnclex", [0xdb, 0xe2]),
        ("fnstsw ax", [0xdf, 0xe0]),
    ] {
        let (observed, state, before, after) = in_set(&code, CR0_BASE, CR4_FP, &seed(), &data);
        assert!(
            observed.exit.starts_with("Unsupported"),
            "{name}: {}",
            observed.exit
        );
        assert_eq!(after, before, "{name}");
        assert_eq!(state.rip, CODE, "{name}");
        assert!(state.x87.is_none(), "{name}");
    }
}

const ORACLE: &str = include_str!("data/x87_env_oracle_i5_3210m.txt");

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[2 * index..2 * index + 2], 16).unwrap())
        .collect()
}

/// One instruction through the set on a shared cell and RAM, with RAX = address.
fn set_step(cell: &RefCell<GuestFpState>, ram: &mut Ram, code: [u8; 2], address: u64) {
    let mut state = base_state(CR0_BASE, CR4_FP);
    state.gpr[Register::Rax as usize] = address;
    ram.put(CODE, &code);
    let set = make_set(cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        ram,
        run_config(1),
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap();
    assert!(
        matches!(
            result.exit,
            Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
        ),
        "{code:02x?}: {:?}",
        result.exit
    );
}

/// The 324 physical rows of S2-32c (Core i5-3210M: FNINIT; FRSTOR in; FNSTENV env; FNSAVE save; FNSTENV env2),
/// replayed instruction by instruction through the CPL0 set: every byte of the three outputs, the one write per
/// image and the one 108-byte read are those of the bare run and of the processor.
#[test]
fn the_oracle_rows_of_s2_32c_are_reproduced_byte_for_byte_through_the_set() {
    const IN: u64 = 0x4000;
    const ENV: u64 = 0x5000;
    const SAVE: u64 = 0x6000;
    const ENV2: u64 = 0x7000;
    let mut rows = 0;
    for line in ORACLE.lines() {
        let fields: Vec<Vec<u8>> = line.split_whitespace().map(hex).collect();
        let [image, env, save, env2] = &fields[..] else {
            panic!("{line}")
        };
        rows += 1;
        // Both selector states known (the set requires it); the image overwrites the whole environment.
        for selectors in [(0, 0), (0x5a5a, 0xa5a5)] {
            let mut x87 = initial(Some(0));
            x87.instruction_selector = Some(selectors.0);
            x87.data_selector = Some(selectors.1);
            let cell = RefCell::new(GuestFpState {
                x87,
                sse: cpu().sse.unwrap(),
            });
            let mut ram = Ram::new(&[]);
            ram.put(IN, image);
            set_step(&cell, &mut ram, FRSTOR, IN);
            assert_eq!(ram.reads, [(IN, 108)], "one 108-byte read: {line}");
            set_step(&cell, &mut ram, FNSTENV, ENV);
            assert_eq!(ram.get(ENV, 28), &env[..], "env {line}");
            set_step(&cell, &mut ram, FNSAVE, SAVE);
            assert_eq!(ram.get(SAVE, 108), &save[..], "save {line}");
            set_step(&cell, &mut ram, FNSTENV, ENV2);
            assert_eq!(ram.get(ENV2, 28), &env2[..], "env2 {line}");
            assert_eq!(ram.writes, [(ENV, 28), (SAVE, 108), (ENV2, 28)], "{line}");
            let x87 = cell.borrow().x87.clone();
            assert_eq!(
                (x87.control_word, x87.status_word, x87.tag_word),
                (0x037f, 0, 0xffff),
                "FNSAVE re-initialised {line}"
            );
            assert_eq!(
                (x87.instruction_selector, x87.data_selector),
                (Some(0), Some(0))
            );
        }
    }
    assert_eq!(rows, 324);
}
