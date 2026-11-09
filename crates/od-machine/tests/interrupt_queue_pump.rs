//! S4-27i3: the IRQ event queue of the flat event pump (`InterruptPump`) and the
//! idle arbitration of ISSUE-070.
//!
//! The images are assembled here byte-for-byte (mirroring the Python builder's
//! validated encodings). Expected values come from the rules of the queue and
//! the Intel SDM, never from interpreter output:
//!
//! * The caller schedules `(key, vector, due_ns)`; an event whose deadline is at
//!   or before the virtual time is due at the next boundary, a later one is
//!   delivered when the halted CPU's clock reaches its deadline (idle advance:
//!   the time after the wake is exactly the deadline, instructions take 0 ns).
//! * `request_fixed_edge` sets one IRR bit: two due events with one vector make
//!   one IRR edge, hence one interrupt entry and one handler run.
//! * 64-bit interrupt gate frame (SDM Vol. 3A 6.14.2): SS, RSP, RFLAGS, CS, RIP
//!   (five qwords); the stack top 0x6ff0 is 16-byte aligned, so RSP becomes
//!   0x6ff0 - 40 = 0x6fc8 and the pushed RFLAGS is 0x202 (IF=1 before the
//!   entry). A wake from HLT pushes the RIP after HLT; an interrupt taken at a
//!   boundary with IF=1 before HLT pushes the RIP of the HLT itself.
//! * EOI clears the in-service bit (SDM Vol. 3A 10.8.3.1): without it the same
//!   vector stays blocked (ISSUE-070 scenario).
//! * APIC timer period (SDM Vol. 3A 10.5.4): Initial Count 625, divisor 16 and
//!   a 100 MHz input give P = 625 * 16 / 100e6 s = 100_000 ns.

use od_isa::{Register, StiShadow};
use od_machine::{
    DeviceError, EventQueueError, InterruptPump, PumpEntrySource, PumpExit, PumpInjection,
    PumpQueueError, PumpRejection, PumpTraceKind as K, STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA,
    STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES, STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR as VECTOR,
    STAGE4_INTERRUPT_TIMER_PROBE_VECTOR as TIMER_VECTOR, load_stage4_interrupt_queue_probe,
    load_stage4_interrupt_timer_probe,
};

const BASE: usize = 0x1000;
const FRAME: u64 = 0x6fc8;
const STACK_TOP: u64 = 0x6ff0;
const COUNTER: u64 = STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA;
/// Deadline of the future event (virtual ns).
const FUTURE_NS: u64 = 250_000;
/// 625 * 16 / 100 MHz.
const PERIOD_NS: u64 = 100_000;

fn mov_edi() -> Vec<u8> {
    vec![0xbf, 0x00, 0x00, 0xe0, 0xfe]
}

fn mov_eax(value: u32) -> Vec<u8> {
    let mut code = vec![0xb8];
    code.extend(value.to_le_bytes());
    code
}

fn store_eax(disp: u32) -> Vec<u8> {
    let mut code = vec![0x89, 0x87];
    code.extend(disp.to_le_bytes());
    code
}

/// `MOV EAX,value; MOV [RDI+disp],EAX`.
fn write_reg(disp: u32, value: u32) -> Vec<u8> {
    [mov_eax(value), store_eax(disp)].concat()
}

/// `MOV EDI,0xFEE00000; MOV ESI,0x5000; MOV EAX,[RSI]; ADD EAX,1; MOV [RSI],EAX`.
fn count_in_ram() -> Vec<u8> {
    let mut code = mov_edi();
    code.extend([0xbe, 0x00, 0x50, 0x00, 0x00]);
    code.extend([0x8b, 0x06, 0x83, 0xc0, 0x01, 0x89, 0x06]);
    code
}

/// Counter handler with EOI.
fn handler() -> Vec<u8> {
    [count_in_ram(), write_reg(0xb0, 0), vec![0x48, 0xcf]].concat()
}

