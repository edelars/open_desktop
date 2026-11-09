//! S4-27i4b: the event pump over the paged backend and the equality of the
//! flat and paged backends.
//!
//! The paged guest is the S4-27h/S4-27i4a image with the main code
//! `STI; HLT; HLT; HLT` (vector 0x41, handler `MOV [RBX],EAX; IRETQ`); the flat
//! guest is the S4-27i3 queue image (vector 0x62, handler with a RAM counter and
//! EOI). Expected values come from the queue rules of S4-27i3 and the Intel SDM,
//! never from interpreter output:
//!
//! * an event whose deadline is at or before the virtual time is due at the next
//!   boundary; a later one is delivered when the halted CPU's clock reaches its
//!   deadline (instructions take 0 ns);
//! * two due events with one vector make one IRR edge, one entry, one handler run;
//! * 64-bit interrupt gate frame (SDM Vol. 3A 6.14.2): RIP, CS, RFLAGS, RSP, SS
//!   from the low address; a wake from HLT pushes the RIP after the HLT, an entry
//!   at a boundary before an HLT with IF=1 pushes the RIP of the HLT itself;
//! * EOI clears the in-service bit; the handler returns with IRETQ.
//!
//! Comparison of the two backends is by architectural fields (trace names and
//! entry sources, virtual time, frame RIP relative to the HLT, CS/RFLAGS/SS of the
//! frame, IRR/ISR, entry and handler-return counts), never by addresses. The flat
//! pump runs in entry-only delivery mode so that one pump step is one entry on
//! both backends.

use od_isa::StiShadow;
use od_machine::{
    DeviceError, EventQueueError, InterruptPump, PagedBackend, PumpBackend, PumpEntrySource,
    PumpExit, PumpInjection, PumpQueueError, PumpRejection, PumpTraceKind as K,
    STAGE4_INTERRUPT_PUMP_PAGED_HANDLER_RIP as PAGED_HANDLER,
    STAGE4_INTERRUPT_PUMP_PAGED_HLT_RIPS as PAGED_HLT, STAGE4_INTERRUPT_PUMP_PAGED_STACK_GPA,
    STAGE4_INTERRUPT_PUMP_PAGED_STACK_GVA, STAGE4_INTERRUPT_PUMP_PAGED_STI_RIP as PAGED_STI,
    STAGE4_INTERRUPT_PUMP_PAGED_VECTOR as PAGED_VECTOR, STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA,
    STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES, STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR as FLAT_VECTOR,
    Stage4InterruptPumpPagedStart, enable_stage4_interrupt_pump_paged_apic,
    expected_stage4_interrupt_pump_paged_image, load_stage4_interrupt_pump_paged_probe,
    load_stage4_interrupt_queue_probe,
};

/// Deadline of the future event (virtual ns).
const FUTURE_NS: u64 = 250_000;
const PAGED_STACK_TOP: u64 = 0x0070_0ff8;
const FLAT_STACK_TOP: u64 = 0x6ff0;

type Paged = InterruptPump<PagedBackend>;

// ---- paged fixtures ------------------------------------------------------------

fn paged(start: Stage4InterruptPumpPagedStart) -> Paged {
    load_stage4_interrupt_pump_paged_probe(&expected_stage4_interrupt_pump_paged_image(), start)
        .unwrap()
}

fn paged_gpa(gva: u64) -> u64 {
    STAGE4_INTERRUPT_PUMP_PAGED_STACK_GPA + (gva - STAGE4_INTERRUPT_PUMP_PAGED_STACK_GVA)
}

/// PTEs of main, handler, IDT, GDT, Local APIC and stack.
fn paged_ptes(pump: &mut Paged) -> [u64; 6] {
    [0x4008, 0x4010, 0x5000, 0x5008, 0x5010, 0x5800].map(|gpa| pump.peek_ram_qword(gpa).unwrap())
}

// ---- flat fixtures (the S4-27i3 queue image, assembled byte for byte) -----------

const BASE: usize = 0x1000;

fn mov_edi() -> Vec<u8> {
    vec![0xbf, 0x00, 0x00, 0xe0, 0xfe]
}

