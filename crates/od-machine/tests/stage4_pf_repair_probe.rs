use od_isa::Register;
use od_machine::{
    AccessWidth, AdBit, GuestPhysicalAddress, LoadedStage4PfRepairProbe,
    STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_PF_REPAIR_FINAL_RIP, STAGE4_PF_REPAIR_IMAGE_BYTES,
    STAGE4_PF_REPAIR_INITIAL_RSP, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    Stage4PfRepairInjection, Stage4PfRepairStep::*, Stage4PfRepairStop,
    load_stage4_pf_repair_probe,
};

const INITIAL: u64 = 0x1122334455667788;
const CANARY: u64 = 0x6a3591c7d248b0ef;
const STORE: u64 = 0x8877665544332211;

fn put(b: &mut [u8], gpa: u64, v: u64) {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    b[o..o + 8].copy_from_slice(&v.to_le_bytes())
}
fn put_bytes(b: &mut [u8], gpa: u64, v: &[u8]) {
    let o = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    b[o..o + v.len()].copy_from_slice(v)
}

fn image(initial: u64, canary: u64, store: u64) -> Vec<u8> {
    let mut b = vec![0; STAGE4_PF_REPAIR_IMAGE_BYTES];
    for (g, v) in [
        (0x1000, 0x2027),
        (0x2000, 0x3027),
        (0x3010, 0x4027),
        (0x3018, 0x5027),
        (0x3020, 0x6007),
        (0x4008, 0x7027),
        (0x4010, 0x8027),
        (0x4018, 0x6027),
        (0x5000, 0x9027),
        (0x5008, 0xa027),
        (0x5808, 0xb027),
        (0x5810, 0xc027),
        (0x6000, 0xd001),
        (0xd000, initial),
        (0xd008, canary),
        (0xd010, store),
    ] {
        put(&mut b, g, v)
    }
    put_bytes(&mut b, 0x7000, &[0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03]);
    put_bytes(
        &mut b,
        0x8000,
        &[
            0x48, 0x89, 0x37, 0x0f, 0x01, 0x3b, 0x48, 0x8d, 0x64, 0x24, 0x08, 0x48, 0xcf,
        ],
    );
    // IDT gate 14 -> 0x402000, CS 0x08, IST 0, interrupt gate.
    put_bytes(
        &mut b,
        0x9000 + 14 * 16,
        &[
            0x00, 0x20, 0x08, 0x00, 0x00, 0x8e, 0x40, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    put_bytes(&mut b, 0xa008, &[0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0]);
    put_bytes(&mut b, 0xa010, &[0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0]);
    b
}

const DATA: u64 = 0x80_0000;
const MAIN: u64 = 0x40_1000;
const HANDLER: u64 = 0x40_2000;
const RF: u64 = 1 << 16;

fn qword(m: &mut LoadedStage4PfRepairProbe, gpa: u64) -> u64 {
    m.bus_mut()
        .read_physical(GuestPhysicalAddress::new(gpa), AccessWidth::Qword)
        .unwrap()
}

fn load() -> LoadedStage4PfRepairProbe {
    load_stage4_pf_repair_probe(&image(INITIAL, CANARY, STORE)).unwrap()
}

fn writes(e: &od_machine::Stage4PfRepairEvent) -> Vec<(u64, u64)> {
    e.committed_writes
        .iter()
        .map(|r| (r.start().get(), r.len()))
        .collect()
}

#[test]
fn entry_state_is_the_fixed_loader_handoff() {
    let mut m = load();
    let s = m.state();
    assert_eq!(s.rip, MAIN);
    assert_eq!(s.rflags, 0x202);
    assert_eq!(s.cr0, Some(0x8001_0033));
    assert_eq!(s.cr3, Some(0x1000));
    assert_eq!(s.cr4, Some(0x20));
    assert_eq!(s.cs_selector, Some(0x08));
    assert_eq!(s.ss_selector, Some(0x10));
    assert_eq!(s.gdtr.unwrap().base, 0x60_1000);
    assert_eq!(s.idtr.unwrap().base, 0x60_0000);
    assert_eq!(s.gpr[Register::Rbx as usize], DATA);
    assert_eq!(s.gpr[Register::Rcx as usize], STORE);
    assert_eq!(s.gpr[Register::Rdi as usize], 0x40_3000);
    assert_eq!(s.gpr[Register::Rsi as usize], 0xd023);
    assert_eq!(s.gpr[Register::Rsp as usize], STAGE4_PF_REPAIR_INITIAL_RSP);
    assert_eq!(m.initial_qword(), INITIAL);
    assert_eq!(m.canary_qword(), CANARY);
    assert_eq!(m.store_qword(), STORE);
    assert_eq!(m.pte(), 0xd001);
    assert_eq!(m.data_qword(), INITIAL);
}

#[test]
fn fault_delivery_repair_invlpg_discard_iretq_retry_load_with_exact_state() {
    let mut m = load();
    let mut callbacks = Vec::new();
    let report = m.run(16, |r| callbacks.push((r.start().get(), r.len())));
    assert_eq!(report.stop, Stage4PfRepairStop::Completed);
    assert_eq!((report.steps, report.retired), (8, 6));
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(
        steps,
        [Fault, Delivery, Repair, Invlpg, Discard, Iretq, Retry, Load]
    );
    let e = &report.events;
    // (retired, RIP, RSP, RFLAGS.RF, PTE, data qword) after every step.
    let top = STAGE4_PF_REPAIR_INITIAL_RSP;
    let frame = 0x70_1ff0; // 0x702020 minus five frame qwords and the error code
    let expected = [
        (false, MAIN, top, false, 0xd021, INITIAL),
        (false, HANDLER, frame, false, 0xd021, INITIAL),
        (true, HANDLER + 3, frame, false, 0xd023, INITIAL),
        (true, HANDLER + 6, frame, false, 0xd023, INITIAL),
        (true, HANDLER + 11, frame + 8, false, 0xd023, INITIAL),
        (true, MAIN, top, true, 0xd023, INITIAL),
        (true, MAIN + 3, top, false, 0xd063, STORE),
        (true, STAGE4_PF_REPAIR_FINAL_RIP, top, false, 0xd063, STORE),
    ];
    for (event, (retired, rip, rsp, rf, pte, data)) in e.iter().zip(expected) {
        assert_eq!(event.retired, retired, "{:?}", event.step);
        assert_eq!(event.rip, rip, "{:?}", event.step);
        assert_eq!(event.rsp, rsp, "{:?}", event.step);
        assert_eq!(event.rflags & RF != 0, rf, "{:?}", event.step);
        assert_eq!(event.pte, pte, "{:?}", event.step);
        assert_eq!(event.data_qword, data, "{:?}", event.step);
        assert_eq!(event.canary_qword, CANARY, "{:?}", event.step);
        assert_eq!(event.cr2, Some(DATA), "{:?}", event.step);
    }
    // Fault: #PF(P|W) on the supervisor RO page, nothing retires, A set on PD and PTE only.
    let f = e[0].page_fault.unwrap();
    assert_eq!(
        (f.vector, f.error_code, f.cr2, f.saved_rip),
        (14, Some(3), DATA, MAIN)
    );
    assert_eq!(writes(&e[0]), [(0x3020, 8), (0x6000, 8)]);
    assert_eq!(
        e[0].ad_updates
            .iter()
            .map(|u| (u.address.get(), u.bit))
            .collect::<Vec<_>>(),
        [(0x3020, AdBit::Accessed), (0x6000, AdBit::Accessed)]
    );
    assert_eq!(e[0].rflags, 0x202);
    // Delivery: error code 3 on the stack top, IF cleared by the interrupt gate, frame writes.
    assert_eq!(e[1].error_code_on_stack, Some(3));
    assert_eq!(e[1].rflags, 2);
    assert_eq!(e[1].rax, 0);
    assert_eq!(
        writes(&e[1]),
        [
            (0x5810, 8),
            (0xc018, 8),
            (0xc010, 8),
            (0xc008, 8),
            (0xc000, 8),
            (0x5808, 8),
            (0xbff8, 8),
            (0xbff0, 8)
        ]
    );
    // Repair: D on the alias PTE, then the PTE store itself.
    assert_eq!(writes(&e[2]), [(0x4018, 8), (0x6000, 8)]);
    assert_eq!(e[2].ad_updates.len(), 1);
    assert_eq!(e[2].ad_updates[0].address.get(), 0x4018);
    assert_eq!(e[2].ad_updates[0].bit, AdBit::Dirty);
    // INVLPG and LEA retire without committed writes.
    assert!(e[3].committed_writes.is_empty() && e[3].detail.as_deref().unwrap().contains("Invlpg"));
    assert!(e[4].committed_writes.is_empty());
    // IRETQ pops the fault RIP (not the error code) and sets RF in RFLAGS.
    assert_eq!(e[5].iretq_frame_rip, Some(MAIN));
    assert_eq!(e[5].rflags, 0x1_0202);
    assert!(e[5].committed_writes.is_empty());
    // Retry: D on the repaired PTE, then the data qword; RF is cleared by the retired store.
    assert_eq!(writes(&e[6]), [(0x6000, 8), (0xd000, 8)]);
    assert_eq!(e[6].ad_updates[0].bit, AdBit::Dirty);
    assert_eq!(
        (e[6].ad_updates[0].before, e[6].ad_updates[0].after),
        (0xd023, 0xd063)
    );
    assert_eq!(e[6].rflags, 0x202);
    // Load: no writes; RAX gets the stored value; canary never changed.
    assert!(e[7].committed_writes.is_empty());
    assert_eq!(e[7].rax, STORE);
    assert_eq!(
        callbacks,
        [
            (0x3020, 8),
            (0x6000, 8),
            (0x5810, 8),
            (0xc018, 8),
            (0xc010, 8),
            (0xc008, 8),
            (0xc000, 8),
            (0x5808, 8),
            (0xbff8, 8),
            (0xbff0, 8),
            (0x4018, 8),
            (0x6000, 8),
            (0x6000, 8),
            (0xd000, 8)
        ]
    );
    assert_eq!(m.data_qword(), STORE);
    assert_eq!(m.canary(), CANARY);
    assert_eq!(m.pte(), 0xd063);
    // Page tables above the data PTE keep the A bits and nothing else changed.
    assert_eq!(qword(&mut m, 0x3020), 0x6027);
    assert_eq!(qword(&mut m, 0x4018), 0x6067);
    assert_eq!(qword(&mut m, 0x5810) & 0x60, 0x60);
    assert_eq!(qword(&mut m, 0x5808) & 0x60, 0x60);
    assert_eq!(m.state().cr2, Some(DATA));
}

#[test]
fn budget_limits_the_sequence_and_each_prefix_state_is_observable() {
    for budget in 0..=8_u64 {
        let mut m = load();
        let report = m.run(budget, |_| {});
        assert_eq!(report.steps, budget);
        assert_eq!(report.events.len() as u64, budget);
        assert_eq!(
            report.stop,
            if budget == 8 {
                Stage4PfRepairStop::Completed
            } else {
                Stage4PfRepairStop::BudgetExhausted
            }
        );
        // Fault and delivery retire no instruction.
        let retired = budget.saturating_sub(2);
        assert_eq!(report.retired, retired);
        assert_eq!(
            m.pte(),
            match budget {
                0 => 0xd001,
                1 | 2 => 0xd021,
                3..=6 => 0xd023,
                _ => 0xd063,
            }
        );
        assert_eq!(m.data_qword(), if budget >= 7 { STORE } else { INITIAL });
        assert_eq!(
            m.state().gpr[Register::Rax as usize],
            if budget == 8 { STORE } else { 0 }
        );
    }
}

#[test]
fn injected_handler_that_keeps_the_pte_readonly_faults_a_second_time() {
    let mut m = load();
    m.inject(Stage4PfRepairInjection::HandlerKeepsPteReadOnly);
    let report = m.run(16, |_| {});
    assert_eq!(report.stop, Stage4PfRepairStop::SecondPageFault(Retry));
    assert_eq!((report.steps, report.retired), (7, 4));
    let e = &report.events;
    // The repair store retired but wrote the RO value: A kept, R/W still clear.
    assert_eq!(e[2].step, Repair);
    assert_eq!(e[2].pte, 0xd021);
    assert_eq!(e[5].step, Iretq);
    assert!(e[5].retired);
    assert_eq!(e[5].pte, 0xd021);
    let retry = &e[6];
    assert!(!retry.retired);
    assert_eq!(retry.exit, "page_fault");
    let f = retry.page_fault.unwrap();
    assert_eq!(
        (f.vector, f.error_code, f.cr2, f.saved_rip),
        (14, Some(3), DATA, MAIN)
    );
    assert_eq!(retry.rip, MAIN);
    assert_eq!(retry.pte, 0xd021);
    assert_eq!(retry.data_qword, INITIAL);
    assert_eq!(m.data_qword(), INITIAL);
    assert_eq!(m.state().gpr[Register::Rax as usize], 0);
}

#[test]
fn injected_handler_without_lea_makes_iretq_pop_the_error_code_as_rip() {
    let mut m = load();
    m.inject(Stage4PfRepairInjection::HandlerSkipsLea);
    let report = m.run(16, |_| {});
    assert_eq!(report.stop, Stage4PfRepairStop::Step(Iretq, "unsupported"));
    let steps: Vec<_> = report.events.iter().map(|e| e.step).collect();
    assert_eq!(steps, [Fault, Delivery, Repair, Invlpg, Iretq]);
    assert_eq!((report.steps, report.retired), (5, 2));
    let iretq = &report.events[4];
    assert!(!iretq.retired);
    // The first qword IRETQ would pop as RIP is the #PF error code, not the fault RIP.
    assert_eq!(iretq.iretq_frame_rip, Some(3));
    assert_ne!(iretq.iretq_frame_rip, Some(MAIN));
    // The guest never left the handler and never retried the store.
    assert_eq!(iretq.rip, HANDLER + 6);
    assert_eq!(iretq.rsp, 0x70_1ff0);
    assert_eq!(iretq.data_qword, INITIAL);
    assert_eq!(m.data_qword(), INITIAL);
}

#[test]
fn injected_loader_that_maps_the_page_writable_never_delivers_a_fault() {
    let mut m = load();
    m.inject(Stage4PfRepairInjection::LoaderMapsPageWritable);
    let report = m.run(16, |_| {});
    assert_eq!(report.stop, Stage4PfRepairStop::NoPageFault);
    assert_eq!((report.steps, report.retired), (1, 1));
    let e = &report.events[0];
    assert!(e.retired && e.page_fault.is_none());
    assert_eq!(e.rip, MAIN + 3);
    assert_eq!(e.rsp, STAGE4_PF_REPAIR_INITIAL_RSP);
    assert_eq!(m.data_qword(), STORE);
    // No delivery: the stack pages are untouched.
    assert_eq!(qword(&mut m, 0xbff0), 0);
    assert_eq!(qword(&mut m, 0xc000), 0);
}

fn layout(image: &[u8]) -> Stage4FlatProbeLayoutError {
    match load_stage4_pf_repair_probe(image) {
        Err(Stage4FlatProbeLoadError::Layout(e)) => e,
        Err(other) => panic!("{other:?}"),
        Ok(_) => panic!("image must be rejected"),
    }
}

#[test]
fn loader_rejects_foreign_images() {
    assert!(matches!(
        load_stage4_pf_repair_probe(&[0; 16]),
        Err(Stage4FlatProbeLoadError::ImageSize { .. })
    ));
    // Data page already RW, alias made read-only, extra mapping.
    for (gpa, value) in [(0x6000, 0xd003), (0x4018, 0x6021), (0x5018, 0xd003)] {
        let mut b = image(INITIAL, CANARY, STORE);
        put(&mut b, gpa, value);
        assert!(
            matches!(layout(&b), Stage4FlatProbeLayoutError::PageTableEntry { gpa: g, .. } if g == gpa),
            "{gpa:x}"
        );
    }
    // Handler code, hidden byte in the main page tail, IDT gate, GDT, stack, data tail.
    for (offset, page) in [
        (0x8000_usize, 0x8000_u64),
        (0x7100, 0x7000),
        (0x90e0, 0x9000),
        (0xa010, 0xa000),
        (0xb000, 0xb000),
        (0xc100, 0xc000),
        (0xd100, 0xd000),
    ] {
        let mut b = image(INITIAL, CANARY, STORE);
        b[offset - STAGE4_FLAT_PROBE_IMAGE_GPA as usize] ^= 0x40;
        assert_eq!(
            layout(&b),
            Stage4FlatProbeLayoutError::CodeBytes { gpa: page },
            "{offset:x}"
        );
    }
    for (i, c, s) in [
        (STORE, CANARY, STORE),
        (INITIAL, INITIAL, STORE),
        (INITIAL, CANARY, CANARY),
    ] {
        assert_eq!(
            layout(&image(i, c, s)),
            Stage4FlatProbeLayoutError::DataQwordsEqual
        );
    }
}