fn put(image: &mut [u8], gpa: usize, bytes: &[u8]) {
    image[gpa - BASE..gpa - BASE + bytes.len()].copy_from_slice(bytes);
}

fn gate(handler: u64) -> Vec<u8> {
    let mut gate = Vec::new();
    gate.extend((handler as u16).to_le_bytes());
    gate.extend(8_u16.to_le_bytes());
    gate.extend([0, 0x8e, 0, 0]);
    gate.extend([0; 8]);
    gate
}

struct Built {
    image: Vec<u8>,
    /// Address of the HLT instruction.
    hlt: u64,
    handler: u64,
}

fn build_with(vector: u8, entry: Vec<u8>, handler: Vec<u8>, hlt: u64) -> Built {
    let mut image = vec![0; STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES];
    for (i, d) in [0_u64, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff]
        .iter()
        .enumerate()
    {
        put(&mut image, 0x2000 + 8 * i, &d.to_le_bytes());
    }
    let handler_gpa = 0x4000 + entry.len() as u64;
    put(
        &mut image,
        0x3000 + usize::from(vector) * 16,
        &gate(handler_gpa),
    );
    put(&mut image, 0x4000, &entry);
    put(&mut image, handler_gpa as usize, &handler);
    Built {
        image,
        hlt,
        handler: handler_gpa,
    }
}

/// `SVR = 0x1ff`, then the given tail.
fn queue_entry(tail: &[u8]) -> Vec<u8> {
    [mov_edi(), write_reg(0xf0, 0x1ff), tail.to_vec()].concat()
}

/// `STI; loop: HLT; JMP loop`: HLT follows the 16-byte prologue and STI.
fn build_queue() -> Built {
    let entry = queue_entry(&[0xfb, 0xf4, 0xeb, 0xfd]);
    build_with(VECTOR, entry, handler(), 0x4000 + 16 + 1)
}

/// `STI; NOP; loop: HLT; JMP loop`: the boundary before HLT is clear with IF=1.
fn build_queue_nop() -> Built {
    let entry = queue_entry(&[0xfb, 0x90, 0xf4, 0xeb, 0xfd]);
    build_with(VECTOR, entry, handler(), 0x4000 + 16 + 2)
}

fn load(built: &Built) -> InterruptPump {
    load_stage4_interrupt_queue_probe(&built.image).unwrap()
}

fn counter(pump: &mut InterruptPump) -> u64 {
    pump.peek_ram_qword(COUNTER).unwrap() & 0xffff_ffff
}

fn time(pump: &InterruptPump) -> u64 {
    pump.virtual_time_ns().unwrap()
}

fn entries(pump: &InterruptPump, vector: u8) -> Vec<(u64, PumpEntrySource)> {
    pump.trace()
        .iter()
        .filter_map(|event| match event.kind {
            K::Entry { vector: v, source } if v == vector => Some((event.rip, source)),
            _ => None,
        })
        .collect()
}

fn count(pump: &InterruptPump, wanted: &K) -> usize {
    pump.trace()
        .iter()
        .filter(|event| &event.kind == wanted)
        .count()
}

fn frame(pump: &mut InterruptPump) -> [u64; 5] {
    [0, 8, 16, 24, 32].map(|offset| pump.peek_ram_qword(FRAME + offset).unwrap())
}

/// Evidence that distinguishes the accepted run from the injections.
#[derive(Debug, PartialEq)]
struct Outcome {
    stop: PumpExit,
    wakeups: usize,
    counter: u64,
    time_ns: u64,
}

fn outcome(pump: &mut InterruptPump, vector: u8, stop: PumpExit) -> Outcome {
    Outcome {
        stop,
        wakeups: entries(pump, vector).len(),
        counter: counter(pump),
        time_ns: time(pump),
    }
}

/// Run the prologue, STI and HLT: the CPU is halted at virtual time 0 and no
/// deadline exists, so the pump reports `IdleHalt`.
fn run_to_idle(pump: &mut InterruptPump) {
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert!(pump.is_waiting());
    assert_eq!(time(pump), 0, "instructions take 0 ns");
    assert_eq!(counter(pump), 0);
}