fn write_reg(disp: u32, value: u32) -> Vec<u8> {
    let mut code = vec![0xb8];
    code.extend(value.to_le_bytes());
    code.extend([0x89, 0x87]);
    code.extend(disp.to_le_bytes());
    code
}

fn put(image: &mut [u8], gpa: usize, bytes: &[u8]) {
    image[gpa - BASE..gpa - BASE + bytes.len()].copy_from_slice(bytes);
}

/// `HLT` address and handler address of a flat image.
struct FlatBuilt {
    image: Vec<u8>,
    hlt: u64,
    handler: u64,
}

/// SVR enable, then `tail`; handler: count in RAM at 0x5000, EOI, IRETQ.
fn flat_image(tail: &[u8], hlt_offset: u64) -> FlatBuilt {
    let entry = [mov_edi(), write_reg(0xf0, 0x1ff), tail.to_vec()].concat();
    let mut counter = mov_edi();
    counter.extend([0xbe, 0x00, 0x50, 0x00, 0x00]);
    counter.extend([0x8b, 0x06, 0x83, 0xc0, 0x01, 0x89, 0x06]);
    let handler = [counter, write_reg(0xb0, 0), vec![0x48, 0xcf]].concat();
    let mut image = vec![0; STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES];
    for (i, d) in [0_u64, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff]
        .iter()
        .enumerate()
    {
        put(&mut image, 0x2000 + 8 * i, &d.to_le_bytes());
    }
    let handler_gpa = 0x4000 + entry.len() as u64;
    let mut gate = Vec::new();
    gate.extend((handler_gpa as u16).to_le_bytes());
    gate.extend(8_u16.to_le_bytes());
    gate.extend([0, 0x8e, 0, 0]);
    gate.extend([0; 8]);
    put(&mut image, 0x3000 + usize::from(FLAT_VECTOR) * 16, &gate);
    put(&mut image, 0x4000, &entry);
    put(&mut image, handler_gpa as usize, &handler);
    FlatBuilt {
        image,
        hlt: 0x4000 + 16 + hlt_offset,
        handler: handler_gpa,
    }
}

/// `STI; loop: HLT; JMP loop`.
fn flat_loop() -> FlatBuilt {
    flat_image(&[0xfb, 0xf4, 0xeb, 0xfd], 1)
}

/// `STI; NOP; loop: HLT; JMP loop`.
fn flat_nop() -> FlatBuilt {
    flat_image(&[0xfb, 0x90, 0xf4, 0xeb, 0xfd], 2)
}

fn flat(built: &FlatBuilt) -> InterruptPump {
    load_stage4_interrupt_queue_probe(&built.image)
        .unwrap()
        .with_entry_only_delivery()
}

fn flat_counter(pump: &mut InterruptPump) -> u64 {
    pump.peek_ram_qword(STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA)
        .unwrap()
        & 0xffff_ffff
}

// ---- backend-neutral helpers ----------------------------------------------------

/// What differs between the two guests; everything else is compared.
struct Rig {
    vector: u8,
    stack_top: u64,
    /// The first HLT (a boundary entry before it pushes this RIP).
    first_hlt: u64,
    handler: u64,
    /// Where a stack address is stored in guest RAM.
    to_gpa: fn(u64) -> u64,
}

fn paged_rig() -> Rig {
    Rig {
        vector: PAGED_VECTOR,
        stack_top: PAGED_STACK_TOP,
        first_hlt: PAGED_HLT[0],
        handler: PAGED_HANDLER,
        to_gpa: paged_gpa,
    }
}

fn flat_rig(built: &FlatBuilt) -> Rig {
    Rig {
        vector: FLAT_VECTOR,
        stack_top: FLAT_STACK_TOP,
        first_hlt: built.hlt,
        handler: built.handler,
        to_gpa: |gva| gva,
    }
}

fn time<B: PumpBackend>(pump: &InterruptPump<B>) -> u64 {
    pump.virtual_time_ns().unwrap()
}

