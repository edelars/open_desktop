//! S4-27i4a oracle. Every expected value is derived from the Intel SDM or from the accepted S4-27h
//! paged-delivery precedent, not from a run of the code under test:
//! - SDM Vol. 2B (STI): IF := 1; when IF was 0 before, maskable interrupts are inhibited until
//!   after the next instruction. With IF already 1 there is no inhibition.
//! - SDM Vol. 2A (HLT) and Vol. 3A 6.8: HLT retires (RIP advances past it) and the processor stops
//!   until an interrupt; the interrupt shadow of the STI ends with the instruction that follows it,
//!   so the waking interrupt saves the RIP after HLT. An interrupt at the completed boundary before
//!   HLT (IF=1, no shadow) saves the address of HLT.
//! - SDM Vol. 3A 6.14.2/6.14.3 (64-bit interrupt stack frame): RSP is aligned down to 16, SS, RSP,
//!   RFLAGS, CS, RIP are pushed (five qwords, no error code) and an interrupt gate clears IF in the
//!   live RFLAGS; the saved RFLAGS keeps IF.
//! - SDM Vol. 3A 13.8.4/13.9: IRR -> ISR on acknowledge; an EOI write of 0 clears the ISR bit.
//! - SDM Vol. 3A 4.8: A on use, D on the first write; a supervisor write to a read-only page with
//!   CR0.WP=1 is a #PF with P|W (error code 3) after the walk has set A.

use od_isa::{
    InterruptDeliveryPhase, InterruptFrameSlot, InterruptOutsideProfile, PagedProbeExit,
    PagedProbeRejection, Register, StiShadow,
};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, LoadedStage4PagedStiHltProbe, STAGE4_FLAT_PROBE_IMAGE_GPA,
    STAGE4_PAGED_STI_HLT_IMAGE_BYTES, Stage4FlatProbeLoadError, Stage4PagedStiHltEvent,
    Stage4PagedStiHltInjection, Stage4PagedStiHltReport, Stage4PagedStiHltScenario,
    Stage4PagedStiHltStep::*, Stage4PagedStiHltStop, load_stage4_paged_sti_hlt_probe,
    step_paged_hlt, step_paged_sti,
};

fn put(b: &mut [u8], gpa: u64, v: &[u8]) {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    b[o..o + v.len()].copy_from_slice(v)
}

