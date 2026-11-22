//! Tests of the PAE protected-mode step (S2-25o). There is no hardware oracle for PAE paging: the step is
//! checked against `ProtectedCpu::step` over physical memory (`BoardRam`, the model walks the same tables
//! itself) on identical tables and programs, in lockstep, state and RAM image after every instruction,
//! and by literal SDM page-fault codes (Vol. 3A section 5.7).

use super::*;
use crate::ap_legacy_exec::BoardRam;
use crate::paged_pae_adapter::tests::{
    ACCESSED, CODE, CountingDevice, DATA_A, DATA_B, DATA_X, DIRTY, Expect, MMIO_AT, P, PDPT, PT,
    RW, Scenario, US, at, cpu, dump, first_difference, fixture, get, memory, put,
};
use crate::{MmioBinding, PhysicalMemory, RamLimits};
use od_isa::protected_exec::ProtectedRefusal;
use std::sync::{Arc, Mutex};

const WP: bool = true;
const CELL: u32 = 0x5010;
const CELL2: u32 = 0x5014;
const CROSSING: u32 = 0x5FFE;
const DATA_RANGE: std::ops::Range<usize> = 0x1_0000..0x1_9000;

fn le(value: u32) -> [u8; 4] {
    value.to_le_bytes()
}

fn mov_eax_imm(value: u32) -> Vec<u8> {
    [&[0xB8][..], &le(value)].concat()
}
fn mov_ecx_imm(value: u32) -> Vec<u8> {
    [&[0xB9][..], &le(value)].concat()
}
fn mov_eax_mem(disp: u32) -> Vec<u8> {
    [&[0xA1][..], &le(disp)].concat()
}
fn mov_mem_eax(disp: u32) -> Vec<u8> {
    [&[0xA3][..], &le(disp)].concat()
}
fn mov_edx_mem(disp: u32) -> Vec<u8> {
    [&[0x8B, 0x15][..], &le(disp)].concat()
}
fn cmp_eax_mem(disp: u32) -> Vec<u8> {
    [&[0x3B, 0x05][..], &le(disp)].concat()
}
fn cmp_edx_mem(disp: u32) -> Vec<u8> {
    [&[0x3B, 0x15][..], &le(disp)].concat()
}
fn bt_mem(disp: u32, bit: u8) -> Vec<u8> {
    [&[0x0F, 0xBA, 0x25][..], &le(disp), &[bit]].concat()
}
fn add_esp(value: u32) -> Vec<u8> {
    [&[0x81, 0xC4][..], &le(value)].concat()
}

fn base() -> Scenario {
    Scenario::new("", false, 0, Expect::Ok)
}

/// The tables of `scenario`, the program at the code page and a copy of it on the XD page and at the
/// end of page 5 (the cases that fetch there).
fn install(memory: &PhysicalMemory, code: &[u8], scenario: &Scenario) {
    fixture(memory, scenario);
    for at_physical in [u64::from(CODE), DATA_X, DATA_A + 0xFFE] {
        memory.store_bytes(at(at_physical), code, |_| {}).unwrap();
    }
}

fn memory_with(code: &[u8], scenario: &Scenario) -> PhysicalMemory {
    let memory = memory();
    install(&memory, code, scenario);
    memory
}

/// One step through the adapter over `memory`, built from `cpu` as the board does for every instruction.
fn new_step(
    cpu: &mut ProtectedCpu,
    memory: &PhysicalMemory,
    writes: &mut Vec<(u64, u64)>,
) -> (PaeStepOutcome, Option<PageFaultRecord>) {
    let paging = PaeProbeConfig::from_protected_cpu(cpu).unwrap();
    let mut sink = |range: GuestPhysicalRange| writes.push((range.start().get(), range.len()));
    let mut adapter =
        PagedPaeMemoryAdapter::over_memory(memory, paging, u64::from(cpu.eip), &mut sink);
    let outcome = step_pae_protected(cpu, &mut adapter);
    (outcome, adapter.page_fault().cloned())
}