fn count<B: PumpBackend>(pump: &InterruptPump<B>, wanted: &K) -> usize {
    pump.trace()
        .iter()
        .filter(|event| &event.kind == wanted)
        .count()
}

fn frame<B: PumpBackend>(pump: &mut InterruptPump<B>, rig: &Rig, rsp: u64) -> [u64; 5] {
    [0, 8, 16, 24, 32].map(|offset| pump.peek_ram_qword((rig.to_gpa)(rsp + offset)).unwrap())
}

/// One entry, sampled right after the pump step that made it.
#[derive(Debug, PartialEq)]
struct EntryRec {
    source: PumpEntrySource,
    time_ns: u64,
    /// The pump step stopped in the handler.
    in_handler: bool,
    /// Frame RIP minus the address of the HLT that the entry interrupted or woke.
    frame_rip_after_hlt: u64,
    cs: u64,
    rflags: u64,
    ss: u64,
    /// The pushed RSP is the stack top; the new RSP is the 16-byte aligned top
    /// minus the five-qword frame (SDM Vol. 3A 6.14.2).
    pushed_rsp_is_top: bool,
    new_rsp_is_aligned_top_minus_frame: bool,
    /// IRR and ISR bits of the vector right after the entry.
    irr_isr: (bool, bool),
    /// IF is clear in the handler (interrupt gate).
    if_clear: bool,
}

/// What the two backends must agree on.
#[derive(Debug, PartialEq)]
struct Arch {
    stop: PumpExit,
    /// Event names and entry sources in order, with the clock at the event.
    trace: Vec<(&'static str, Option<PumpEntrySource>, Option<u64>)>,
    entries: Vec<EntryRec>,
    handler_returns: usize,
    eois: usize,
    irr_isr: (bool, bool),
    time_ns: u64,
    queue_len: usize,
    coalesced: u64,
    waiting: bool,
    final_if: bool,
    final_rsp_is_top: bool,
}

/// Step one pump step at a time until the pump stops, sampling every entry.
fn drive<B: PumpBackend>(pump: &mut InterruptPump<B>, rig: &Rig) -> (PumpExit, Vec<EntryRec>) {
    let mut entries = Vec::new();
    let mut seen = pump.trace().len();
    // Address of the HLT the CPU executed last (the trace records the RIP after it).
    let mut last_hlt = pump
        .trace()
        .iter()
        .rev()
        .find(|e| e.kind == K::Hlt)
        .map(|e| e.rip - 1);
    loop {
        let result = pump.run(1, || false);
        let new: Vec<_> = pump.trace()[seen..].to_vec();
        seen = pump.trace().len();
        for event in &new {
            match event.kind {
                K::Hlt => last_hlt = Some(event.rip - 1),
                K::Entry { source, .. } => {
                    let hlt_addr = match source {
                        PumpEntrySource::Boundary => rig.first_hlt,
                        PumpEntrySource::HaltWake => last_hlt.unwrap(),
                    };
                    let rsp = pump.state().gpr[od_isa::Register::Rsp as usize];
                    let f = frame(pump, rig, rsp);
                    let state = pump.state();
                    let (rip, rflags) = (state.rip, state.rflags);
                    entries.push(EntryRec {
                        source,
                        time_ns: time(pump),
                        in_handler: rip == rig.handler,
                        frame_rip_after_hlt: f[0] - hlt_addr,
                        cs: f[1],
                        rflags: f[2],
                        ss: f[4],
                        pushed_rsp_is_top: f[3] == rig.stack_top,
                        new_rsp_is_aligned_top_minus_frame: rsp == (rig.stack_top & !0xf) - 40,
                        irr_isr: pump.apic().vector_state(rig.vector),
                        if_clear: rflags & 0x200 == 0,
                    });
                }
                _ => {}
            }
        }
        if result.exit != PumpExit::Budget {
            return (result.exit, entries);
        }
        assert!(pump.trace().len() < 200, "runaway");
    }
}

fn arch<B: PumpBackend>(
    pump: &mut InterruptPump<B>,
    rig: &Rig,
    stop: PumpExit,
    entries: Vec<EntryRec>,
) -> Arch {
    let rsp = pump.state().gpr[od_isa::Register::Rsp as usize];
    Arch {
        stop,
        trace: pump
            .trace()
            .iter()
            .map(|e| {
                let source = match e.kind {
                    K::Entry { source, .. } => Some(source),
                    _ => None,
                };
                (e.kind.name(), source, e.clock_ns)
            })
            .collect(),
        entries,
        handler_returns: count(pump, &K::Iretq),
        eois: count(pump, &K::Eoi { vector: rig.vector }),
        irr_isr: pump.apic().vector_state(rig.vector),
        time_ns: time(pump),
        queue_len: pump.irq_queue_len(),
        coalesced: pump.coalesced_irqs(),
        waiting: pump.is_waiting(),
        final_if: pump.state().rflags & 0x200 != 0,
        final_rsp_is_top: rsp == rig.stack_top,
    }
}

/// Prologue, STI and the first HLT: halted at 0 ns, no deadline.
fn run_to_idle<B: PumpBackend>(pump: &mut InterruptPump<B>) {
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert!(pump.is_waiting());
    assert_eq!(time(pump), 0, "instructions take 0 ns");
}

// ---- scenario drivers shared by both backends -----------------------------------

fn due_and_future<B: PumpBackend>(pump: &mut InterruptPump<B>, rig: &Rig) -> Arch {
    run_to_idle(pump);
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, rig.vector, 0).unwrap(); // due now
    pump.schedule_irq(generation, 2, rig.vector, FUTURE_NS)
        .unwrap(); // future
    assert_eq!(pump.irq_queue_len(), 2);
    assert_eq!(pump.irq_queue_next_deadline(), Some(0));
    let (stop, entries) = drive(pump, rig);
    arch(pump, rig, stop, entries)
}

