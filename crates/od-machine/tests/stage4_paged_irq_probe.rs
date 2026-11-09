//! S4-27h oracle. Every expected value is derived from the Intel SDM or from the accepted
//! S4-19b/S4-33c paged-delivery precedent, not from a run of the code under test:
//! - SDM Vol. 3A 6.14.2/6.14.3 (64-bit mode interrupt stack frame): RSP is aligned down to 16,
//!   SS, RSP, RFLAGS, CS, RIP (five qwords, no error code) are pushed from high to low, and an
//!   interrupt gate clears IF (and TF/NT/RF/VM) in the live RFLAGS; the saved RFLAGS keeps IF.
//! - SDM Vol. 3A 13.8.4/13.9: the vector moves IRR -> ISR on acknowledge; a write of 0 to the
//!   EOI register clears the highest ISR bit; a request is not eligible while an equal or
//!   higher priority class is in service (PPR).
//! - SDM Vol. 3A 4.8: the processor sets A on use and D on the first write; a supervisor write to
//!   a read-only page faults when CR0.WP=1 (#PF error code P|W = 3), setting A in the walk.
//! - IRETQ in 64-bit mode pops RIP, CS, RFLAGS, RSP, SS.

use od_isa::Register;
use od_machine::{
    AccessWidth, GuestPhysicalAddress, LoadedStage4PagedIrqProbe, STAGE4_FLAT_PROBE_IMAGE_GPA,
    STAGE4_PAGED_IRQ_IMAGE_BYTES, STAGE4_PAGED_IRQ_INITIAL_RSP, Stage4FlatProbeLayoutError,
    Stage4FlatProbeLoadError, Stage4PagedIrqEvent, Stage4PagedIrqInjection, Stage4PagedIrqStep::*,
    Stage4PagedIrqStop, load_stage4_paged_irq_probe, step_paged_mov_dword_store,
};

fn put(b: &mut [u8], gpa: u64, v: u64) {
    put_bytes(b, gpa, &v.to_le_bytes())
}
fn put_bytes(b: &mut [u8], gpa: u64, v: &[u8]) {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    b[o..o + v.len()].copy_from_slice(v)
}