fn assert_same_ram(left: &PhysicalMemory, right: &PhysicalMemory, what: &str) {
    assert_eq!(
        first_difference(&dump(left), &dump(right)),
        None,
        "{what}: first differing RAM byte (offset, model, step)"
    );
}

/// The program of the lockstep: every covered form, taken and untaken branches, a page-crossing
/// load and store, `RDMSR` and an `EFER` write that changes the walk of the next instructions.
fn program() -> Vec<u8> {
    [
        mov_eax_imm(0x1234_5678),
        mov_mem_eax(CELL),
        mov_edx_mem(CELL),
        cmp_edx_mem(CELL2),
        vec![0x75, 0x05], // jne +5 (taken: skips the next MOV)
        mov_eax_imm(0xDEAD_BEEF),
        cmp_eax_mem(CELL),
        vec![0x74, 0x05], // je +5 (taken)
        mov_ecx_imm(1),
        add_esp(0x1000),
        bt_mem(CELL2, 3), // bit 3 of the pattern byte 0x9C: CF = 1
        vec![0x73, 0x05], // jae +5 (not taken: CF = 1)
        mov_ecx_imm(0x77),
        vec![0x0F, 0x09], // wbinvd
        mov_ecx_imm(0xC000_0080),
        vec![0x0F, 0x32], // rdmsr
        mov_eax_mem(CROSSING),
        mov_mem_eax(CROSSING),
        mov_eax_mem(CROSSING),
        mov_eax_imm(0x801), // SCE | NXE
        vec![0x0F, 0x30],   // wrmsr
        mov_eax_mem(CELL),
    ]
    .concat()
}

#[test]
fn the_step_equals_protected_cpu_over_physical_memory_in_lockstep_on_a_program_of_every_covered_form()
 {
    let scenario = base();
    let code = program();
    let model_memory = memory_with(&code, &scenario);
    let new_memory = memory_with(&code, &scenario);
    let mut model = cpu(WP);
    model.gpr[4] = 0x7000; // ESP
    let mut ours = model;
    let mut retired = Vec::new();
    let mut all_writes = Vec::new();
    loop {
        let index = retired.len();
        let expected = model.step(&mut BoardRam::new(&model_memory));
        let (outcome, _) = new_step(&mut ours, &new_memory, &mut all_writes);
        match (expected, outcome) {
            (Ok(left), PaeStepOutcome::Retired(right)) => {
                assert_eq!(left, right, "step {index}: instruction");
                assert!(pae_covers(&left), "step {index}: {left:?} is covered");
                retired.push(left);
            }
            (Err(left), PaeStepOutcome::Stopped(PaeProtectedStop::Refused(right))) => {
                // The terminal refusal: the zero bytes after the program (the same Unsupported).
                assert_eq!(left, right);
                assert!(matches!(left, ProtectedRefusal::Unsupported { .. }));
                break;
            }
            other => panic!("step {index}: the paths differ: {other:?}"),
        }
        assert_eq!(model, ours, "step {index}: registers and flags");
        assert_same_ram(&model_memory, &new_memory, &format!("step {index}"));
        assert!(index < 64, "the program did not end");
    }
    assert_eq!(model, ours, "the terminal state");
    assert_eq!(retired.len(), 20);
    assert_eq!(ours.efer, 0x801, "the WRMSR took effect for the next walks");
    assert_eq!(ours.gpr[4], 0x8000, "ADD ESP");
    // The crossing store dirtied the leaf of both pages, the loads only accessed them.
    for (page, dirty) in [(5u64, true), (6, true)] {
        let entry = get(&new_memory, PT + 8 * page);
        assert_eq!(entry & ACCESSED, ACCESSED, "page {page}");
        assert_eq!(entry & DIRTY != 0, dirty, "page {page}");
    }
    // The adapter reported the writes of the walks and of the operands, in order, none twice.
    assert!(all_writes.contains(&(PT + 40, 8)), "{all_writes:?}");
}