fn coalesce<B: PumpBackend>(pump: &mut InterruptPump<B>, rig: &Rig) -> Arch {
    run_to_idle(pump);
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, rig.vector, 0).unwrap();
    pump.schedule_irq(generation, 2, rig.vector, 0).unwrap();
    let (stop, entries) = drive(pump, rig);
    arch(pump, rig, stop, entries)
}

/// Pump at the boundary before the first HLT with IF=1 and no shadow.
fn boundary<B: PumpBackend>(pump: &mut InterruptPump<B>, rig: &Rig) -> Arch {
    assert_eq!(pump.state().rip, rig.first_hlt);
    assert!(pump.state().rflags & 0x200 != 0);
    assert_eq!(pump.state().sti_shadow, Some(StiShadow::Clear));
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, rig.vector, 0).unwrap();
    let (stop, entries) = drive(pump, rig);
    arch(pump, rig, stop, entries)
}

fn flat_at_boundary() -> (InterruptPump, Rig) {
    let built = flat_nop();
    let mut pump = flat(&built);
    while !(pump.state().rip == built.hlt
        && pump.state().rflags & 0x200 != 0
        && pump.state().sti_shadow == Some(StiShadow::Clear))
    {
        assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
        assert!(pump.trace().len() < 50, "runaway");
    }
    (pump, flat_rig(&built))
}

fn paged_at_boundary() -> (Paged, Rig) {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOn);
    // STI at IF=1: no shadow; the next boundary is before the first HLT.
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    (pump, paged_rig())
}

/// Phase 1 of the failed-request scenario: the caller disables the APIC, so the
/// request of the due event fails and the event is kept (or lost by the injection);
/// phase 2 enables the APIC again and runs to idle.
fn failed_request<B: PumpBackend>(
    mut pump: InterruptPump<B>,
    rig: &Rig,
    start_rip: u64,
    injection: bool,
    // A global disable resets the Local APIC. The flat guest re-enables it with its
    // own SVR write; the paged guest has no APIC code, so the owner repeats the
    // loader's SVR write.
    reenable: impl FnOnce(&mut InterruptPump<B>),
) -> (usize, usize, Arch) {
    if injection {
        pump = pump.with_injection(PumpInjection::PopBeforeRequest);
    }
    pump.apic().set_global_enabled(false).unwrap();
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, rig.vector, 0).unwrap();
    pump.schedule_irq(generation, 2, rig.vector, FUTURE_NS)
        .unwrap();
    let failed = pump.run(10_000, || false);
    assert_eq!(
        failed.exit,
        PumpExit::Rejected(PumpRejection::QueueRequest {
            vector: rig.vector,
            error: DeviceError::UnsupportedConfiguration,
        })
    );
    // The guest did not move: the failure precedes the instruction.
    assert_eq!((failed.retired, pump.state().rip), (0, start_rip));
    let kept = pump.irq_queue_len();
    pump.apic().set_global_enabled(true).unwrap();
    reenable(&mut pump);
    let (stop, entries) = drive(&mut pump, rig);
    let wakeups = entries.len();
    (kept, wakeups, arch(&mut pump, rig, stop, entries))
}