/// Written from the profile description, independent of the loader's own table.
fn image() -> Vec<u8> {
    let mut b = vec![0; STAGE4_PAGED_STI_HLT_IMAGE_BYTES];
    for (g, v) in [
        (0x1000u64, 0x2023u64), // PML4[0] -> PDPT
        (0x2000, 0x3023),       // PDPT[0] -> PD
        (0x3010, 0x4023),       // PD[2] (GVA 4 MiB) -> code table
        (0x3018, 0x5023),       // PD[3] (GVA 6 MiB) -> system table
        (0x4008, 0x6003),       // GVA 0x401000 main
        (0x4010, 0x7003),       // GVA 0x402000 handler
        (0x5000, 0x8003),       // GVA 0x600000 IDT
        (0x5008, 0x9003),       // GVA 0x601000 GDT
        (0x5010, 0xfee0_0003),  // GVA 0x602000 Local APIC page
        (0x5800, 0xa003),       // GVA 0x700000 stack
    ] {
        put(&mut b, g, &v.to_le_bytes())
    }
    // STI; HLT; HLT.
    put(&mut b, 0x6000, &[0xfb, 0xf4, 0xf4]);
    // MOV [RBX],EAX; IRETQ.
    put(&mut b, 0x7000, &[0x89, 0x03, 0x48, 0xcf]);
    // Vector 0x41 gate -> 0x402000, CS 0x08, IST 0, present interrupt gate (type 0xE).
    put(
        &mut b,
        0x8000 + 0x41 * 16,
        &[
            0x00, 0x20, 0x08, 0x00, 0x00, 0x8e, 0x40, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    put(&mut b, 0x9008, &[0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0]);
    put(&mut b, 0x9010, &[0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0]);
    b
}

const MAIN: u64 = 0x40_1000;
const HLT1: u64 = 0x40_1001;
const HLT2: u64 = 0x40_1002;
const END: u64 = 0x40_1003;
const HANDLER: u64 = 0x40_2000;
const EOI_GVA: u64 = 0x60_20b0;
const RSP0: u64 = 0x70_0ff8;
/// RSP aligned down to 16 (0x700ff0) minus five qwords.
const FRAME: u64 = 0x70_0fc8;
const IF: u64 = 1 << 9;

fn load(scenario: Stage4PagedStiHltScenario) -> LoadedStage4PagedStiHltProbe {
    load_stage4_paged_sti_hlt_probe(&image(), scenario).unwrap()
}

fn run(
    scenario: Stage4PagedStiHltScenario,
    injection: Stage4PagedStiHltInjection,
) -> (LoadedStage4PagedStiHltProbe, Stage4PagedStiHltReport) {
    let mut m = load(scenario);
    m.inject(injection);
    let report = m.run(16, |_| {});
    (m, report)
}

fn pte(e: &Stage4PagedStiHltEvent) -> [u64; 6] {
    let p = e.ptes;
    [p.main, p.handler, p.idt, p.gdt, p.lapic, p.stack]
}

fn qword(m: &mut LoadedStage4PagedStiHltProbe, gpa: u64) -> u64 {
    m.bus_mut()
        .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
        .unwrap()
}

#[test]
fn entry_state_is_the_fixed_loader_handoff_per_scenario() {
    for (scenario, rflags) in [
        (Stage4PagedStiHltScenario::WakeFromHlt, 0x002),
        (Stage4PagedStiHltScenario::BoundaryBeforeHlt, 0x202),
    ] {
        let m = load(scenario);
        let s = m.state();
        assert_eq!(s.rip, MAIN);
        assert_eq!(s.rflags, rflags, "{scenario:?}");
        assert_eq!(s.cr0, Some(0x8001_0033));
        assert_eq!(s.cr3, Some(0x1000));
        assert_eq!(s.cr4, Some(0x20));
        assert_eq!(s.sti_shadow, Some(StiShadow::Clear));
        assert_eq!(s.activity, Some(od_isa::CpuActivity::Running));
        assert_eq!(s.idtr.unwrap().limit, 0x41 * 16 + 15);
        assert_eq!(s.gpr[Register::Rbx as usize], EOI_GVA);
        assert_eq!(s.gpr[Register::Rax as usize], 0);
        assert_eq!(s.gpr[Register::Rsp as usize], RSP0);
        assert_eq!(m.vector_state(), (false, false));
        assert_eq!(m.apic().eligible(), None);
    }
}

#[test]
fn loader_rejects_a_foreign_image() {
    let mut foreign = image();
    foreign[0x5000 - 0x1000] = 0x90; // STI replaced by NOP
    assert!(matches!(
        load_stage4_paged_sti_hlt_probe(&foreign, Stage4PagedStiHltScenario::WakeFromHlt),
        Err(Stage4FlatProbeLoadError::Layout(_))
    ));
    assert!(matches!(
        load_stage4_paged_sti_hlt_probe(&[0; 16], Stage4PagedStiHltScenario::WakeFromHlt),
        Err(Stage4FlatProbeLoadError::ImageSize { .. })
    ));
}

#[test]
fn wake_from_hlt_with_exact_state_and_ad_bits() {
    let mut callbacks = Vec::new();
    let mut m = load(Stage4PagedStiHltScenario::WakeFromHlt);
    let report = m.run(16, |r| callbacks.push((r.start().get(), r.len())));
    assert_eq!(report.stop, Stage4PagedStiHltStop::Completed);
    assert_eq!((report.steps, report.retired), (7, 5));
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(
        steps,
        [Sti, Hlt, IdlePoll, Wake, Eoi, Iretq, ResumeHlt],
        "plan order"
    );
    let e = &report.events;

    // (retired, RIP, RSP, RFLAGS, halted, shadow, IRR, ISR) after every step.
    let expected = [
        // STI at IF=0: IF=1 and an interrupt shadow.
        (true, HLT1, RSP0, 0x202, false, true, false, false),
        // HLT retires inside the shadow, ends it and halts; RIP is after HLT.
        (true, HLT2, RSP0, 0x202, true, false, false, false),
        // Nothing requested: the halted CPU keeps waiting, nothing changes.
        (false, HLT2, RSP0, 0x202, true, false, false, false),
        // Wake: gate clears IF, new RSP below the aligned old RSP, vector in service.
        (false, HANDLER, FRAME, 0x002, false, false, false, true),
        (true, HANDLER + 2, FRAME, 0x002, false, false, false, false),
        (true, HLT2, RSP0, 0x202, false, false, false, false),
        // The second HLT after the handler returned.
        (true, END, RSP0, 0x202, true, false, false, false),
    ];
    for (event, (retired, rip, rsp, rflags, halted, shadow, irr, isr)) in e.iter().zip(expected) {
        let step = event.step;
        assert_eq!(event.retired, retired, "{step:?}");
        assert_eq!(event.rip, rip, "{step:?} rip");
        assert_eq!(event.rsp, rsp, "{step:?} rsp");
        assert_eq!(event.rflags, rflags, "{step:?} rflags");
        assert_eq!(event.halted, halted, "{step:?} halted");
        assert_eq!(event.shadow_blocking, shadow, "{step:?} shadow");
        assert_eq!((event.irr, event.isr), (irr, isr), "{step:?} irr/isr");
        assert_eq!(event.cs, 0x08, "{step:?}");
        assert_eq!(event.cr2, None, "{step:?}");
        assert_eq!(event.page_fault, None, "{step:?}");
        assert_eq!(event.frame.is_some(), step == Wake, "{step:?}");
    }
    assert_eq!(
        [e[0].exit, e[1].exit, e[2].exit, e[3].exit],
        ["retired", "halted", "idle", "entered"]
    );

    // Frame at the new RSP, low to high: RIP (after HLT), CS, RFLAGS, RSP, SS.
    assert_eq!(e[3].frame, Some([HLT2, 0x08, 0x202, RSP0, 0x10]));
    assert_eq!(e[3].frame.unwrap()[2] & IF, IF, "saved RFLAGS keeps IF");
    for (i, v) in [HLT2, 0x08, 0x202, RSP0, 0x10].into_iter().enumerate() {
        assert_eq!(qword(&mut m, 0xa000 + 0xfc8 + 8 * i as u64), v);
    }
    assert_eq!(qword(&mut m, 0xa000 + 0xff8), 0, "above the aligned RSP");

    // PTE A/D after each step: [main, handler, idt, gdt, lapic, stack].
    let ptes = [
        [0x6023, 0x7003, 0x8003, 0x9003, 0xfee0_0003, 0xa003], // STI fetched main
        [0x6023, 0x7003, 0x8003, 0x9003, 0xfee0_0003, 0xa003], // HLT, same page
        [0x6023, 0x7003, 0x8003, 0x9003, 0xfee0_0003, 0xa003], // idle poll: no access
        [0x6023, 0x7003, 0x8023, 0x9023, 0xfee0_0003, 0xa063], // gate, descriptor (A), frame (A,D)
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063], // handler fetch, EOI store (A,D)
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063], // IRETQ
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063], // second HLT
    ];
    for (event, expected) in e.iter().zip(ptes) {
        assert_eq!(pte(event), expected, "{:?}", event.step);
    }
    // The wake reports the A/D updates it made on the IDT, GDT and stack leaf PTEs.
    let updated: Vec<u64> = e[3].ad_updates.iter().map(|u| u.address.get()).collect();
    for leaf in [0x5000, 0x5008, 0x5800] {
        assert!(updated.contains(&leaf), "{leaf:#x} in {updated:x?}");
    }
    // The five frame stores are committed RAM writes of the stack page.
    let writes: Vec<_> = e[3]
        .committed_writes
        .iter()
        .map(|r| (r.start().get(), r.len()))
        .collect();
    for slot in 0..5u64 {
        assert!(
            writes.contains(&(0xa000 + 0xfc8 + 8 * slot, 8)),
            "{writes:x?}"
        );
    }
    assert!(!callbacks.is_empty());
    assert_eq!(m.vector_state(), (false, false));
    assert_eq!(m.apic().eligible(), None);
    assert_eq!(m.state().rip, END);
}