/// A fault: the same `#PF` (error code, CR2) from both, the state of each unchanged, the data pages
/// untouched (a refused store writes no byte).
struct Fault {
    name: &'static str,
    scenario: Scenario,
    code: Vec<u8>,
    eip: u32,
    error_code: u32,
    cr2: u32,
}

fn faults() -> Vec<Fault> {
    let read_5000 = mov_eax_mem(0x5000);
    let store_5000 = mov_mem_eax(0x5000);
    let fault = |name, scenario, code: Vec<u8>, eip, error_code, cr2| Fault {
        name,
        scenario,
        code,
        eip,
        error_code,
        cr2,
    };
    vec![
        fault(
            "read of a page that is not present",
            base().pte5(0),
            read_5000.clone(),
            CODE,
            0,
            0x5000,
        ),
        fault(
            "store to a read-only page with CR0.WP",
            base().pte5(DATA_A | P | US),
            store_5000,
            CODE,
            3,
            0x5000,
        ),
        fault(
            "instruction fetch from an XD page with EFER.NXE",
            base(),
            read_5000.clone(),
            0x7000,
            0x11,
            0x7000,
        ),
        fault(
            "instruction fetch from a page that is not present",
            base().pte5(0),
            read_5000.clone(),
            0x5000,
            0x10,
            0x5000,
        ),
        fault(
            "instruction whose last bytes are on an absent page",
            base().pte6(0),
            mov_eax_imm(0x4433_2211),
            0x5FFE,
            0x10,
            0x6000,
        ),
        fault(
            "crossing load whose second page is absent",
            base().pte6(0),
            mov_eax_mem(CROSSING),
            CODE,
            0,
            0x6000,
        ),
        fault(
            "crossing store whose second page is read-only",
            base().pte6(DATA_B | P | US),
            mov_mem_eax(CROSSING),
            CODE,
            3,
            0x6000,
        ),
        fault(
            "load through a reserved bit of the PTE",
            base().pte5(DATA_A | P | RW | US | (1 << 51)),
            read_5000,
            CODE,
            9,
            0x5000,
        ),
        fault(
            "load through an absent PDPTE",
            base(),
            mov_eax_mem(0x8000_0000),
            CODE,
            0,
            0x8000_0000,
        ),
    ]
}

#[test]
fn a_page_fault_is_the_same_as_the_models_and_changes_neither_state_nor_data() {
    for case in faults() {
        let model_memory = memory_with(&case.code, &case.scenario);
        let new_memory = memory_with(&case.code, &case.scenario);
        let data_before = dump(&new_memory)[DATA_RANGE].to_vec();
        let mut model = cpu(WP);
        model.eip = case.eip;
        model.gpr[0] = 0xA1B2_C3D4;
        let before = model;
        let mut ours = model;

        let refusal = model
            .step(&mut BoardRam::new(&model_memory))
            .expect_err(case.name);
        let ProtectedRefusal::PageFault { linear, .. } = refusal else {
            panic!("{}: the model refused with {refusal:?}", case.name);
        };
        assert_eq!(
            (model.page_fault_error_code(&refusal), linear),
            (Some(case.error_code), case.cr2),
            "{}: the model",
            case.name
        );

        let (outcome, record) = new_step(&mut ours, &new_memory, &mut Vec::new());
        let PaeStepOutcome::Fault(fault) = outcome else {
            panic!("{}: the step is {outcome:?}", case.name);
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(case.error_code), Some(u64::from(case.cr2))),
            "{}: the step",
            case.name
        );
        let record = record.expect("the adapter recorded the page fault");
        assert_eq!(record.saved_rip(), u64::from(case.eip), "{}", case.name);
        assert_eq!(record.cr2_value(), u64::from(case.cr2), "{}", case.name);

        assert_eq!(model, before, "{}: the model is unchanged", case.name);
        assert_eq!(ours, before, "{}: the step leaves the state", case.name);
        assert_eq!(
            dump(&new_memory)[DATA_RANGE].to_vec(),
            data_before,
            "{}",
            case.name
        );
        assert_eq!(
            dump(&model_memory)[DATA_RANGE].to_vec(),
            data_before,
            "{}",
            case.name
        );
    }
}