// ---- (a) the S4-27i3 queue scenario on the paged profile -------------------------

#[test]
fn paged_due_event_is_delivered_now_and_future_event_at_its_deadline() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    let rig = paged_rig();
    run_to_idle(&mut pump);
    // STI, then HLT in the shadow: halted after the first HLT.
    assert_eq!(pump.state().rip, PAGED_HLT[0] + 1);
    assert_eq!(pump.state().sti_shadow, Some(StiShadow::Clear));
    let seen = due_and_future_after_idle(&mut pump, &rig);
    assert_eq!(seen.stop, PumpExit::IdleHalt);
    // Two wakes: the due event at 0 ns, the future one at its deadline.
    let times: Vec<_> = seen.entries.iter().map(|e| e.time_ns).collect();
    assert_eq!(times, vec![0, FUTURE_NS]);
    assert!(seen.entries.iter().all(|e| e.in_handler));
    assert!(
        seen.entries
            .iter()
            .all(|e| e.source == PumpEntrySource::HaltWake)
    );
    // The frame carries the RIP after the HLT that was waiting.
    assert!(seen.entries.iter().all(|e| e.frame_rip_after_hlt == 1));
    assert_eq!((seen.handler_returns, seen.eois), (2, 2));
    assert_eq!(
        (seen.time_ns, seen.queue_len, seen.coalesced),
        (FUTURE_NS, 0, 0)
    );
    assert_eq!(seen.irr_isr, (false, false));
    assert_eq!(
        count(
            &pump,
            &K::Irr {
                vector: PAGED_VECTOR
            }
        ),
        2
    );
    assert_eq!(count(&pump, &K::Hlt), 3);
    // The second wake returned to the third HLT, which halted again.
    assert_eq!(
        (pump.state().rip, pump.state().rflags),
        (PAGED_HLT[2] + 1, 0x202)
    );
    assert_eq!(
        pump.state().gpr[od_isa::Register::Rsp as usize],
        PAGED_STACK_TOP
    );
    assert_eq!(
        frame(&mut pump, &rig, 0x0070_0fc8),
        [PAGED_HLT[2], 8, 0x202, PAGED_STACK_TOP, 0x10]
    );
    // A/D bits: STI/HLT fetched main, the entry read the gate and descriptor and
    // wrote the frame, the handler fetch and the EOI store set A/D of the APIC page.
    assert_eq!(
        paged_ptes(&mut pump),
        [0x6023, 0x7023, 0x8023, 0x9023, 0xfee0_0063, 0xa063]
    );
}

/// Same as `due_and_future`, for a pump already at idle.
fn due_and_future_after_idle(pump: &mut Paged, rig: &Rig) -> Arch {
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, rig.vector, 0).unwrap();
    pump.schedule_irq(generation, 2, rig.vector, FUTURE_NS)
        .unwrap();
    assert_eq!(pump.irq_queue_len(), 2);
    assert_eq!(pump.irq_queue_next_deadline(), Some(0));
    let (stop, entries) = drive(pump, rig);
    arch(pump, rig, stop, entries)
}