#[test]
fn due_event_is_delivered_now_and_future_event_at_its_deadline() {
    let built = build_queue();
    let mut pump = load(&built);
    run_to_idle(&mut pump);
    assert_eq!(pump.state().rip, built.hlt + 1);

    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, VECTOR, 0).unwrap(); // due now
    pump.schedule_irq(generation, 2, VECTOR, FUTURE_NS).unwrap(); // future
    assert_eq!(pump.irq_queue_len(), 2);
    assert_eq!(pump.irq_queue_next_deadline(), Some(0));

    // One pump step at a time: sample the virtual time at each wake.
    let mut entry_times = Vec::new();
    let mut seen = 0;
    let stop = loop {
        let result = pump.run(1, || false);
        for event in &pump.trace()[seen..] {
            if let K::Entry { vector, source } = event.kind {
                assert_eq!((vector, source), (VECTOR, PumpEntrySource::HaltWake));
                // The entry is observed at the handler's first instruction.
                assert_eq!(event.rip, built.handler);
                entry_times.push(time(&pump));
            }
        }
        // The future event waits for its deadline: after the first wake it is
        // still queued with its deadline unchanged.
        if entry_times.len() == 1 && result.exit == PumpExit::Budget && time(&pump) == 0 {
            assert_eq!(pump.irq_queue_next_deadline(), Some(FUTURE_NS));
        }
        seen = pump.trace().len();
        if result.exit != PumpExit::Budget {
            break result.exit;
        }
        assert!(pump.trace().len() < 200, "runaway");
    };

    assert_eq!(stop, PumpExit::IdleHalt);
    assert_eq!(
        outcome(&mut pump, VECTOR, stop),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 2,
            counter: 2,
            time_ns: FUTURE_NS,
        }
    );
    assert_eq!(entry_times, vec![0, FUTURE_NS]);
    assert_eq!(pump.irq_queue_len(), 0);
    assert_eq!(pump.coalesced_irqs(), 0);
    // Every frame carried the RIP after HLT: IRETQ returned to the JMP.
    assert_eq!(frame(&mut pump), [built.hlt + 1, 8, 0x202, STACK_TOP, 0x10]);
    assert_eq!(pump.apic().vector_state(VECTOR), (false, false));
    assert_eq!(
        (pump.state().rip, pump.state().rflags),
        (built.hlt + 1, 0x202)
    );
    assert_eq!(pump.state().gpr[Register::Rsp as usize], STACK_TOP);
    assert_eq!(count(&pump, &K::Irr { vector: VECTOR }), 2);
    assert_eq!(count(&pump, &K::Eoi { vector: VECTOR }), 2);
    assert_eq!(count(&pump, &K::Iretq), 2);
    assert_eq!(count(&pump, &K::Hlt), 3);
}

#[test]
fn due_event_before_hlt_with_if_set_enters_at_the_boundary_with_rip_at_hlt() {
    let built = build_queue_nop();
    let mut pump = load(&built);
    // Step to the boundary before HLT: IF=1 and the STI shadow is clear.
    while !(pump.state().rip == built.hlt
        && pump.state().rflags & 0x200 != 0
        && pump.state().sti_shadow == Some(StiShadow::Clear))
    {
        assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
        assert!(pump.trace().len() < 50, "runaway");
    }
    assert_eq!(counter(&mut pump), 0);
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, VECTOR, 0).unwrap();
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert_eq!(
        outcome(&mut pump, VECTOR, result.exit),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 1,
            counter: 1,
            time_ns: 0,
        }
    );
    assert_eq!(entries(&pump, VECTOR)[0].1, PumpEntrySource::Boundary);
    // The frame's RIP is the HLT itself: it had not retired when the vector came.
    assert_eq!(frame(&mut pump), [built.hlt, 8, 0x202, STACK_TOP, 0x10]);
    assert_eq!(pump.irq_queue_len(), 0);
}

