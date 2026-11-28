//! S7-32, ADR-0023 slice C1, plan step C1-5a: `IRETQ` (`48 CF`) from CPL3 to CPL3 in the set, against
//! a model written from the Intel SDM IRET page (Vol. 2A "IRET/IRETD/IRETQ", the IA-32e-MODE and
//! RETURN-TO-SAME-PRIVILEGE-LEVEL pseudocode and the "64-Bit Mode Exceptions" list; read from the
//! felixcloutier.com copy of the SDM text):
//!
//! - NT = 1 is #GP(0) before the frame is popped;
//! - RIP, CS, RFLAGS are popped and CS is loaded: null #GP(0); RPL < CPL #GP(sel); index beyond
//!   the GDT limit #GP(sel); not code #GP(sel); D ∧ L #GP(sel); nonconforming DPL ≠ RPL or conforming
//!   DPL > RPL #GP(sel); not present #NP(sel);
//! - RSP, SS are popped and SS is loaded: null #GP(0) (back to CPL3 64-bit mode); RPL ≠ RPL(CS)
//!   #GP(sel); beyond the limit #GP(sel); not a writable data segment #GP(sel); DPL ≠ RPL(CS)
//!   #GP(sel); not present #SS(0);
//! - a non-canonical RIP is #GP(0);
//! - the flags: CF, PF, AF, ZF, SF, TF, DF, OF, NT, RF, AC, ID from the frame; IF only if CPL <= IOPL;
//!   IOPL, VIF, VIP never (CPL ≠ 0).
//!
//! The order inside one segment load is the choice of the model (the list does not order it) and is
//! the one documented on `iretq_same_level_cpl3`. Not modeled (an honest `Unsupported`, nothing
//! changed): TF in the frame, a TI = 1 selector, a CS with L = 0 (compatibility mode).

mod cpl3_sweep;

use cpl3_sweep::{
    CR4, Cells, GDT, STACK_PAGE, SweepMemory, USER_CODE32, USER_CODE64, USER_DATA, machine,
    page_fault, run_exit, run_on,
};
use od_isa::execute::RunExit;
use od_isa::{ArchitecturalFault, LongModeState, Register};

const NT: u64 = 1 << 14;
const IF: u64 = 1 << 9;
const TF: u64 = 1 << 8;
const IOPL: u64 = 3 << 12;
const ACCESSED: u64 = 1 << 40;
/// The frame: RIP, CS, RFLAGS, RSP, SS (from the RSP of the machine minus 0x100).
const FRAME_RSP: u64 = STACK_PAGE + 0xe00;
const TARGET: u64 = 0x0000_7ff6_1234_5000;
const NEW_RSP: u64 = 0x0000_00e1_2345_6780;

/// A case: the frame, the GDT qwords (index 0..=10) and the RFLAGS of the state.
#[derive(Clone)]
struct Case {
    frame: [u64; 5],
    gdt: Vec<u64>,
    rflags: u64,
}

fn valid(iopl: u8) -> Case {
    let (state, memory) = machine(3, iopl, CR4);
    let gdt = (0..11)
        .map(|index| memory.peek_u64(GDT + 8 * index).unwrap())
        .collect();
    Case {
        frame: [TARGET, 0x33, 2 | 0x8d5, NEW_RSP, 0x2b],
        gdt,
        rflags: state.rflags,
    }
}

fn run(case: &Case) -> cpl3_sweep::Run {
    let (mut state, mut memory) = machine(3, ((case.rflags >> 12) & 3) as u8, CR4);
    state.rflags = case.rflags;
    state.gpr[Register::Rsp as usize] = FRAME_RSP;
    for (index, value) in case.frame.iter().enumerate() {
        memory.put_u64(FRAME_RSP + 8 * index as u64, *value);
    }
    for (index, value) in case.gdt.iter().enumerate() {
        memory.put_u64(GDT + 8 * index as u64, *value);
    }
    run_on(state, memory, Cells::new(), &[0x48, 0xcf], 1)
}

#[derive(Debug, PartialEq)]
enum Expected {
    Retired,
    Fault(u8, Option<u32>),
    NotModeled,
}

fn canonical(address: u64) -> bool {
    ((address as i64) << 16 >> 16) as u64 == address
}

