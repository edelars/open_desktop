//! S2-39 (CPL0 set): the MMX forms in the CPL0 profile set with the FP member. The MMX registers are the x87 data
//! registers, whose state lives in the member's cell (the x87 forms stay refused there); for the length of one MMX
//! instruction the set moves the x87 half of the cell into the CPU state, where the shared MMX core runs, and
//! stores it back. The expected values are the results of the same instruction in the bare run under the FP-init
//! CR0/CR4 pair (the S2-33 path validated against the physical oracle, extended by the S2-39 forms, see
//! s2_39_mmx.rs), plus literal vectors and the gate rules of Intel SDM Vol. 2A EMMS, Vol. 1 §9.6.1 and
//! Vol. 2A Table 2-21.

mod mmx_common;

use mmx_common::*;
use od_isa::execute::{
    self, ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cr4Profile, GuestFpState,
    RunConfig, RunExit, run_with_cpl0_profile_set,
};
use od_isa::state::{SseState, X87State};
use od_isa::{
    AccessContext, ArchitecturalFault, CpuActivity, LongModeState, PrivilegeLevel, Register,
    StiShadow, cpuid::CpuidProfile,
};
use std::cell::RefCell;

const CR4_FP: u64 = CR4_PAE | CR4_OSFXSR | (1 << 10);

fn context(privilege: PrivilegeLevel) -> AccessContext {
    ctx(privilege)
}

#[derive(Clone, Debug, PartialEq)]
struct Observed {
    exit: String,
    rip: u64,
    rflags: u64,
    gpr: [u64; 16],
    x87: X87State,
    sse: SseState,
    memory: Vec<u8>,
    reads: Vec<(u64, usize)>,
    writes: Vec<(u64, usize)>,
}

/// The x87 state of an x87 program (S2-33 seed) with known FCS/FDS, which the FP member requires.
fn seed() -> X87State {
    let mut x87 = cpu().x87.unwrap();
    x87.instruction_selector = Some(0x2b);
    x87.data_selector = Some(0x53);
    x87
}

/// The bare reference has no FCS/FDS (the FP-init pair does not carry them); the set requires them. They are
/// not touched by an MMX form, which the comparison of the cell with its seed checks separately.
fn without_selectors(mut x87: X87State) -> X87State {
    x87.instruction_selector = None;
    x87.data_selector = None;
    x87
}

fn describe(exit: &RunExit) -> String {
    format!("{exit:?}")
}

fn base_state(cr0: u64, cr4: u64) -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: FLAGS,
        cr0: Some(cr0),
        cr4: Some(cr4),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr = cpu().gpr;
    state
}

/// The bare reference: the FP-init CR0/CR4 pair over a state that carries x87 and SSE.
fn bare(
    code: &[u8],
    cr0: u64,
    cr4: u64,
    count: u64,
    prepare: &dyn Fn(&mut LongModeState, &mut Ram),
) -> Observed {
    let mut state = fp_init_cpu(cr0, cr4);
    let mut ram = Ram::new(code);
    prepare(&mut state, &mut ram);
    let result = execute::run_with_cr4_profile(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: count,
            stop_rip: None,
            context: context(PrivilegeLevel::Ring0),
        },
        Cr4Profile::FpInitLong64V1,
        || false,
    );
    let sse = state.sse.clone().unwrap();
    Observed {
        exit: describe(&result.exit),
        rip: state.rip,
        rflags: state.rflags,
        gpr: state.gpr,
        x87: without_selectors(state.x87.clone().unwrap()),
        sse,
        memory: ram.bytes[DATA as usize..DATA as usize + 0x100].to_vec(),
        reads: ram.reads.clone(),
        writes: ram.writes.clone(),
    }
}

fn fresh_cell() -> RefCell<GuestFpState> {
    RefCell::new(GuestFpState {
        x87: seed(),
        sse: cpu().sse.unwrap(),
    })
}