#[test]
fn boundary_entry_before_hlt_saves_the_address_of_hlt() {
    let (mut m, report) = run(
        Stage4PagedStiHltScenario::BoundaryBeforeHlt,
        Stage4PagedStiHltInjection::None,
    );
    assert_eq!(report.stop, Stage4PagedStiHltStop::Completed);
    assert_eq!((report.steps, report.retired), (6, 4));
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(steps, [Sti, Entry, Eoi, Iretq, Hlt, IdlePoll]);
    let e = &report.events;
    // (RIP, RSP, RFLAGS, halted, shadow, IRR, ISR).
    let expected = [
        // IF was already 1: STI leaves no shadow.
        (HLT1, RSP0, 0x202, false, false, false, false),
        (HANDLER, FRAME, 0x002, false, false, false, true),
        (HANDLER + 2, FRAME, 0x002, false, false, false, false),
        // IRETQ returns to the HLT that had not executed yet.
        (HLT1, RSP0, 0x202, false, false, false, false),
        (HLT2, RSP0, 0x202, true, false, false, false),
        (HLT2, RSP0, 0x202, true, false, false, false),
    ];
    for (event, (rip, rsp, rflags, halted, shadow, irr, isr)) in e.iter().zip(expected) {
        let step = event.step;
        assert_eq!(
            (event.rip, event.rsp, event.rflags),
            (rip, rsp, rflags),
            "{step:?}"
        );
        assert_eq!(
            (event.halted, event.shadow_blocking, event.irr, event.isr),
            (halted, shadow, irr, isr),
            "{step:?}"
        );
    }
    assert_eq!(e[1].exit, "entered");
    assert_eq!(e[1].frame, Some([HLT1, 0x08, 0x202, RSP0, 0x10]));
    assert_eq!(
        pte(&e[1]),
        [0x6023, 0x7003, 0x8023, 0x9023, 0xfee0_0003, 0xa063]
    );
    assert_eq!(
        pte(&e[5]),
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063]
    );
    assert_eq!(qword(&mut m, 0xa000 + 0xfc8), HLT1);
}