#[test]
fn paged_future_event_waits_for_its_deadline_after_the_first_wake() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    run_to_idle(&mut pump);
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, PAGED_VECTOR, 0).unwrap();
    pump.schedule_irq(generation, 2, PAGED_VECTOR, FUTURE_NS)
        .unwrap();
    // Step until the first handler run has returned and the CPU halted again.
    while count(&pump, &K::Iretq) == 0 || !pump.is_waiting() {
        assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
        assert!(pump.trace().len() < 50, "runaway");
    }
    // The future event is still queued with its deadline unchanged, time is 0.
    assert_eq!(time(&pump), 0);
    assert_eq!(pump.irq_queue_len(), 1);
    assert_eq!(pump.irq_queue_next_deadline(), Some(FUTURE_NS));
    assert_eq!(
        count(
            &pump,
            &K::Eoi {
                vector: PAGED_VECTOR
            }
        ),
        1
    );
}

#[test]
fn paged_due_event_before_hlt_with_if_set_enters_at_the_boundary_with_rip_at_hlt() {
    let (mut pump, rig) = paged_at_boundary();
    let seen = boundary(&mut pump, &rig);
    assert_eq!(seen.stop, PumpExit::IdleHalt);
    assert_eq!(seen.entries.len(), 1);
    assert_eq!(seen.entries[0].source, PumpEntrySource::Boundary);
    // The frame's RIP is the HLT itself: it had not retired when the vector came.
    assert_eq!(seen.entries[0].frame_rip_after_hlt, 0);
    assert_eq!((seen.handler_returns, seen.eois, seen.time_ns), (1, 1, 0));
    assert_eq!(
        frame(&mut pump, &rig, 0x0070_0fc8),
        [PAGED_HLT[0], 8, 0x202, PAGED_STACK_TOP, 0x10]
    );
    // IRETQ returned to the first HLT, which halted (RIP after it).
    assert_eq!(pump.state().rip, PAGED_HLT[0] + 1);
    assert_eq!(pump.irq_queue_len(), 0);
}

#[test]
fn paged_pause_and_resume_keep_the_future_event() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    run_to_idle(&mut pump);
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 7, PAGED_VECTOR, FUTURE_NS)
        .unwrap();

    // A zero budget pauses before any step.
    let paused = pump.run(0, || false);
    assert_eq!(paused.exit, PumpExit::Budget);
    assert_eq!((paused.quanta, paused.retired), (0, 0));
    assert_eq!(pump.irq_queue_len(), 1);
    assert_eq!(pump.irq_queue_next_deadline(), Some(FUTURE_NS));
    assert_eq!(time(&pump), 0);

    // One step is the idle advance: the clock reaches the deadline, the event is
    // still queued and undelivered.
    let step = pump.run(1, || false);
    assert_eq!((step.exit, step.quanta), (PumpExit::Budget, 1));
    assert_eq!(time(&pump), FUTURE_NS);
    assert_eq!(pump.irq_queue_len(), 1);
    assert_eq!(count(&pump, &K::Iretq), 0);

    // Resume: the event is delivered exactly once.
    let resumed = pump.run(10_000, || false);
    assert_eq!(resumed.exit, PumpExit::IdleHalt);
    assert_eq!(
        (count(&pump, &K::Iretq), time(&pump), pump.irq_queue_len()),
        (1, FUTURE_NS, 0)
    );
    // A further run does not deliver it again.
    assert_eq!(pump.run(10_000, || false).exit, PumpExit::IdleHalt);
    assert_eq!(count(&pump, &K::Iretq), 1);
}

#[test]
fn paged_reset_rejects_the_old_generation_and_drops_its_events() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    run_to_idle(&mut pump);
    let old = pump.irq_generation();
    pump.schedule_irq(old, 1, PAGED_VECTOR, FUTURE_NS).unwrap();
    pump.schedule_irq(old, 2, PAGED_VECTOR, 0).unwrap();
    assert_eq!(pump.irq_queue_len(), 2);

    let new = pump.reset_irq_queue().unwrap();
    assert_ne!(old, new);
    assert_eq!(pump.irq_generation(), new);
    assert_eq!(pump.irq_queue_len(), 0);
    assert_eq!(pump.irq_queue_next_deadline(), None);
    let stale = PumpQueueError::Queue(EventQueueError::StaleGeneration);
    assert_eq!(pump.schedule_irq(old, 3, PAGED_VECTOR, 0), Err(stale));
    assert_eq!(pump.cancel_irq(old, 1), Err(stale));

    // The old events are not delivered and time does not move: no deadline.
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert_eq!((count(&pump, &K::Iretq), time(&pump)), (0, 0));
    assert_eq!(pump.apic().vector_state(PAGED_VECTOR), (false, false));

    // The same key under the new generation is a fresh event, delivered once.
    pump.schedule_irq(new, 1, PAGED_VECTOR, FUTURE_NS).unwrap();
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert_eq!(
        (count(&pump, &K::Iretq), time(&pump), pump.irq_queue_len()),
        (1, FUTURE_NS, 0)
    );
}