fn in_set(
    code: &[u8],
    cr0: u64,
    cr4: u64,
    count: u64,
    privilege: PrivilegeLevel,
    prepare: &dyn Fn(&mut LongModeState, &mut Ram),
) -> (Observed, LongModeState, GuestFpState, GuestFpState) {
    let mut state = base_state(cr0, cr4);
    let mut ram = Ram::new(code);
    prepare(&mut state, &mut ram);
    let cell = fresh_cell();
    let before = cell.borrow().clone();
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
        .with_fp_state(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: count,
            stop_rip: None,
            context: context(privilege),
        },
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
        exit: describe(&exit),
        rip: state.rip,
        rflags: state.rflags,
        gpr: state.gpr,
        x87: without_selectors(after.x87.clone()),
        sse: after.sse.clone(),
        memory: ram.bytes[DATA as usize..DATA as usize + 0x100].to_vec(),
        reads: ram.reads.clone(),
        writes: ram.writes.clone(),
    };
    (observed, state, before, after)
}

fn data(state: &mut LongModeState, ram: &mut Ram) {
    state.gpr[Register::Rax as usize] = DATA;
    ram.put(DATA, &0x0807_0605_0403_0201u64.to_le_bytes());
    ram.put(DATA + 8, &0xf0e0_d0c0_b0a0_9080u64.to_le_bytes());
}

const FORMS: &[(&str, &[u8])] = &[
    ("paddb mm0,mm1", &[0x0f, 0xfc, 0xc1]),
    ("paddsw mm2,mm3", &[0x0f, 0xed, 0xd3]),
    ("psubusb mm7,mm6", &[0x0f, 0xd8, 0xfe]),
    ("pand mm1,mm1", &[0x0f, 0xdb, 0xc9]),
    ("pandn mm4,mm5", &[0x0f, 0xdf, 0xe5]),
    ("por mm0,[rax]", &[0x0f, 0xeb, 0x00]),
    ("pxor mm3,mm3", &[0x0f, 0xef, 0xdb]),
    ("pcmpeqb mm0,mm1", &[0x0f, 0x74, 0xc1]),
    ("pcmpgtw mm2,[rax+8]", &[0x0f, 0x65, 0x50, 0x08]),
    ("pmullw mm1,mm2", &[0x0f, 0xd5, 0xca]),
    ("pmulhw mm1,[rax]", &[0x0f, 0xe5, 0x08]),
    ("pmaddwd mm5,mm4", &[0x0f, 0xf5, 0xec]),
    ("punpcklbw mm0,mm1", &[0x0f, 0x60, 0xc1]),
    ("punpckhdq mm0,[rax]", &[0x0f, 0x6a, 0x00]),
    ("packsswb mm2,mm3", &[0x0f, 0x63, 0xd3]),
    ("packuswb mm0,[rax]", &[0x0f, 0x67, 0x00]),
    ("packssdw mm6,mm7", &[0x0f, 0x6b, 0xf7]),
    ("psllw mm1,mm2", &[0x0f, 0xf1, 0xca]),
    ("psrlq mm1,[rax]", &[0x0f, 0xd3, 0x08]),
    ("psrad mm0,mm1", &[0x0f, 0xe2, 0xc1]),
    ("psllw mm4,3", &[0x0f, 0x71, 0xf4, 0x03]),
    ("psraw mm4,9", &[0x0f, 0x71, 0xe4, 0x09]),
    ("psrld mm5,20", &[0x0f, 0x72, 0xd5, 0x14]),
    ("psllq mm6,70", &[0x0f, 0x73, 0xf6, 0x46]),
    ("pshufw mm0,mm1,0x1b", &[0x0f, 0x70, 0xc1, 0x1b]),
    ("pshufw mm2,[rax],0xe4", &[0x0f, 0x70, 0x10, 0xe4]),
    ("movd mm1,eax", &[0x0f, 0x6e, 0xc8]),
    ("movq mm1,rax", &[0x48, 0x0f, 0x6e, 0xc8]),
    ("movd mm2,[rax]", &[0x0f, 0x6e, 0x10]),
    ("movd eax,mm1", &[0x0f, 0x7e, 0xc8]),
    ("movq rax,mm1", &[0x48, 0x0f, 0x7e, 0xc8]),
    ("movd [rax+4],mm1", &[0x0f, 0x7e, 0x48, 0x04]),
    ("movq mm1,mm2", &[0x0f, 0x6f, 0xca]),
    ("movq mm1,[rax+8]", &[0x0f, 0x6f, 0x48, 0x08]),
    ("movq [rax+8],mm1", &[0x0f, 0x7f, 0x48, 0x08]),
    ("movq mm3,mm4 (store form)", &[0x0f, 0x7f, 0xe3]),
    ("movq [rax],mm1 (REX.W ignored)", &[0x48, 0x0f, 0x7f, 0x08]),
    (
        "paddq-less psubd mm0,mm1 with 67",
        &[0x67, 0x0f, 0xfa, 0xc1],
    ),
    ("pssse3 pshufb mm0,mm1", &[0x0f, 0x38, 0x00, 0xc1]),
    ("pabsw mm0,[rax]", &[0x0f, 0x38, 0x1d, 0x00]),
    ("emms", &[0x0f, 0x77]),
];