#[test]
fn idt_gate_not_present_keeps_the_cpu_halted_and_the_request_pending() {
    let (m, report) = run(
        Stage4PagedStiHltScenario::WakeFromHlt,
        Stage4PagedStiHltInjection::IdtGateNotPresent,
    );
    assert_eq!(
        report.stop,
        Stage4PagedStiHltStop::DeliveryOutsideProfile(InterruptOutsideProfile::InvalidGate)
    );
    assert_eq!((report.steps, report.retired), (4, 2));
    let e = &report.events;
    assert_eq!(e.len(), 4);
    assert_eq!((e[3].step, e[3].exit), (Wake, "outside_profile"));
    // CPU unchanged: still halted at the instruction after HLT; the vector stays pending,
    // never entered service, the handler did not run.
    assert_eq!((e[3].rip, e[3].rsp, e[3].rflags), (HLT2, RSP0, 0x202));
    assert!(e[3].halted && !e[3].shadow_blocking);
    assert_eq!((e[3].irr, e[3].isr), (true, false));
    assert_eq!(e[3].frame, None);
    // Only the gate was read (A on the IDT leaf); GDT, stack and handler pages are untouched.
    assert_eq!(
        pte(&e[3]),
        [0x6023, 0x7003, 0x8023, 0x9003, 0xfee0_0003, 0xa003]
    );
    assert!(m.state().activity == Some(od_isa::CpuActivity::Halted));
    assert_eq!(
        m.apic().eligible(),
        Some(0x41),
        "request survives for a retry"
    );
}