#[test]
fn pause_and_resume_keep_the_future_event() {
    let built = build_queue();
    let mut pump = load(&built);
    run_to_idle(&mut pump);
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 7, VECTOR, FUTURE_NS).unwrap();

    // A zero budget pauses before any step: the event, time and counter stay.
    let paused = pump.run(0, || false);
    assert_eq!(paused.exit, PumpExit::Budget);
    assert_eq!((paused.quanta, paused.retired), (0, 0));
    assert_eq!(pump.irq_queue_len(), 1);
    assert_eq!(pump.irq_queue_next_deadline(), Some(FUTURE_NS));
    assert_eq!((time(&pump), counter(&mut pump)), (0, 0));

    // One step is the idle advance: the clock reaches the deadline, the event is
    // still queued and undelivered.
    let step = pump.run(1, || false);
    assert_eq!((step.exit, step.quanta), (PumpExit::Budget, 1));
    assert_eq!((time(&pump), counter(&mut pump)), (FUTURE_NS, 0));
    assert_eq!(pump.irq_queue_len(), 1);

    // Resume: the event is delivered exactly once.
    let resumed = pump.run(10_000, || false);
    assert_eq!(resumed.exit, PumpExit::IdleHalt);
    assert_eq!(
        outcome(&mut pump, VECTOR, resumed.exit),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 1,
            counter: 1,
            time_ns: FUTURE_NS,
        }
    );
    assert_eq!(pump.irq_queue_len(), 0);
    // A further run does not deliver it again.
    assert_eq!(pump.run(10_000, || false).exit, PumpExit::IdleHalt);
    assert_eq!(counter(&mut pump), 1);
}

#[test]
fn reset_rejects_the_old_generation_and_drops_its_events() {
    let built = build_queue();
    let mut pump = load(&built);
    run_to_idle(&mut pump);
    let old = pump.irq_generation();
    pump.schedule_irq(old, 1, VECTOR, FUTURE_NS).unwrap();
    pump.schedule_irq(old, 2, VECTOR, 0).unwrap();
    assert_eq!(pump.irq_queue_len(), 2);

    let new = pump.reset_irq_queue().unwrap();
    assert_ne!(old, new);
    assert_eq!(pump.irq_generation(), new);
    assert_eq!(pump.irq_queue_len(), 0);
    assert_eq!(pump.irq_queue_next_deadline(), None);
    // The old token is refused for every operation.
    let stale = PumpQueueError::Queue(EventQueueError::StaleGeneration);
    assert_eq!(pump.schedule_irq(old, 3, VECTOR, 0), Err(stale));
    assert_eq!(pump.cancel_irq(old, 1), Err(stale));
    assert_eq!(pump.irq_queue_len(), 0);

    // The old events are not delivered and time does not move: no deadline.
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert_eq!(
        outcome(&mut pump, VECTOR, result.exit),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 0,
            counter: 0,
            time_ns: 0,
        }
    );
    assert_eq!(pump.apic().vector_state(VECTOR), (false, false));

    // The same key under the new generation is a fresh event, delivered once.
    pump.schedule_irq(new, 1, VECTOR, FUTURE_NS).unwrap();
    let result = pump.run(10_000, || false);
    assert_eq!(
        outcome(&mut pump, VECTOR, result.exit),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 1,
            counter: 1,
            time_ns: FUTURE_NS,
        }
    );
    assert_eq!(pump.irq_queue_len(), 0);
}