#[test]
fn paged_two_due_events_with_one_vector_make_one_irr_edge_and_one_entry() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    let seen = coalesce(&mut pump, &paged_rig());
    assert_eq!(seen.stop, PumpExit::IdleHalt);
    assert_eq!(seen.entries.len(), 1);
    assert_eq!((seen.handler_returns, seen.eois, seen.time_ns), (1, 1, 0));
    // One IRR rise, one merged event, one entry, one EOI.
    assert_eq!(
        count(
            &pump,
            &K::Irr {
                vector: PAGED_VECTOR
            }
        ),
        1
    );
    assert_eq!(
        count(
            &pump,
            &K::Coalesced {
                vector: PAGED_VECTOR
            }
        ),
        1
    );
    assert_eq!(pump.coalesced_irqs(), 1);
    assert_eq!(pump.irq_queue_len(), 0);
}

// ---- (b) injection: pop before a successful request -------------------------------

fn paged_failed(injection: bool) -> (usize, usize, Arch) {
    failed_request(
        paged(Stage4InterruptPumpPagedStart::InterruptsOff),
        &paged_rig(),
        PAGED_STI,
        injection,
        |pump| enable_stage4_interrupt_pump_paged_apic(pump).unwrap(),
    )
}

#[test]
fn paged_failed_request_keeps_the_event_and_the_injection_loses_it() {
    let (accepted_kept, accepted_wakes, accepted) = paged_failed(false);
    let (injected_kept, injected_wakes, injected) = paged_failed(true);
    // Accepted: the due event stays queued, is delivered after the retry (the wake
    // follows STI; HLT) and the future event at FUTURE_NS: (IdleHalt, 2, 2, deadline).
    assert_eq!(accepted_kept, 2);
    assert_eq!(
        (
            &accepted.stop,
            accepted_wakes,
            accepted.handler_returns,
            accepted.time_ns
        ),
        (&PumpExit::IdleHalt, 2, 2, FUTURE_NS)
    );
    // Injection: the due event was removed before the failing request and is lost:
    // (IdleHalt, 1, 1, deadline).
    assert_eq!(injected_kept, 1);
    assert_eq!(
        (
            &injected.stop,
            injected_wakes,
            injected.handler_returns,
            injected.time_ns
        ),
        (&PumpExit::IdleHalt, 1, 1, FUTURE_NS)
    );
    assert_ne!(accepted, injected);
}

// ---- (c) the same scenarios give the same architectural sequences -----------------

fn flat_loop_pump() -> (InterruptPump, Rig) {
    let built = flat_loop();
    (flat(&built), flat_rig(&built))
}

#[test]
fn flat_and_paged_agree_on_due_and_future_events() {
    let (mut flat_pump, flat_rig) = flat_loop_pump();
    let flat_seen = due_and_future(&mut flat_pump, &flat_rig);
    let mut paged_pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    let paged_seen = due_and_future(&mut paged_pump, &paged_rig());
    assert_eq!(flat_counter(&mut flat_pump), 2);
    assert_eq!(flat_seen.handler_returns, 2);
    assert_eq!(flat_seen, paged_seen);
    assert_eq!(paged_seen.entries.len(), 2);
}