#[test]
fn read_only_stack_aborts_the_wake_with_an_architectural_page_fault() {
    let (m, report) = run(
        Stage4PagedStiHltScenario::WakeFromHlt,
        Stage4PagedStiHltInjection::StackReadOnly,
    );
    let Stage4PagedStiHltStop::DeliveryAborted { phase, cause } = &report.stop else {
        panic!("unexpected stop {:?}", report.stop)
    };
    assert_eq!(
        *phase,
        InterruptDeliveryPhase::StackWrite {
            slot: InterruptFrameSlot::OldSs
        }
    );
    // The first frame qword is SS at aligned RSP - 8 = 0x700fe8: #PF with P|W (error code 3).
    let od_isa::memory::MemoryError::Architectural(fault) = cause else {
        panic!("unexpected cause {cause:?}")
    };
    assert_eq!(
        (fault.vector, fault.error_code, fault.linear_address),
        (14, Some(3), Some(0x70_0fe8))
    );
    assert_eq!((report.steps, report.retired), (4, 2));
    let e = &report.events;
    assert_eq!((e[3].step, e[3].exit), (Wake, "delivery_aborted"));
    let pf = e[3].page_fault.unwrap();
    assert_eq!(
        (pf.vector, pf.error_code, pf.linear_address),
        (14, Some(3), Some(0x70_0fe8))
    );
    // Terminal model: no nested delivery, CR2 and the CPU are untouched, the vector is not confirmed.
    assert_eq!(e[3].cr2, None);
    assert_eq!((e[3].rip, e[3].rsp, e[3].rflags), (HLT2, RSP0, 0x202));
    assert!(e[3].halted);
    assert_eq!((e[3].irr, e[3].isr), (true, false));
    // The walk set A on the read-only stack PTE and never D; IDT and GDT were read.
    assert_eq!(
        pte(&e[3]),
        [0x6023, 0x7003, 0x8023, 0x9023, 0xfee0_0003, 0xa021]
    );
    assert_eq!(m.state().cr2, None);
}

#[test]
fn outcomes_are_distinguishable_by_stop_steps_irr_isr_rsp_rip_and_ad() {
    let mut outcomes = Vec::new();
    for injection in [
        Stage4PagedStiHltInjection::None,
        Stage4PagedStiHltInjection::IdtGateNotPresent,
        Stage4PagedStiHltInjection::StackReadOnly,
    ] {
        let (mut m, report) = run(Stage4PagedStiHltScenario::WakeFromHlt, injection);
        let last = report.events.last().unwrap();
        outcomes.push((
            format!("{:?}", report.stop),
            report.steps,
            (last.irr, last.isr),
            last.rsp,
            last.rip,
            pte(last),
            m.ptes().idt,
        ));
    }
    for i in 0..outcomes.len() {
        for j in i + 1..outcomes.len() {
            assert_ne!(outcomes[i], outcomes[j], "{i} vs {j}");
            assert_ne!(outcomes[i].0, outcomes[j].0, "stop {i} vs {j}");
        }
    }
    // The two failures share steps, IRR/ISR, RSP and RIP; their A/D bits and stop differ.
    assert_eq!(
        (outcomes[1].1, outcomes[1].2, outcomes[1].3, outcomes[1].4),
        (outcomes[2].1, outcomes[2].2, outcomes[2].3, outcomes[2].4)
    );
    assert_ne!(outcomes[1].5, outcomes[2].5);
}

#[test]
fn budget_stops_between_steps_without_losing_state() {
    for budget in 0..=7u64 {
        let mut m = load(Stage4PagedStiHltScenario::WakeFromHlt);
        let report = m.run(budget, |_| {});
        assert_eq!(report.steps, budget);
        assert_eq!(
            report.stop,
            if budget == 7 {
                Stage4PagedStiHltStop::Completed
            } else {
                Stage4PagedStiHltStop::BudgetExhausted
            }
        );
    }
}

fn state_of(m: &LoadedStage4PagedStiHltProbe) -> od_isa::LongModeState {
    m.state().clone()
}

fn ctx() -> od_isa::memory::AccessContext {
    od_isa::memory::AccessContext {
        mode: od_isa::memory::InstructionMode::Long64,
        privilege: od_isa::memory::PrivilegeLevel::Ring0,
        origin: od_isa::memory::AccessOrigin::Explicit,
    }
}