#[test]
fn two_due_events_with_one_vector_make_one_irr_edge_and_one_entry() {
    let built = build_queue();
    let mut pump = load(&built);
    run_to_idle(&mut pump);
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, VECTOR, 0).unwrap();
    pump.schedule_irq(generation, 2, VECTOR, 0).unwrap();
    let result = pump.run(10_000, || false);
    assert_eq!(
        outcome(&mut pump, VECTOR, result.exit),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 1,
            counter: 1,
            time_ns: 0,
        }
    );
    // One IRR rise, one merged event, one entry, one EOI.
    assert_eq!(count(&pump, &K::Irr { vector: VECTOR }), 1);
    assert_eq!(count(&pump, &K::Coalesced { vector: VECTOR }), 1);
    assert_eq!(pump.coalesced_irqs(), 1);
    assert_eq!(count(&pump, &K::Eoi { vector: VECTOR }), 1);
    assert_eq!(pump.irq_queue_len(), 0);
}

/// Scenario of the failed request: the APIC is globally disabled by the caller,
/// so `request_fixed_edge` fails for the due event. After it is enabled again
/// the guest runs and the due and future events are delivered.
fn failed_request_case(injection: Option<PumpInjection>) -> (Outcome, usize) {
    let built = build_queue();
    let mut pump = load(&built);
    if let Some(injection) = injection {
        pump = pump.with_injection(injection);
    }
    pump.apic().set_global_enabled(false).unwrap();
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, VECTOR, 0).unwrap();
    pump.schedule_irq(generation, 2, VECTOR, FUTURE_NS).unwrap();

    let failed = pump.run(10_000, || false);
    assert_eq!(
        failed.exit,
        PumpExit::Rejected(PumpRejection::QueueRequest {
            vector: VECTOR,
            error: DeviceError::UnsupportedConfiguration,
        })
    );
    // The guest did not move: the failure precedes the instruction.
    assert_eq!((failed.retired, pump.state().rip), (0, 0x4000));
    let kept = pump.irq_queue_len();

    pump.apic().set_global_enabled(true).unwrap();
    let result = pump.run(10_000, || false);
    let outcome = outcome(&mut pump, VECTOR, result.exit);
    (outcome, kept)
}

#[test]
fn failed_request_keeps_the_event_and_the_injection_loses_it() {
    let (accepted, accepted_kept) = failed_request_case(None);
    let (injected, injected_kept) = failed_request_case(Some(PumpInjection::PopBeforeRequest));
    // Accepted: the due event E1 stays queued, is delivered after the retry (the
    // request puts the vector in the IRR before the guest enables the APIC; the
    // wake follows STI; HLT) and the future event at FUTURE_NS.
    assert_eq!(accepted_kept, 2);
    assert_eq!(
        accepted,
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 2,
            counter: 2,
            time_ns: FUTURE_NS,
        }
    );
    // Injection: E1 was removed before the failing request and is lost.
    assert_eq!(injected_kept, 1);
    assert_eq!(
        injected,
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 1,
            counter: 1,
            time_ns: FUTURE_NS,
        }
    );
    assert_ne!(accepted, injected);
}

// ---- ISSUE-070 and the timer/queue minimum: the S4-27i2 timer loader ----------

fn timer_prologue() -> Vec<u8> {
    [
        mov_edi(),
        write_reg(0xf0, 0x1ff),     // SVR: software enable
        write_reg(0x3e0, 0x3),      // divide by 16
        write_reg(0x320, 0x2_0061), // periodic | vector 0x61
        write_reg(0x380, 625),      // Initial Count: arms the timer
    ]
    .concat()
}

/// `STI; loop: HLT; JMP loop` after the timer prologue.
fn timer_entry() -> Vec<u8> {
    [timer_prologue(), vec![0xfb, 0xf4, 0xeb, 0xfd]].concat()
}

fn load_timer(built: &Built) -> InterruptPump {
    load_stage4_interrupt_timer_probe(&built.image).unwrap()
}