/// The model of the SDM text above (GDT limit 0x57 of the machine).
fn model(case: &Case) -> Expected {
    let gp = |selector: u16| Expected::Fault(13, Some(u32::from(selector & 0xfffc)));
    let descriptor = |selector: u16| -> Option<u64> {
        let index = usize::from(selector >> 3);
        (u64::from(selector & !7) + 7 <= 0x57).then(|| case.gdt[index])
    };
    if case.rflags & NT != 0 {
        return Expected::Fault(13, Some(0));
    }
    let [rip, cs, rflags, _, ss] = case.frame;
    let (cs, ss) = (cs as u16, ss as u16);
    // CS
    if cs & 0xfffc == 0 {
        return Expected::Fault(13, Some(0));
    }
    if cs & 4 != 0 {
        return Expected::NotModeled;
    }
    let rpl = cs & 3;
    if rpl < 3 {
        return gp(cs);
    }
    let Some(code) = descriptor(cs) else {
        return gp(cs);
    };
    let access = (code >> 40) as u8;
    let (l, d) = (code >> 53 & 1 != 0, code >> 54 & 1 != 0);
    if access & 0x18 != 0x18 || (l && d) {
        return gp(cs);
    }
    let dpl = u16::from(access >> 5 & 3);
    let conforming = access & 4 != 0;
    if (conforming && dpl > rpl) || (!conforming && dpl != rpl) {
        return gp(cs);
    }
    if access & 0x80 == 0 {
        return Expected::Fault(11, Some(u32::from(cs & 0xfffc)));
    }
    if !l {
        return Expected::NotModeled;
    }
    // SS
    if ss & 0xfffc == 0 {
        return Expected::Fault(13, Some(0));
    }
    if ss & 4 != 0 {
        return Expected::NotModeled;
    }
    if ss & 3 != rpl {
        return gp(ss);
    }
    let Some(stack) = descriptor(ss) else {
        return gp(ss);
    };
    let access = (stack >> 40) as u8;
    let writable_data = access & 0x10 != 0 && access & 0x08 == 0 && access & 0x02 != 0;
    if !writable_data || u16::from(access >> 5 & 3) != rpl {
        return gp(ss);
    }
    if access & 0x80 == 0 {
        return Expected::Fault(12, Some(0));
    }
    if !canonical(rip) {
        return Expected::Fault(13, Some(0));
    }
    if rflags & TF != 0 {
        return Expected::NotModeled;
    }
    // A conforming CS with DPL < 3 is legal but leaves a CS cache the CPL3 entry does not admit:
    // not modeled (review A4).
    if dpl != 3 {
        return Expected::NotModeled;
    }
    Expected::Retired
}

fn outcome(run: &cpl3_sweep::Run) -> Expected {
    match run_exit(run.result.as_ref().unwrap()) {
        RunExit::InstructionBudgetExhausted => Expected::Retired,
        RunExit::Fault(ArchitecturalFault {
            vector, error_code, ..
        }) => Expected::Fault(*vector, *error_code),
        RunExit::Unsupported { opcode: 0xcf } => Expected::NotModeled,
        other => panic!("unexpected exit {other:?}"),
    }
}

fn gdt_bytes(memory: &SweepMemory) -> Vec<u8> {
    memory.peek(GDT, 0x58).unwrap()
}

/// The initial state and memory of a case, to compare a non-retired run with.
fn initial(case: &Case) -> (LongModeState, Vec<u8>) {
    let (mut state, mut memory) = machine(3, ((case.rflags >> 12) & 3) as u8, CR4);
    state.rflags = case.rflags;
    state.gpr[Register::Rsp as usize] = FRAME_RSP;
    for (index, value) in case.gdt.iter().enumerate() {
        memory.put_u64(GDT + 8 * index as u64, *value);
    }
    (state, gdt_bytes(&memory))
}

#[test]
fn iretq_cpl3_to_cpl3_commits_the_frame_and_sets_the_accessed_bits() {
    for accessed in [false, true] {
        let mut case = valid(0);
        for index in [5usize, 6] {
            case.gdt[index] = if accessed {
                case.gdt[index] | ACCESSED
            } else {
                case.gdt[index] & !ACCESSED
            };
        }
        let run = run(&case);
        assert_eq!(outcome(&run), Expected::Retired, "accessed {accessed}");
        assert_eq!(run.state.rip, TARGET);
        assert_eq!(run.state.gpr[Register::Rsp as usize], NEW_RSP);
        assert_eq!(run.state.cs_selector, Some(0x33));
        assert_eq!(run.state.ss_selector, Some(0x2b));
        assert_eq!(run.state.cs_cache.unwrap().access, 0xfb);
        assert_eq!(run.state.ss_cache.unwrap().access, 0xf3);
        let gdt = gdt_bytes(&run.memory);
        for index in 0..11usize {
            let value = u64::from_le_bytes(gdt[8 * index..8 * index + 8].try_into().unwrap());
            let expected = if matches!(index, 5 | 6) {
                case.gdt[index] | ACCESSED
            } else {
                case.gdt[index]
            };
            assert_eq!(value, expected, "GDT[{index:#x}] accessed {accessed}");
        }
        let descriptor_writes = run
            .memory
            .writes
            .iter()
            .filter(|(address, _)| (GDT..GDT + 0x58).contains(address))
            .count();
        assert_eq!(descriptor_writes, if accessed { 0 } else { 2 });
    }
}