/// Written from the profile description, independent of the loader's own table.
fn image() -> Vec<u8> {
    let mut b = vec![0; STAGE4_PAGED_IRQ_IMAGE_BYTES];
    for (g, v) in [
        (0x1000, 0x2023),      // PML4[0] -> PDPT
        (0x2000, 0x3023),      // PDPT[0] -> PD
        (0x3010, 0x4023),      // PD[2] (GVA 4 MiB) -> code table
        (0x3018, 0x5023),      // PD[3] (GVA 6 MiB) -> system table
        (0x4008, 0x6003),      // GVA 0x401000 main
        (0x4010, 0x7003),      // GVA 0x402000 handler
        (0x5000, 0x8003),      // GVA 0x600000 IDT
        (0x5008, 0x9003),      // GVA 0x601000 GDT
        (0x5010, 0xfee0_0003), // GVA 0x602000 Local APIC page
        (0x5800, 0xa003),      // GVA 0x700000 stack
    ] {
        put(&mut b, g, v)
    }
    // Two INVLPG [RBX].
    put_bytes(&mut b, 0x6000, &[0x0f, 0x01, 0x3b, 0x0f, 0x01, 0x3b]);
    // MOV [RBX],EAX; IRETQ.
    put_bytes(&mut b, 0x7000, &[0x89, 0x03, 0x48, 0xcf]);
    // Vector 0x41 gate -> 0x402000, CS 0x08, IST 0, present interrupt gate (type 0xE).
    put_bytes(
        &mut b,
        0x8000 + 0x41 * 16,
        &[
            0x00, 0x20, 0x08, 0x00, 0x00, 0x8e, 0x40, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    put_bytes(&mut b, 0x9008, &[0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0]);
    put_bytes(&mut b, 0x9010, &[0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0]);
    b
}

const MAIN: u64 = 0x40_1000;
const HANDLER: u64 = 0x40_2000;
const EOI_GVA: u64 = 0x60_20b0;
const RSP0: u64 = 0x70_0ff8;
/// RSP aligned down to 16 (0x700ff0) minus five qwords.
const FRAME: u64 = 0x70_0fc8;
const IF: u64 = 1 << 9;

fn load() -> LoadedStage4PagedIrqProbe {
    load_stage4_paged_irq_probe(&image()).unwrap()
}

fn run(
    m: &mut LoadedStage4PagedIrqProbe,
    injection: Stage4PagedIrqInjection,
) -> od_machine::Stage4PagedIrqReport {
    m.inject(injection);
    m.run(16, |_| {})
}

fn pte(e: &Stage4PagedIrqEvent) -> [u64; 6] {
    let p = e.ptes;
    [p.main, p.handler, p.idt, p.gdt, p.lapic, p.stack]
}

fn stack_qword(m: &mut LoadedStage4PagedIrqProbe, gpa: u64) -> u64 {
    m.bus_mut()
        .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
        .unwrap()
}

#[test]
fn entry_state_is_the_fixed_loader_handoff() {
    let m = load();
    let s = m.state();
    assert_eq!(s.rip, MAIN);
    assert_eq!(s.rflags, 0x202, "IF=1 from the initial state, not STI");
    assert_eq!(s.cr0, Some(0x8001_0033));
    assert_eq!(s.cr3, Some(0x1000));
    assert_eq!(s.cr4, Some(0x20));
    assert_eq!(s.cs_selector, Some(0x08));
    assert_eq!(s.ss_selector, Some(0x10));
    assert_eq!(s.gdtr.unwrap().base, 0x60_1000);
    assert_eq!(s.idtr.unwrap().base, 0x60_0000);
    assert_eq!(s.idtr.unwrap().limit, 0x41 * 16 + 15);
    assert_eq!(s.gpr[Register::Rbx as usize], EOI_GVA);
    assert_eq!(s.gpr[Register::Rax as usize], 0);
    assert_eq!(s.gpr[Register::Rsp as usize], STAGE4_PAGED_IRQ_INITIAL_RSP);
    assert_eq!(STAGE4_PAGED_IRQ_INITIAL_RSP, RSP0);
    assert_eq!(m.vector_state(), (false, false));
    assert_eq!(m.apic().eligible(), None);
}

#[test]
fn delivery_eoi_iretq_resume_with_exact_state() {
    let mut m = load();
    let mut callbacks = Vec::new();
    m.inject(Stage4PagedIrqInjection::None);
    let report = m.run(16, |r| callbacks.push((r.start().get(), r.len())));
    assert_eq!(report.stop, Stage4PagedIrqStop::Completed);
    assert_eq!((report.steps, report.retired), (5, 4));
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(steps, [Main, Delivery, Eoi, Iretq, Resume]);
    let e = &report.events;

    // (retired, RIP, RSP, RFLAGS, IRR, ISR) after every step.
    let expected = [
        (true, MAIN + 3, RSP0, 0x202, false, false),
        // Interrupt gate: IF cleared, bit 1 stays set; new RSP is below the aligned old RSP.
        (false, HANDLER, FRAME, 0x002, false, true),
        (true, HANDLER + 2, FRAME, 0x002, false, false),
        (true, MAIN + 3, RSP0, 0x202, false, false),
        (true, MAIN + 6, RSP0, 0x202, false, false),
    ];
    for (event, (retired, rip, rsp, rflags, irr, isr)) in e.iter().zip(expected) {
        assert_eq!(event.retired, retired, "{:?}", event.step);
        assert_eq!(event.rip, rip, "{:?}", event.step);
        assert_eq!(event.rsp, rsp, "{:?}", event.step);
        assert_eq!(event.rflags, rflags, "{:?}", event.step);
        assert_eq!((event.irr, event.isr), (irr, isr), "{:?}", event.step);
        assert_eq!(event.cs, 0x08, "{:?}", event.step);
        assert_eq!(event.cr2, None, "{:?}", event.step);
        assert_eq!(event.page_fault, None, "{:?}", event.step);
    }
    assert_eq!(e[1].exit, "delivered");

    // Frame at the new RSP, low to high: RIP, CS, RFLAGS, RSP, SS.
    assert_eq!(e[1].frame, Some([MAIN + 3, 0x08, 0x202, RSP0, 0x10]));
    assert_eq!(e[1].frame.unwrap()[2] & IF, IF, "saved RFLAGS keeps IF");
    // The five frame stores are RAM writes of the stack page (D set on first store).
    // After delivery nothing above the aligned RSP is written.
    assert_eq!(
        stack_qword(&mut m, 0xa000 + 0xff8),
        0,
        "above the aligned RSP"
    );

    // PTE A/D after each step: [main, handler, idt, gdt, lapic, stack].
    let ptes = [
        [0x6023, 0x7003, 0x8003, 0x9003, 0xfee0_0003, 0xa003], // main fetched
        [0x6023, 0x7003, 0x8023, 0x9023, 0xfee0_0003, 0xa063], // gate, descriptor, frame stores
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063], // handler fetch, EOI store (D)
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063], // IRETQ reads stack, fetches code
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063],
    ];
    for (event, expected) in e.iter().zip(ptes) {
        assert_eq!(pte(event), expected, "{:?}", event.step);
    }
    // Every write the walker or the guest committed is reported through the callback.
    assert!(!callbacks.is_empty());
    // Frame stores: the five qword stores land in the stack page (GPA 0xa000 + offset).
    let delivery_writes: Vec<_> = e[1]
        .committed_writes
        .iter()
        .map(|r| (r.start().get(), r.len()))
        .collect();
    for slot in 0..5u64 {
        assert!(
            delivery_writes.contains(&(0xa000 + 0xfc8 + 8 * slot, 8)),
            "frame slot {slot}: {delivery_writes:x?}"
        );
    }
    // Guest memory holds the same frame.
    for (i, v) in [MAIN + 3, 0x08, 0x202, RSP0, 0x10].into_iter().enumerate() {
        assert_eq!(stack_qword(&mut m, 0xa000 + 0xfc8 + 8 * i as u64), v);
    }
    // No pending or in-service vector remains and nothing is eligible.
    assert_eq!(m.vector_state(), (false, false));
    assert_eq!(m.apic().eligible(), None);
}