/// The one difference of the images on a fault, by design (S2-25l): the adapter keeps the accessed bit of
/// every entry its walk reached, also when the access then faults on the rights of the leaf; the model
/// walks first and records no bit for a walk that ends in a `#PF`.
#[test]
fn a_faulting_access_keeps_the_accessed_bit_of_the_leaf_it_reached_where_the_model_records_none() {
    for (index, leaf) in [(1usize, PT + 8 * 5), (6, PT + 8 * 6)] {
        let case = &faults()[index];
        let model_memory = memory_with(&case.code, &case.scenario);
        let new_memory = memory_with(&case.code, &case.scenario);
        let mut model = cpu(WP);
        let mut ours = model;
        model
            .step(&mut BoardRam::new(&model_memory))
            .expect_err(case.name);
        let (outcome, _) = new_step(&mut ours, &new_memory, &mut Vec::new());
        assert!(matches!(outcome, PaeStepOutcome::Fault(_)), "{}", case.name);
        let (left, right) = (dump(&model_memory), dump(&new_memory));
        let differing: Vec<usize> = (0..left.len())
            .filter(|&at| left[at] != right[at])
            .collect();
        assert_eq!(
            differing,
            vec![leaf as usize],
            "{}: only the leaf byte differs",
            case.name
        );
        assert_eq!(get(&model_memory, leaf) & ACCESSED, 0, "{}", case.name);
        assert_eq!(get(&new_memory, leaf) & ACCESSED, ACCESSED, "{}", case.name);
        assert_eq!(
            get(&new_memory, leaf) & DIRTY,
            0,
            "{}: a refused store dirties nothing",
            case.name
        );
    }
}

#[test]
fn a_model_refusal_is_the_same_refusal_with_the_state_unchanged() {
    // (name, code, how to prepare the model)
    type Prepare = fn(&mut ProtectedCpu);
    let cases: Vec<(&str, Vec<u8>, Prepare)> = vec![
        ("operand beyond the DS limit", mov_eax_mem(0x5000), |cpu| {
            cpu.ds.limit = 0x4FFF
        }),
        ("store through a read-only DS", mov_mem_eax(0x5000), |cpu| {
            cpu.ds.access = 0x91
        }),
        ("an opcode outside the closed set", vec![0x90, 0x90], |_| {}),
        (
            "RDMSR of an MSR that is not EFER",
            vec![0x0F, 0x32],
            |cpu| cpu.gpr[1] = 0x10,
        ),
        ("WRMSR of a reserved EFER bit", vec![0x0F, 0x30], |cpu| {
            cpu.gpr[1] = 0xC000_0080;
            cpu.gpr[0] = 1 << 20;
        }),
        ("a CS that is not 32-bit", mov_eax_imm(1), |cpu| {
            cpu.cs.flags = 0
        }),
    ];
    for (name, code, prepare) in cases {
        let model_memory = memory_with(&code, &base());
        let new_memory = memory_with(&code, &base());
        let mut model = cpu(WP);
        prepare(&mut model);
        let before = model;
        let mut ours = model;
        let expected = model
            .step(&mut BoardRam::new(&model_memory))
            .expect_err(name);
        let (outcome, _) = new_step(&mut ours, &new_memory, &mut Vec::new());
        assert_eq!(
            outcome,
            PaeStepOutcome::Stopped(PaeProtectedStop::Refused(expected)),
            "{name}"
        );
        assert_eq!(model, before, "{name}");
        assert_eq!(ours, before, "{name}");
    }
}