#[test]
fn every_mmx_form_in_the_set_gives_the_result_of_the_bare_run() {
    for (name, code) in FORMS {
        let reference = bare(code, CR0_BASE, CR4_FP, 1, &data);
        let (observed, _, before, after) =
            in_set(code, CR0_BASE, CR4_FP, 1, PrivilegeLevel::Ring0, &data);
        assert_eq!(
            reference.exit, "InstructionBudgetExhausted",
            "{name}: the reference retires"
        );
        assert_eq!(observed, reference, "{name}");
        assert_ne!(
            after.x87, before.x87,
            "{name}: the x87 half of the cell takes the MMX state effect"
        );
        assert_eq!(after.sse, before.sse, "{name}: the SSE half is untouched");
    }
}

#[test]
fn a_program_of_mmx_forms_in_the_set_equals_the_bare_run_and_ends_with_the_emptied_tag_word() {
    // MOVD mm0,eax ; MOVQ mm1,[rax] ; PADDW mm0,mm1 ; PSLLQ mm0,8 ; PUNPCKLBW mm0,mm1 ; MOVQ [rax+8],mm0 ;
    // MOVD eax,mm0 ; EMMS
    let code: Vec<u8> = [
        &[0x0f, 0x6e, 0xc0][..],
        &[0x0f, 0x6f, 0x08],
        &[0x0f, 0xfd, 0xc1],
        &[0x0f, 0x73, 0xf0, 0x08],
        &[0x0f, 0x60, 0xc1],
        &[0x0f, 0x7f, 0x40, 0x08],
        &[0x0f, 0x7e, 0xc0],
        &[0x0f, 0x77],
    ]
    .concat();
    let reference = bare(&code, CR0_BASE, CR4_FP, 8, &data);
    let (observed, _, _, after) = in_set(&code, CR0_BASE, CR4_FP, 8, PrivilegeLevel::Ring0, &data);
    assert_eq!(reference.exit, "InstructionBudgetExhausted");
    assert_eq!(observed, reference);
    assert_eq!(after.x87.tag_word, 0xffff);
    assert_eq!(after.x87.status_word & (7 << 11), 0);
    // The registers keep their MMX payloads and exponent fields.
    assert_eq!(&after.x87.registers[0][8..], &[0xff, 0xff]);
}

#[test]
fn a_literal_result_in_the_set() {
    // PADDW mm0,mm1 with MM0 = 0x7fff_0001_ffff_8000 and MM1 = 0x0001_ffff_0001_8000: the word sums wrap to
    // 0x8000, 0x0000, 0x0000, 0x0000 (low word first: 0x8000 + 0x8000, 0xffff + 0x0001, 0x0001 + 0xffff,
    // 0x7fff + 0x0001), i.e. MM0 = 0x8000_0000_0000_0000; bits 79:64 of R0 become all ones.
    let mut seeded = seed();
    seeded.registers[0][..8].copy_from_slice(&0x7fff_0001_ffff_8000u64.to_le_bytes());
    seeded.registers[1][..8].copy_from_slice(&0x0001_ffff_0001_8000u64.to_le_bytes());
    let cell = RefCell::new(GuestFpState {
        x87: seeded,
        sse: cpu().sse.unwrap(),
    });
    let mut state = base_state(CR0_BASE, CR4_FP);
    let mut ram = Ram::new(&[0x0f, 0xfd, 0xc1]);
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
        .with_fp_state(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: context(PrivilegeLevel::Ring0),
        },
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
    let x87 = cell.borrow().x87.clone();
    assert_eq!(
        &x87.registers[0][..8],
        &0x8000_0000_0000_0000u64.to_le_bytes()
    );
    assert_eq!(&x87.registers[0][8..], &[0xff, 0xff]);
    assert_eq!(
        &x87.registers[1][..8],
        &0x0001_ffff_0001_8000u64.to_le_bytes(),
        "the source is not rewritten"
    );
    assert_eq!(
        x87.registers[1][8..],
        seed().registers[1][8..],
        "its exponent field is not rewritten either"
    );
    assert_eq!(x87.status_word & (7 << 11), 0, "TOP = 0");
    assert_eq!(x87.tag_word, 0, "every tag valid");
}