#[test]
fn idt_gate_not_present_refuses_delivery_before_any_commit() {
    let mut m = load();
    let report = run(&mut m, Stage4PagedIrqInjection::IdtGateNotPresent);
    assert_eq!(
        report.stop,
        Stage4PagedIrqStop::DeliveryOutsideProfile(od_isa::InterruptOutsideProfile::InvalidGate)
    );
    assert_eq!((report.steps, report.retired), (2, 1));
    let e = &report.events;
    assert_eq!(e.len(), 2);
    assert_eq!(e[1].step, Delivery);
    assert_eq!(e[1].exit, "outside_profile");
    // CPU unchanged, vector stays pending and never entered service, handler not run.
    assert_eq!((e[1].rip, e[1].rsp, e[1].rflags), (MAIN + 3, RSP0, 0x202));
    assert_eq!((e[1].irr, e[1].isr), (true, false));
    assert_eq!(e[1].frame, None);
    // Only the gate was read (IDT A set); GDT, stack and handler pages are untouched.
    assert_eq!(
        pte(&e[1]),
        [0x6023, 0x7003, 0x8023, 0x9003, 0xfee0_0003, 0xa003]
    );
    assert_eq!(stack_qword(&mut m, 0xa000 + 0xfc8), 0);
    assert_eq!(m.state().rip, MAIN + 3);
    assert_eq!(m.state().rflags, 0x202);
    assert_eq!(
        m.apic().eligible(),
        Some(0x41),
        "request survives for a later retry"
    );
}