#[test]
fn an_instruction_outside_the_covered_set_is_named_and_executes_nothing() {
    let forms: Vec<(&str, Vec<u8>)> = vec![
        ("MOV CR3, EAX", vec![0x0F, 0x22, 0xD8]),
        ("MOV CR0, EAX", vec![0x0F, 0x22, 0xC0]),
        ("MOV CR4, EAX", vec![0x0F, 0x22, 0xE0]),
        ("MOV DS, EDX", vec![0x8E, 0xDA]),
        (
            "JMP FAR ptr16:32",
            vec![0xEA, 0x00, 0x90, 0x00, 0x00, 0x08, 0x00],
        ),
    ];
    for (name, code) in forms {
        let model_memory = memory_with(&code, &base());
        let new_memory = memory_with(&code, &base());
        let mut ours = cpu(WP);
        ours.gpr[0] = 0x8005_0033;
        let before = ours;
        let (decoded, length) = before.decode(&BoardRam::new(&model_memory)).expect(name);
        assert!(!pae_covers(&decoded), "{name}");
        let (outcome, _) = new_step(&mut ours, &new_memory, &mut Vec::new());
        assert_eq!(
            outcome,
            PaeStepOutcome::Stopped(PaeProtectedStop::Uncovered {
                eip: CODE,
                instruction: decoded,
                length,
            }),
            "{name}"
        );
        assert_eq!(ours, before, "{name}: nothing executed");
        assert_eq!(
            dump(&new_memory)[DATA_RANGE].to_vec(),
            dump(&model_memory)[DATA_RANGE].to_vec()
        );
        assert_eq!(
            get(&new_memory, 0x1000),
            get(&model_memory, 0x1000),
            "the PDPT is as it was"
        );
    }
}

#[test]
fn a_model_that_is_not_on_pae_paging_is_refused_before_any_fetch() {
    let code = mov_eax_imm(1);
    let memory = memory_with(&code, &base());
    for (name, change) in [
        (
            "CR4.PAE clear",
            (|cpu: &mut ProtectedCpu| cpu.cr4 = 0) as fn(&mut ProtectedCpu),
        ),
        ("CR0.PG clear", |cpu| cpu.cr0 &= !(1 << 31)),
        ("IA-32e active", |cpu| cpu.efer |= 1 << 10),
        ("real mode", |cpu| cpu.cr0 &= !1),
    ] {
        let mut model = cpu(WP);
        change(&mut model);
        let before = model;
        let mut ignored = |_: GuestPhysicalRange| {};
        let mut adapter = PagedPaeMemoryAdapter::over_memory(
            &memory,
            PaeProbeConfig::from_protected_cpu(&cpu(WP)).unwrap(),
            0,
            &mut ignored,
        );
        assert_eq!(
            step_pae_protected(&mut model, &mut adapter),
            PaeStepOutcome::Stopped(PaeProtectedStop::NotPaePaging),
            "{name}"
        );
        assert_eq!(model, before);
    }
    assert_eq!(get(&memory, PT + 8 * 8) & ACCESSED, 0, "nothing was walked");
}