#[test]
fn timer_vector_blocked_in_service_is_idle_halt_not_budget() {
    // The handler counts but writes no EOI: the timer vector stays in service.
    let no_eoi = [count_in_ram(), vec![0x48, 0xcf]].concat();
    let entry = timer_entry();
    let hlt = 0x4000 + timer_prologue().len() as u64 + 1;
    let built = build_with(TIMER_VECTOR, entry, no_eoi, hlt);

    for budget in [100, 10_000] {
        let mut pump = load_timer(&built);
        let result = pump.run(budget, || false);
        // First expiry at P is delivered; the second (2P) raises the IRR bit, which
        // stays blocked behind the in-service bit: nothing can wake the CPU, so the
        // pump is idle instead of advancing the clock until the budget is spent.
        assert_eq!(result.exit, PumpExit::IdleHalt, "budget {budget}");
        assert!(result.quanta < 100, "quanta {}", result.quanta);
        assert_eq!(
            outcome(&mut pump, TIMER_VECTOR, result.exit),
            Outcome {
                stop: PumpExit::IdleHalt,
                wakeups: 1,
                counter: 1,
                time_ns: 2 * PERIOD_NS,
            },
            "budget {budget}"
        );
        assert_eq!(pump.apic().vector_state(TIMER_VECTOR), (true, true));
        assert_eq!(pump.apic().eligible(), None);
        assert!(pump.is_waiting());
        // The periodic timer stays armed, but the clock does not move any more.
        assert_eq!(pump.apic().next_timer_deadline(), Some(3 * PERIOD_NS));
        let again = pump.run(budget, || false);
        assert_eq!((again.exit, again.quanta), (PumpExit::IdleHalt, 0));
        assert_eq!(time(&pump), 2 * PERIOD_NS);
    }
}

#[test]
fn idle_advance_goes_to_the_earlier_of_the_timer_and_the_queue_deadline() {
    // Counting handler with EOI; it cancels the timer after five wake-ups.
    let cancel = [mov_eax(0), store_eax(0x380)].concat();
    let mut handler = count_in_ram();
    handler.extend([0x83, 0xf8, 5]); // CMP EAX,5
    handler.extend([0x75, cancel.len() as u8]); // JNE ack
    handler.extend(cancel);
    handler.extend(write_reg(0xb0, 0)); // EOI
    handler.extend([0x48, 0xcf]);
    let hlt = 0x4000 + timer_prologue().len() as u64 + 1;
    let built = build_with(TIMER_VECTOR, timer_entry(), handler, hlt);

    let mut pump = load_timer(&built);
    run_to_idle_timer(&mut pump, hlt);
    // The queue event (vector 0x61, 50 us) is earlier than the first timer expiry
    // (P = 100 us): the wake is at 50 us, the following ones at the timer's k * P.
    let generation = pump.irq_generation();
    pump.schedule_irq(generation, 1, TIMER_VECTOR, PERIOD_NS / 2)
        .unwrap();
    let mut entry_times = Vec::new();
    let mut seen = 0;
    let stop = loop {
        let result = pump.run(1, || false);
        for event in &pump.trace()[seen..] {
            if matches!(event.kind, K::Entry { .. }) {
                entry_times.push(time(&pump));
            }
        }
        seen = pump.trace().len();
        if result.exit != PumpExit::Budget {
            break result.exit;
        }
        assert!(pump.trace().len() < 200, "runaway");
    };
    assert_eq!(stop, PumpExit::IdleHalt);
    assert_eq!(
        entry_times,
        vec![
            PERIOD_NS / 2,
            PERIOD_NS,
            2 * PERIOD_NS,
            3 * PERIOD_NS,
            4 * PERIOD_NS
        ]
    );
    assert_eq!(
        outcome(&mut pump, TIMER_VECTOR, stop),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 5,
            counter: 5,
            time_ns: 4 * PERIOD_NS,
        }
    );
    assert_eq!(pump.irq_queue_len(), 0);
}

/// The timer prologue arms the timer, so the CPU idles only when the pump is
/// stopped by the caller at the first wait.
fn run_to_idle_timer(pump: &mut InterruptPump, hlt: u64) {
    while !pump.is_waiting() {
        assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    }
    assert_eq!(pump.state().rip, hlt + 1);
    assert_eq!(time(pump), 0);
}