#[test]
fn exact_steps_admit_only_their_byte_and_the_base_profile() {
    let mut m = load(Stage4PagedStiHltScenario::WakeFromHlt);
    let defined = od_isa::INTEGER_FLAGS;
    let sti = |m: &mut LoadedStage4PagedStiHltProbe, s: &mut od_isa::LongModeState| {
        step_paged_sti(s, m.bus_mut(), 0x1000, 0xd00, ctx(), defined, |_| {}).unwrap()
    };
    let hlt = |m: &mut LoadedStage4PagedStiHltProbe, s: &mut od_isa::LongModeState| {
        step_paged_hlt(s, m.bus_mut(), 0x1000, 0xd00, ctx(), defined, |_| {}).unwrap()
    };

    // The byte at RIP must be the step's own opcode: HLT at the STI address is unsupported.
    let mut s = state_of(&m);
    let r = hlt(&mut m, &mut s);
    assert_eq!(
        (r.cpu.exit, r.cpu.instructions_executed),
        (PagedProbeExit::Unsupported { opcode: 0xfb }, 0)
    );
    assert_eq!(s, state_of(&m), "refused step changes nothing");
    let mut s = state_of(&m);
    s.rip = HLT1;
    let r = sti(&mut m, &mut s);
    assert_eq!(
        (r.cpu.exit, r.cpu.instructions_executed),
        (PagedProbeExit::Unsupported { opcode: 0xf4 }, 0)
    );

    // STI at IF=0 sets IF and the shadow; STI inside the shadow is rejected.
    let mut s = state_of(&m);
    let r = sti(&mut m, &mut s);
    assert_eq!(r.cpu.exit, PagedProbeExit::Retired);
    assert_eq!((s.rip, s.rflags), (HLT1, 0x202));
    assert_eq!(s.sti_shadow, Some(StiShadow::Blocking));
    let mut again = s.clone();
    again.rip = MAIN;
    let r = sti(&mut m, &mut again);
    assert_eq!(
        r.cpu.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::StiShadowActive)
    );

    // HLT in the shadow with IF=1 halts and ends the shadow; with IF=0 in the shadow it is rejected.
    let mut halted = s.clone();
    let r = hlt(&mut m, &mut halted);
    assert_eq!(r.cpu.exit, PagedProbeExit::Halted);
    assert_eq!(r.cpu.instructions_executed, 1);
    assert_eq!(halted.activity, Some(od_isa::CpuActivity::Halted));
    assert_eq!(halted.sti_shadow, Some(StiShadow::Clear));
    assert_eq!(halted.rip, HLT2);
    let mut no_if = s.clone();
    no_if.rflags &= !IF;
    let r = hlt(&mut m, &mut no_if);
    assert_eq!(
        r.cpu.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::StiShadowActive)
    );
    // A halted CPU executes nothing.
    let r = hlt(&mut m, &mut halted);
    assert_eq!(
        (r.cpu.exit, r.cpu.instructions_executed),
        (PagedProbeExit::Halted, 0)
    );
    assert_eq!(halted.rip, HLT2);

    // CR0 outside the BASE profile and a wrong EFER are outside the profile.
    let mut bad = state_of(&m);
    bad.cr0 = Some(0x8000_0033); // WP clear: not the BASE profile
    let r = sti(&mut m, &mut bad);
    assert_eq!(
        r.cpu.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::ControlRegistersOutsideProfile)
    );
    let mut wrong_efer = state_of(&m);
    let r = step_paged_hlt(
        &mut wrong_efer,
        m.bus_mut(),
        0x1000,
        0x500,
        ctx(),
        defined,
        |_| {},
    );
    assert_eq!(
        r.unwrap().cpu.exit,
        PagedProbeExit::Rejected(PagedProbeRejection::ControlRegistersOutsideProfile),
        "EFER other than 0xD00 is outside the paged profile"
    );
    assert_eq!(wrong_efer, state_of(&m));
}