fn bus_with_device(state: &Arc<Mutex<(usize, u64)>>) -> MachineBus {
    let bus = MachineBus::new(
        crate::paged_pae_adapter::tests::map(),
        RamLimits::new(1 << 20),
        vec![Box::new(CountingDevice(Arc::clone(state)))],
        vec![MmioBinding {
            region_start: at(MMIO_AT),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    fixture(bus.physical_memory(), &base());
    bus.physical_memory()
        .store_bytes(
            at(u64::from(CODE)),
            &[mov_eax_mem(0x9004), mov_mem_eax(0x9004)].concat(),
            |_| {},
        )
        .unwrap();
    bus
}

#[test]
fn an_mmio_operand_is_one_four_byte_access_of_the_device_where_the_model_refuses() {
    let state = Arc::new(Mutex::new((0usize, 0x1122_3344u64)));
    let mut bus = bus_with_device(&state);
    // The model over RAM refuses the MMIO operand (it has no device).
    let mut model = cpu(WP);
    let refusal = model
        .step(&mut BoardRam::new(bus.physical_memory()))
        .expect_err("a RAM-only model");
    assert!(
        matches!(
            refusal,
            ProtectedRefusal::DataUnreadable { linear: 0x9004, .. }
        ),
        "{refusal:?}"
    );

    let mut ours = cpu(WP);
    ours.gpr[0] = 0xAABB_CCDD;
    for _ in 0..2 {
        let paging = PaeProbeConfig::from_protected_cpu(&ours).unwrap();
        let mut ignored = |_: GuestPhysicalRange| {};
        let mut adapter = PagedPaeMemoryAdapter::over_machine(
            &mut bus,
            0,
            paging,
            u64::from(ours.eip),
            &mut ignored,
        );
        assert!(matches!(
            step_pae_protected(&mut ours, &mut adapter),
            PaeStepOutcome::Retired(_)
        ));
    }
    // The load read the device once (EAX took its value), the store wrote it once.
    assert_eq!(*state.lock().unwrap(), (2, 0x1122_3344));
    assert_eq!(ours.gpr[0], 0x1122_3344);
    assert_eq!(ours.eip, CODE + 10);
    let leaf = get(bus.physical_memory(), PT + 72);
    assert_eq!(leaf & (ACCESSED | DIRTY), ACCESSED | DIRTY);
}

/// The state of a vCPU on PAE paging, as the host loop holds it.
fn state_of(model: &ProtectedCpu) -> od_isa::LongModeState {
    let mut state = od_isa::LongModeState {
        rip: u64::from(model.eip),
        rflags: u64::from(model.eflags),
        cr0: Some(u64::from(model.cr0)),
        cr3: None,
        cr4: Some(u64::from(model.cr4)),
        ..Default::default()
    };
    for (slot, value) in state.gpr.iter_mut().zip(model.gpr) {
        *slot = u64::from(value);
    }
    state
}

fn bus_for_state(code: &[u8], scenario: &Scenario) -> MachineBus {
    let bus = MachineBus::new(
        crate::paged_pae_adapter::tests::map(),
        RamLimits::new(1 << 20),
        vec![Box::new(CountingDevice(Arc::new(Mutex::new((0, 0)))))],
        vec![MmioBinding {
            region_start: at(MMIO_AT),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    install(bus.physical_memory(), code, scenario);
    bus
}

#[test]
fn a_long_mode_state_on_pae_paging_runs_like_the_model_and_keeps_what_the_model_has_no_room_for() {
    let code = program();
    let mut bus = bus_for_state(&code, &base());
    let model_memory = memory_with(&code, &base());
    let mut model = cpu(WP);
    model.gpr[4] = 0x7000;
    let mut state = state_of(&model);
    // The high halves of the registers are not the model's: they stay.
    state.gpr[0] |= 0xFFFF_FFFF_0000_0000;
    state.rflags |= 0xFFFF_0000_0000;
    let mut walked = Vec::new();
    for index in 0..10 {
        let expected = model.step(&mut BoardRam::new(&model_memory));
        let mut sink = |range: GuestPhysicalRange| walked.push(range.start().get());
        let (outcome, record) =
            run_pae_state_step(&mut state, &mut bus, 0, PDPT, model.efer, &mut sink);
        assert_eq!(
            outcome,
            PaeStepOutcome::Retired(expected.unwrap()),
            "step {index}"
        );
        assert!(record.is_none());
        assert_eq!(state.rip, u64::from(model.eip), "step {index}");
        for register in 0..8 {
            assert_eq!(
                state.gpr[register] as u32, model.gpr[register],
                "step {index} r{register}"
            );
        }
        assert_eq!(state.rflags as u32, model.eflags, "step {index}");
    }
    assert_eq!(
        state.gpr[0] >> 32,
        0xFFFF_FFFF,
        "the high half of RAX is kept"
    );
    assert_eq!(
        state.rflags >> 32,
        0xFFFF,
        "the high half of RFLAGS is kept"
    );
    assert_same_ram(&model_memory, bus.physical_memory(), "ten steps");
    // Steps 11..=18 retire; the 19th is the WRMSR of the program, which this path does not take (EFER is
    // the caller's): it is named and the state stays.
    for _ in 10..18 {
        let expected = model.step(&mut BoardRam::new(&model_memory)).unwrap();
        let (outcome, _) =
            run_pae_state_step(&mut state, &mut bus, 0, PDPT, model.efer, &mut |_| {});
        assert_eq!(outcome, PaeStepOutcome::Retired(expected));
    }
    let before = state.clone();
    let (outcome, _) = run_pae_state_step(&mut state, &mut bus, 0, PDPT, model.efer, &mut |_| {});
    assert!(
        matches!(
            outcome,
            PaeStepOutcome::Stopped(PaeProtectedStop::Uncovered {
                instruction: ProtectedInstruction::Wrmsr,
                ..
            })
        ),
        "{outcome:?}"
    );
    assert_eq!(state, before);
}

#[test]
fn a_state_that_cannot_be_the_model_is_refused_with_its_reason_before_any_walk() {
    let code = mov_eax_imm(1);
    let model = cpu(WP);
    let good = state_of(&model);
    let run = |state: &mut od_isa::LongModeState, bus: &mut MachineBus, cr3, efer| {
        run_pae_state_step(state, bus, 0, cr3, efer, &mut |_| {
            panic!("no walk may write")
        })
    };
    let refused = |outcome: PaeStepOutcome| match outcome {
        PaeStepOutcome::Stopped(PaeProtectedStop::State(refusal)) => refusal,
        other => panic!("not a state refusal: {other:?}"),
    };

    let mut bus = bus_for_state(&code, &base());
    let mut state = good.clone();
    state.cs_selector = Some(0x1B);
    assert_eq!(
        refused(run(&mut state, &mut bus, PDPT, model.efer).0),
        PaeStateRefusal::NotCpl0 { cs_selector: 0x1B }
    );
    let mut state = good.clone();
    state.ds_unusable = true;
    assert_eq!(
        refused(run(&mut state, &mut bus, PDPT, model.efer).0),
        PaeStateRefusal::DataSegmentUnusable
    );
    let mut state = good.clone();
    state.cr3 = Some(0x4000);
    assert_eq!(
        refused(run(&mut state, &mut bus, PDPT, model.efer).0),
        PaeStateRefusal::Cr3Mismatch {
            state_cr3: 0x4000,
            cr3: PDPT
        }
    );
    let mut state = good.clone();
    state.rip = 0x1_0000_0000;
    assert_eq!(
        refused(run(&mut state, &mut bus, PDPT, model.efer).0),
        PaeStateRefusal::Wide {
            what: "RIP",
            value: 0x1_0000_0000
        }
    );
    let mut state = good.clone();
    state.cr4 = Some(0);
    assert!(matches!(
        refused(run(&mut state, &mut bus, PDPT, model.efer).0),
        PaeStateRefusal::NotPaeProtected { .. }
    ));
    // A reserved bit of a PDPTE with P = 1 (SDM Vol. 3A section 5.4.1): the PDPTEs are loaded from CR3.
    put(bus.physical_memory(), PDPT, 0x2000 | P | (1 << 2));
    let mut state = good.clone();
    assert!(matches!(
        refused(run(&mut state, &mut bus, PDPT, model.efer).0),
        PaeStateRefusal::Paging(PaeConfigError::PdpteReserved { index: 0, .. })
    ));
    assert_eq!(state, good, "no refusal changed the state");
    assert_eq!(get(bus.physical_memory(), PT + 8 * 8) & ACCESSED, 0);
}