#[test]
fn flat_and_paged_agree_on_the_entry_at_the_boundary_before_hlt() {
    let (mut flat_pump, flat_rig) = flat_at_boundary();
    let flat_seen = boundary(&mut flat_pump, &flat_rig);
    let (mut paged_pump, paged_rig) = paged_at_boundary();
    let paged_seen = boundary(&mut paged_pump, &paged_rig);
    assert_eq!(flat_counter(&mut flat_pump), 1);
    assert_eq!(flat_seen, paged_seen);
    assert_eq!(paged_seen.entries[0].source, PumpEntrySource::Boundary);
    assert_eq!(paged_seen.entries[0].frame_rip_after_hlt, 0);
}

#[test]
fn flat_and_paged_agree_on_coalescing_two_due_events() {
    let (mut flat_pump, flat_rig) = flat_loop_pump();
    let flat_seen = coalesce(&mut flat_pump, &flat_rig);
    let mut paged_pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    let paged_seen = coalesce(&mut paged_pump, &paged_rig());
    assert_eq!(flat_counter(&mut flat_pump), 1);
    assert_eq!(flat_seen, paged_seen);
    assert_eq!((paged_seen.coalesced, paged_seen.entries.len()), (1, 1));
}

#[test]
fn flat_and_paged_agree_on_the_pop_before_request_difference() {
    for injection in [false, true] {
        let built = flat_loop();
        let (flat_kept, flat_wakes, flat_seen) =
            failed_request(flat(&built), &flat_rig(&built), 0x4000, injection, |_| {});
        let (paged_kept, paged_wakes, paged_seen) = paged_failed(injection);
        assert_eq!(flat_kept, paged_kept, "injection {injection}");
        assert_eq!(flat_wakes, paged_wakes, "injection {injection}");
        assert_eq!(flat_seen, paged_seen, "injection {injection}");
        assert_eq!(flat_kept, if injection { 1 } else { 2 });
        assert_eq!(flat_wakes, if injection { 1 } else { 2 });
    }
}

// ---- backend refusals ---------------------------------------------------------------

#[test]
fn paged_gate_not_present_rejects_the_wake_and_keeps_the_irr() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    run_to_idle(&mut pump);
    // Type 0xE with P=0 in the IDT gate of the vector (S4-27h injection).
    pump.backend_mut()
        .bus_mut()
        .patch_physical_ram(
            od_machine::GuestPhysicalAddress::new(
                0x8000 + usize::from(PAGED_VECTOR) as u64 * 16 + 5,
            ),
            &[0x0e],
        )
        .unwrap();
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, PAGED_VECTOR, 0).unwrap();
    let result = pump.run(100, || false);
    assert_eq!(
        result.exit,
        PumpExit::Rejected(PumpRejection::HaltWake(
            od_isa::InterruptOutsideProfile::InvalidGate
        ))
    );
    // Refused before any commit: the CPU stays halted, IRR is kept, ISR is empty.
    assert!(pump.is_waiting());
    assert_eq!(pump.apic().vector_state(PAGED_VECTOR), (true, false));
    assert_eq!(
        count(
            &pump,
            &K::Entry {
                vector: PAGED_VECTOR,
                source: PumpEntrySource::HaltWake
            }
        ),
        0
    );
}

#[test]
fn paged_code_after_the_last_hlt_is_a_terminal_unsupported_opcode() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    run_to_idle(&mut pump);
    let generation = pump.irq_generation();
    // Three wakes: after the third HLT returns, RIP is past the main code (0x00).
    for key in 1..=3 {
        pump.schedule_irq(generation, key, PAGED_VECTOR, key * 10)
            .unwrap();
    }
    let result = pump.run(1000, || false);
    assert_eq!(
        result.exit,
        PumpExit::Terminal(od_machine::DiagnosticTerminalExit::Unsupported { opcode: 0 })
    );
    assert_eq!(count(&pump, &K::Iretq), 3);
}

#[test]
fn paged_stop_is_final() {
    let mut pump = paged(Stage4InterruptPumpPagedStart::InterruptsOff);
    assert_eq!(pump.run(1000, || true).exit, PumpExit::Stopped);
    assert_eq!(pump.run(1000, || false).exit, PumpExit::Stopped);
    assert_eq!(pump.retired_total(), 0);
}