#[test]
fn iretq_cpl3_if_and_iopl_follow_cpl_and_iopl() {
    for iopl in 0..4u8 {
        for frame_if in [0, IF] {
            for frame_iopl in 0..4u64 {
                let mut case = valid(iopl);
                case.rflags = (case.rflags & !IF) | (IF - frame_if);
                case.frame[2] = 2 | frame_if | (frame_iopl << 12) | 0x40;
                let run = run(&case);
                assert_eq!(outcome(&run), Expected::Retired);
                let expected_if = if iopl == 3 {
                    frame_if
                } else {
                    case.rflags & IF
                };
                assert_eq!(
                    run.state.rflags & IF,
                    expected_if,
                    "IOPL {iopl} frame IF {frame_if}"
                );
                assert_eq!(
                    run.state.rflags & IOPL,
                    u64::from(iopl) << 12,
                    "IOPL is never loaded"
                );
                assert_eq!(run.state.rflags & 0x8d5, 0x40);
            }
        }
    }
}

/// Every single violation of the list, then pairs of violations: the vector and error code of the
/// model, and for every non-retired outcome nothing changed (state, GDT, no write).
#[test]
fn iretq_cpl3_faults_in_the_order_of_the_pseudocode() {
    type Mutation = (&'static str, fn(&mut Case));
    let mutations: &[Mutation] = &[
        ("NT", |c| c.rflags |= NT),
        ("CS null", |c| c.frame[1] = 3),
        ("CS TI", |c| c.frame[1] = 0x37),
        ("CS RPL 0", |c| c.frame[1] = 0x30),
        ("CS beyond the limit", |c| c.frame[1] = 0x63),
        ("CS is data", |c| c.frame[1] = 0x2b),
        ("CS D and L", |c| c.gdt[6] |= 1 << 54),
        ("CS nonconforming DPL 0", |c| c.gdt[6] &= !(3 << 45)),
        ("CS conforming DPL 0 (legal)", |c| {
            c.gdt[6] = (c.gdt[6] & !(3 << 45)) | (1 << 42)
        }),
        ("CS not present", |c| c.gdt[6] &= !(1 << 47)),
        ("CS L = 0", |c| c.frame[1] = 0x23),
        ("SS null", |c| c.frame[4] = 3),
        ("SS TI", |c| c.frame[4] = 0x2f),
        ("SS RPL 0", |c| c.frame[4] = 0x28),
        ("SS beyond the limit", |c| c.frame[4] = 0x63),
        ("SS read-only", |c| c.gdt[5] &= !(1 << 41)),
        ("SS is code", |c| c.frame[4] = 0x33),
        ("SS DPL 0", |c| c.gdt[5] &= !(3 << 45)),
        ("SS not present", |c| c.gdt[5] &= !(1 << 47)),
        ("RIP non-canonical", |c| c.frame[0] = 0x0000_8000_0000_0000),
        ("TF in the frame", |c| c.frame[2] |= TF),
    ];
    let mut cases = Vec::new();
    for (index, (name, mutate)) in mutations.iter().enumerate() {
        let mut case = valid(0);
        mutate(&mut case);
        cases.push((name.to_string(), case));
        for (other, mutate_other) in &mutations[index + 1..] {
            let mut pair = valid(0);
            mutate(&mut pair);
            mutate_other(&mut pair);
            cases.push((format!("{name} + {other}"), pair));
        }
    }
    let mut faults = 0;
    for (name, case) in &cases {
        let expected = model(case);
        let run = run(case);
        assert_eq!(outcome(&run), expected, "{name}");
        if expected != Expected::Retired {
            let (state, gdt) = initial(case);
            assert_eq!(run.state, state, "{name}");
            assert_eq!(gdt_bytes(&run.memory), gdt, "{name}");
            assert!(run.memory.writes.is_empty(), "{name}");
            faults += 1;
        }
    }
    assert!(faults > 150, "{faults}");
}

#[test]
fn iretq_cpl3_return_to_cpl0_is_gp_selector() {
    let mut case = valid(0);
    case.frame[1] = 0x08;
    case.frame[4] = 0x10;
    let run = run(&case);
    assert_eq!(outcome(&run), Expected::Fault(13, Some(0x08)));
}

#[test]
fn iretq_cpl3_frame_on_a_supervisor_page_is_a_user_page_fault() {
    let case = valid(0);
    let (mut state, mut memory) = machine(3, 0, CR4);
    state.gpr[Register::Rsp as usize] = FRAME_RSP;
    memory.map(STACK_PAGE, false, true);
    for (index, value) in case.frame.iter().enumerate() {
        memory.put_u64(FRAME_RSP + 8 * index as u64, *value);
    }
    let before = state.clone();
    let run = run_on(state, memory, Cells::new(), &[0x48, 0xcf], 1);
    // A user read of a present supervisor page: P = 1, W = 0, U = 1.
    assert_eq!(
        run_exit(run.result.as_ref().unwrap()),
        &page_fault(0b101, FRAME_RSP)
    );
    assert_eq!(run.state, before);
}

#[test]
fn iretq_cpl3_reads_and_updates_descriptors_with_supervisor_rights() {
    // The GDT page of the machine is supervisor-only; with the Accessed bits clear the return reads
    // both descriptors and writes both back without a #PF.
    let mut case = valid(0);
    case.gdt[5] &= !ACCESSED;
    case.gdt[6] &= !ACCESSED;
    let run = run(&case);
    assert_eq!(outcome(&run), Expected::Retired);
    assert!(!run.memory.pages[&GDT].user);
    assert_eq!(run.state.cs_cache.unwrap().access & 1, 1);
}

#[test]
fn iretq_cpl3_with_a_compatibility_mode_cs_is_unsupported_without_change() {
    let mut case = valid(0);
    case.frame[1] = 0x23;
    assert_eq!(case.gdt[4], USER_CODE32);
    assert_eq!((case.gdt[5], case.gdt[6]), (USER_DATA, USER_CODE64));
    let run = run(&case);
    assert_eq!(outcome(&run), Expected::NotModeled);
    let (state, _) = initial(&case);
    assert_eq!(run.state, state);
    assert!(run.memory.writes.is_empty());
}

/// Review A2: RF of the frame is not kept (the model clears it, as `iretq_cpl0` on its return to
/// CPL3; ISSUE-723): a frame with RFLAGS 0x10202, then a NOP, leaves RFLAGS 0x202.
#[test]
fn iretq_cpl3_clears_rf_and_the_next_instruction_runs() {
    let mut case = valid(0);
    case.frame[0] = cpl3_sweep::RIP + 2;
    case.frame[2] = 0x10202;
    let (mut state, mut memory) = machine(3, 0, CR4);
    state.gpr[Register::Rsp as usize] = FRAME_RSP;
    for (index, value) in case.frame.iter().enumerate() {
        memory.put_u64(FRAME_RSP + 8 * index as u64, *value);
    }
    let run = run_on(state, memory, Cells::new(), &[0x48, 0xcf, 0x90], 2);
    let result = run.result.as_ref().unwrap();
    assert_eq!(run_exit(result), &RunExit::InstructionBudgetExhausted);
    assert_eq!(result.instructions_executed, 2);
    assert_eq!(run.state.rip, cpl3_sweep::RIP + 3);
    assert_eq!(run.state.rflags, 0x202);
}

/// Review A4: a conforming CS with DPL < 3 and RPL 3 passes the checks of the SDM but would leave a
/// CS cache with DPL < 3 (the next CPL3 entry refuses it): `Unsupported`, nothing changed.
#[test]
fn iretq_cpl3_to_a_conforming_cs_with_dpl_below_3_is_unsupported_without_change() {
    for dpl in 0..3u64 {
        let mut case = valid(0);
        case.gdt[6] = (case.gdt[6] & !(3 << 45)) | (dpl << 45) | (1 << 42);
        let run = run(&case);
        assert_eq!(outcome(&run), Expected::NotModeled, "DPL {dpl}");
        let (state, gdt) = initial(&case);
        assert_eq!(run.state, state);
        assert_eq!(gdt_bytes(&run.memory), gdt);
    }
}