fn assert_fault_leaves_everything(
    code: &[u8],
    cr0: u64,
    cr4: u64,
    vector: u8,
    prepare: &dyn Fn(&mut LongModeState, &mut Ram, &mut X87State),
) {
    let mut state = base_state(cr0, cr4);
    let mut ram = Ram::new(code);
    data(&mut state, &mut ram);
    let mut seeded = seed();
    prepare(&mut state, &mut ram, &mut seeded);
    let cell = RefCell::new(GuestFpState {
        x87: seeded.clone(),
        sse: cpu().sse.unwrap(),
    });
    let before_state = state.clone();
    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
        .with_cpuid(&CpuidProfile::X8664V1Stage4A)
        .with_fp_state(&cell);
    let result = run_with_cpl0_profile_set(
        &mut state,
        &mut ram,
        RunConfig {
            instruction_budget: 1,
            stop_rip: None,
            context: context(PrivilegeLevel::Ring0),
        },
        od_isa::INTEGER_FLAGS,
        &set,
        None,
        || false,
    )
    .unwrap();
    let Cpl0ProfileSetExit::Run(RunExit::Fault(ArchitecturalFault { vector: got, .. })) =
        result.exit
    else {
        panic!("{code:02x?}: {:?}", result.exit)
    };
    assert_eq!(got, vector, "{code:02x?}");
    assert_eq!(
        cell.borrow().x87,
        seeded,
        "{code:02x?}: the cell changed on a fault"
    );
    assert_eq!(
        state, before_state,
        "{code:02x?}: the state changed on a fault"
    );
    assert!(ram.writes.is_empty(), "{code:02x?}: memory written");
}

#[test]
fn the_gates_of_every_mmx_form_in_the_set_are_ud_for_em_nm_for_ts_and_mf_for_a_pending_exception() {
    for (name, code) in FORMS {
        assert_fault_leaves_everything(code, CR0_BASE | 1 << 2, CR4_FP, 6, &|_, _, _| {});
        assert_fault_leaves_everything(code, CR0_BASE | 1 << 3, CR4_FP, 7, &|_, _, _| {});
        assert_fault_leaves_everything(code, CR0_BASE | 1 << 2 | 1 << 3, CR4_FP, 6, &|_, _, _| {});
        // IE pending and unmasked.
        assert_fault_leaves_everything(code, CR0_BASE, CR4_FP, 16, &|_, _, x87| {
            x87.control_word = 0x037e;
            x87.status_word = 0x0081 | 5 << 11;
        });
        // Without CR4.OSFXSR the MMX forms still run (Table 2-21 lists OSFXSR for legacy SSE only).
        let reference = bare(code, CR0_BASE, CR4_PAE, 1, &data);
        let (observed, ..) = in_set(code, CR0_BASE, CR4_PAE, 1, PrivilegeLevel::Ring0, &data);
        assert_eq!(observed, reference, "{name}: without OSFXSR");
        assert_eq!(observed.exit, "InstructionBudgetExhausted", "{name}");
    }
}