#[test]
fn read_only_stack_aborts_delivery_with_an_architectural_page_fault() {
    let mut m = load();
    let report = run(&mut m, Stage4PagedIrqInjection::StackReadOnly);
    let Stage4PagedIrqStop::DeliveryAborted { phase, cause } = &report.stop else {
        panic!("unexpected stop {:?}", report.stop)
    };
    assert_eq!(
        *phase,
        od_isa::InterruptDeliveryPhase::StackWrite {
            slot: od_isa::InterruptFrameSlot::OldSs
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
    assert_eq!((report.steps, report.retired), (2, 1));
    let e = &report.events;
    assert_eq!(e[1].exit, "delivery_aborted");
    let pf = e[1].page_fault.unwrap();
    assert_eq!(
        (pf.vector, pf.error_code, pf.linear_address),
        (14, Some(3), Some(0x70_0fe8))
    );
    // Terminal model: no nested delivery, CR2 and the CPU are untouched, the vector is not confirmed.
    assert_eq!(e[1].cr2, None);
    assert_eq!((e[1].rip, e[1].rsp, e[1].rflags), (MAIN + 3, RSP0, 0x202));
    assert_eq!((e[1].irr, e[1].isr), (true, false));
    // The walk set A on the read-only stack PTE and never D; IDT and GDT were read.
    assert_eq!(
        pte(&e[1]),
        [0x6023, 0x7003, 0x8023, 0x9023, 0xfee0_0003, 0xa021]
    );
    assert_eq!(stack_qword(&mut m, 0xa000 + 0xfe8), 0);
}

#[test]
fn handler_without_eoi_keeps_isr_and_blocks_a_second_delivery() {
    let mut m = load();
    let report = run(&mut m, Stage4PagedIrqInjection::HandlerWithoutEoi);
    assert_eq!(report.stop, Stage4PagedIrqStop::SecondDeliveryBlocked);
    assert_eq!((report.steps, report.retired), (5, 3));
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(steps, [Main, Delivery, Iretq, Resume, Redelivery]);
    let e = &report.events;
    assert_eq!((e[1].irr, e[1].isr), (false, true));
    // The bare IRETQ at the handler entry returns to the interrupted instruction.
    assert_eq!(
        (e[2].retired, e[2].rip, e[2].rsp, e[2].rflags),
        (true, MAIN + 3, RSP0, 0x202)
    );
    assert_eq!((e[2].irr, e[2].isr), (false, true));
    assert_eq!((e[3].retired, e[3].rip), (true, MAIN + 6));
    // The same vector is requested again: IRR set, ISR still set, not eligible.
    assert_eq!((e[4].exit, e[4].retired), ("blocked", false));
    assert_eq!((e[4].irr, e[4].isr), (true, true));
    assert_eq!((e[4].rip, e[4].rsp), (MAIN + 6, RSP0));
    // No EOI store happened: the APIC page was never written.
    assert_eq!(e[4].ptes.lapic, 0xfee0_0003);
    assert_eq!(m.apic().eligible(), None);
}

#[test]
fn injections_are_pairwise_distinguishable() {
    let mut seen = Vec::new();
    for injection in [
        Stage4PagedIrqInjection::None,
        Stage4PagedIrqInjection::IdtGateNotPresent,
        Stage4PagedIrqInjection::StackReadOnly,
        Stage4PagedIrqInjection::HandlerWithoutEoi,
    ] {
        let mut m = load();
        let r = run(&mut m, injection);
        let last = r.events.last().unwrap();
        seen.push((
            format!("{:?}", r.stop),
            r.steps,
            r.retired,
            last.irr,
            last.isr,
            last.rsp,
            last.rip,
        ));
    }
    for i in 0..seen.len() {
        for j in i + 1..seen.len() {
            assert_ne!(seen[i], seen[j], "{i} vs {j}");
        }
    }
}

#[test]
fn budget_stops_before_the_next_step_and_does_not_partially_deliver() {
    let mut m = load();
    let report = m.run(1, |_| {});
    assert_eq!(report.stop, Stage4PagedIrqStop::BudgetExhausted);
    assert_eq!((report.steps, report.retired), (1, 1));
    assert_eq!(m.vector_state(), (false, false));
    let report = m.run(0, |_| {});
    assert_eq!((report.steps, report.retired), (0, 0));
}

#[test]
fn loader_rejects_wrong_size_and_every_class_of_corruption() {
    assert_eq!(
        load_stage4_paged_irq_probe(&[0; 16]).err(),
        Some(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_PAGED_IRQ_IMAGE_BYTES,
            actual: 16
        })
    );
    // A page-table entry is reported by GPA with the expected and the actual value.
    let mut b = image();
    put(&mut b, 0x5010, 0xfee0_0023);
    assert_eq!(
        load_stage4_paged_irq_probe(&b).err(),
        Some(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry {
                gpa: 0x5010,
                expected: 0xfee0_0003,
                actual: 0xfee0_0023
            }
        ))
    );
    // Code, IDT, GDT and stack pages are reported by page base.
    for (gpa, offset) in [
        (0x6000, 0u64),
        (0x7000, 3),
        (0x8000, 0x41 * 16 + 5),
        (0x9000, 9),
        (0xa000, 0xff8),
    ] {
        let mut b = image();
        let o = (gpa + offset - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
        b[o] ^= 0x01;
        assert_eq!(
            load_stage4_paged_irq_probe(&b).err(),
            Some(Stage4FlatProbeLoadError::Layout(
                Stage4FlatProbeLayoutError::CodeBytes { gpa }
            )),
            "{gpa:#x}"
        );
    }
}

/// The exact `89 03` profile around the EOI store: its boundaries are explicit.
#[test]
fn dword_store_profile_boundaries() {
    use od_isa::PagedProbeExit::*;
    use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    };
    let go = |mutate: &dyn Fn(&mut od_isa::LongModeState), efer: u64| {
        let mut m = load();
        let mut state = m.state().clone();
        // Execute at the handler entry (EOI store) with the fixed handoff.
        state.rip = HANDLER;
        mutate(&mut state);
        let before = state.clone();
        let report = step_paged_mov_dword_store(
            &mut state,
            m.bus_mut(),
            0x1000,
            efer,
            context,
            od_isa::INTEGER_FLAGS,
            |_| {},
        );
        (report, before, state)
    };
    let go = |mutate: &dyn Fn(&mut od_isa::LongModeState), efer: u64| {
        let (report, before, after) = go(mutate, efer);
        (report.unwrap(), before, after)
    };
    // Exact retirement: RIP += 2, registers and flags unchanged.
    let (r, before, after) = go(&|_| {}, 0xd00);
    assert_eq!((r.cpu.exit, r.cpu.instructions_executed), (Retired, 1));
    assert_eq!(after.rip, HANDLER + 2);
    assert_eq!(after.rflags, before.rflags);
    assert_eq!(after.gpr, before.gpr);
    // Non-canonical RBX: #GP(0) and no retirement.
    let (r, _, after) = go(
        &|s| s.gpr[Register::Rbx as usize] = 0x0001_0000_0000_0000,
        0xd00,
    );
    assert!(matches!(r.cpu.exit, Fault(f) if f.vector == 13 && f.error_code == Some(0)));
    assert_eq!((r.cpu.instructions_executed, after.rip), (0, HANDLER));
    // A dword that would cross a 4 KiB boundary is outside the profile.
    let (r, _, _) = go(&|s| s.gpr[Register::Rbx as usize] = 0x60_2ffd, 0xd00);
    assert!(matches!(r.cpu.exit, Unsupported { opcode: 0x89 }));
    // Other bytes at RIP (the IRETQ in the handler) are not this profile.
    let (r, _, _) = go(&|s| s.rip = HANDLER + 2, 0xd00);
    assert!(matches!(r.cpu.exit, Unsupported { opcode: 0x48 }));
    // EFER, CR0 and CR4 outside the profile are rejected before any access.
    let (r, _, _) = go(&|_| {}, 0x500);
    assert!(matches!(r.cpu.exit, Rejected(_)));
    let (r, _, _) = go(&|s| s.cr0 = Some(0x8000_0033), 0xd00);
    assert!(matches!(r.cpu.exit, Rejected(_)), "WP clear is not BASE");
    let (r, _, _) = go(&|s| s.cr4 = Some(0x20 | (1 << 9)), 0xd00);
    assert!(matches!(r.cpu.exit, Rejected(_)), "CR4 is not BASE");
    // A store to an unmapped linear address is a #PF (not-present, write): error code 2.
    let (r, _, after) = go(&|s| s.gpr[Register::Rbx as usize] = 0x90_0000, 0xd00);
    assert!(matches!(r.cpu.exit, Fault(f) if f.vector == 14 && f.error_code == Some(2)));
    assert_eq!(
        r.page_fault.unwrap().fault().linear_address,
        Some(0x90_0000)
    );
    assert_eq!(after.rip, HANDLER);
}