#[test]
fn a_failing_memory_access_in_the_set_leaves_the_cell_and_the_state_untouched() {
    for code in [
        &[0x0f, 0xfd, 0x00][..],
        &[0x0f, 0x6f, 0x00],
        &[0x0f, 0x7f, 0x00],
        &[0x0f, 0x7e, 0x00],
        &[0x0f, 0x6e, 0x00],
    ] {
        let mut state = base_state(CR0_BASE, CR4_FP);
        let mut ram = Ram::new(code);
        data(&mut state, &mut ram);
        ram.fault_page = Some(DATA);
        let cell = fresh_cell();
        let before = cell.borrow().clone();
        let state_before = state.clone();
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpuid(&CpuidProfile::X8664V1Stage4A)
            .with_fp_state(&cell);
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut ram,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: context(PrivilegeLevel::Ring0),
            },
            od_isa::INTEGER_FLAGS,
            &set,
            None,
            || false,
        )
        .unwrap();
        let Cpl0ProfileSetExit::Run(RunExit::Fault(fault)) = result.exit else {
            panic!("{code:02x?}: {:?}", result.exit)
        };
        assert_eq!(fault.vector, 14);
        assert_eq!(*cell.borrow(), before, "{code:02x?}");
        assert_eq!(state, state_before, "{code:02x?}");
        assert!(state.x87.is_none());
    }
}

#[test]
fn mmx_and_sse_forms_share_one_run_and_fxsave64_stores_the_mmx_state() {
    // MOVD mm0,eax ; XORPS xmm1,xmm2 ; FXSAVE64 [rax+0x40] ; both halves of the cell are visible in the image.
    let code: Vec<u8> = [
        &[0x0f, 0x6e, 0xc0][..],
        &[0x0f, 0x57, 0xca],
        &[0x48, 0x0f, 0xae, 0x40, 0x40],
    ]
    .concat();
    let prepare = |state: &mut LongModeState, _: &mut Ram| {
        state.gpr[Register::Rax as usize] = DATA;
    };
    let (observed, _, before, after) =
        in_set(&code, CR0_BASE, CR4_FP, 3, PrivilegeLevel::Ring0, &prepare);
    assert_eq!(observed.exit, "InstructionBudgetExhausted");
    // MM0 = low dword of RAX (the data address), exponent all ones, TOP = 0, every tag valid.
    assert_eq!(&after.x87.registers[0][..8], &DATA.to_le_bytes());
    assert_eq!(&after.x87.registers[0][8..], &[0xff, 0xff]);
    assert_eq!(after.x87.tag_word, 0);
    // XORPS xmm1,xmm2.
    let mut expected = before.sse.xmm[1];
    for (byte, other) in expected.iter_mut().zip(before.sse.xmm[2]) {
        *byte ^= other;
    }
    assert_eq!(after.sse.xmm[1], expected);
    // FXSAVE64 image at DATA+0x40: FSW TOP = 0, abridged tag 0xff (all valid), ST0 = R0 = MM0 with exponent 0xffff.
    let image = &observed.memory[0x40..0x40 + 0xc0];
    let fsw = u16::from_le_bytes([image[2], image[3]]);
    assert_eq!(fsw & (7 << 11), 0);
    assert_eq!(image[4], 0xff);
    assert_eq!(&image[32..40], &DATA.to_le_bytes());
    assert_eq!(&image[40..42], &[0xff, 0xff]);
}

#[test]
fn x87_forms_the_mmx_conversions_and_the_128_bit_integer_forms_stay_refused_in_the_set_with_no_change()
 {
    for (name, code) in [
        ("fistp dword [rax]", &[0xdb, 0x18][..]),
        ("fld dword [rax]", &[0xd9, 0x00]),
        ("fistt dword [rax]", &[0xdb, 0x08]),
        ("cvtpi2ps xmm1,mm2", &[0x0f, 0x2a, 0xca]),
        ("cvtps2pi mm1,xmm2", &[0x0f, 0x2d, 0xca]),
    ] {
        let (observed, state, before, after) =
            in_set(code, CR0_BASE, CR4_FP, 1, PrivilegeLevel::Ring0, &data);
        assert!(
            observed.exit.starts_with("Unsupported"),
            "{name}: {}",
            observed.exit
        );
        assert_eq!(after, before, "{name}: the cell changed");
        assert_eq!(observed.rip, CODE, "{name}");
        assert!(state.x87.is_none());
    }
}
