//! S4-27i2: owner-driven periodic APIC timer in the flat event pump
//! (`InterruptPump`): dispatch on every boundary, idle advance in the halt wait,
//! 0 ns per instruction, and the relaxed clock gate of the STI quantum.
//!
//! The images are assembled here byte-for-byte (mirroring the Python builder's
//! validated encodings). Expected values come from the Intel SDM and the
//! timer formula, never from interpreter output:
//!
//! * APIC timer (SDM Vol. 3A 10.5.4): the count runs at the timer input
//!   frequency divided by the Divide Configuration divisor (value 0x3 = divide
//!   by 16); a periodic timer reloads the Initial Count every expiry. Initial
//!   Count 625, divisor 16, timer input 100 MHz: 625 * 16 / 100e6 s = 100 us,
//!   so the period P is 100_000 ns. The first expiry is at P (the guest arms
//!   the timer at virtual time 0: instructions take 0 ns), the k-th at k * P,
//!   so after N wake-ups the virtual time is N * P.
//! * 64-bit interrupt gate frame: SS, RSP, RFLAGS, CS, RIP (five qwords); the
//!   stack top 0x6ff0 is 16-byte aligned, so RSP becomes 0x6ff0 - 40 = 0x6fc8;
//!   the pushed RFLAGS is 0x202. A wake from HLT pushes the RIP after HLT.
//! * EOI clears the in-service bit, so IRR and ISR of the timer vector are
//!   clear after the last handler.

use od_machine::{
    InterruptPump, PumpEntrySource, PumpExit, PumpInjection, PumpRejection, PumpTraceKind as K,
    STAGE4_INTERRUPT_TIMER_PROBE_COUNTER_GPA as COUNTER,
    STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_BYTES as IMAGE_BYTES,
    STAGE4_INTERRUPT_TIMER_PROBE_VECTOR as VECTOR, load_stage4_interrupt_timer_probe,
};

const BASE: usize = 0x1000;
const FRAME: u64 = 0x6fc8;
const STACK_TOP: u64 = 0x6ff0;
const WAKEUPS: u8 = 5;
/// 625 * 16 / 100 MHz = 100 us.
const PERIOD_NS: u64 = 100_000;

#[derive(Clone, Copy, PartialEq)]
enum Timer {
    /// Periodic, unmasked: the accepted profile.
    Periodic,
    /// The guest never programs the timer.
    Unarmed,
    /// Periodic and armed but LVT-masked: expiry would raise no interrupt.
    Masked,
}

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

fn prologue(timer: Timer) -> Vec<u8> {
    let mut code = mov_edi();
    code.extend(write_reg(0xf0, 0x1ff)); // SVR: software enable
    if timer != Timer::Unarmed {
        code.extend(write_reg(0x3e0, 0x3)); // divide by 16
        let lvt = if timer == Timer::Masked {
            0x3_0061 // masked | periodic | vector 0x61
        } else {
            0x2_0061 // periodic | vector 0x61
        };
        code.extend(write_reg(0x320, lvt));
        code.extend(write_reg(0x380, 625)); // Initial Count: arms the timer
    }
    code
}

/// `STI; loop: HLT; JMP loop`.
fn entry(timer: Timer) -> Vec<u8> {
    [prologue(timer), vec![0xfb, 0xf4, 0xeb, 0xfd]].concat()
}

fn handler(wakeups: u8) -> Vec<u8> {
    let cancel = [mov_eax(0), store_eax(0x380)].concat();
    let mut code = mov_edi();
    code.extend([0xbe, 0x00, 0x50, 0x00, 0x00]); // MOV ESI,0x5000
    code.extend([0x8b, 0x06]); // MOV EAX,[RSI]
    code.extend([0x83, 0xc0, 0x01]); // ADD EAX,1
    code.extend([0x89, 0x06]); // MOV [RSI],EAX
    code.extend([0x83, 0xf8, wakeups]); // CMP EAX,imm8
    code.extend([0x75, cancel.len() as u8]); // JNE ack
    code.extend(cancel);
    code.extend(write_reg(0xb0, 0)); // EOI
    code.extend([0x48, 0xcf]); // IRETQ
    code
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
    /// Address of the first HLT.
    hlt: u64,
    handler: u64,
}

fn build_with(entry: Vec<u8>, handler: Vec<u8>, hlt: u64) -> Built {
    let mut image = vec![0; IMAGE_BYTES];
    for (i, d) in [0_u64, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff]
        .iter()
        .enumerate()
    {
        put(&mut image, 0x2000 + 8 * i, &d.to_le_bytes());
    }
    let handler_gpa = 0x4000 + entry.len() as u64;
    put(
        &mut image,
        0x3000 + usize::from(VECTOR) * 16,
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

fn build(timer: Timer, wakeups: u8) -> Built {
    // SDM-derived layout: HLT follows the prologue and the one-byte STI.
    let hlt = 0x4000 + prologue(timer).len() as u64 + 1;
    build_with(entry(timer), handler(wakeups), hlt)
}

fn load(built: &Built) -> InterruptPump {
    load_stage4_interrupt_timer_probe(&built.image).unwrap()
}

fn counter(pump: &mut InterruptPump) -> u64 {
    pump.peek_ram_qword(COUNTER).unwrap() & 0xffff_ffff
}

fn time(pump: &InterruptPump) -> u64 {
    pump.virtual_time_ns().unwrap()
}

fn entries(pump: &InterruptPump) -> Vec<(u64, PumpEntrySource)> {
    pump.trace()
        .iter()
        .filter_map(|event| match event.kind {
            K::Entry { vector, source } if vector == VECTOR => Some((event.rip, source)),
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

/// Evidence that distinguishes the accepted run from the injection.
#[derive(Debug, PartialEq)]
struct Outcome {
    stop: PumpExit,
    wakeups: usize,
    counter: u64,
    time_ns: u64,
}

fn outcome(pump: &mut InterruptPump, stop: PumpExit) -> Outcome {
    Outcome {
        stop,
        wakeups: entries(pump).len(),
        counter: counter(pump),
        time_ns: time(pump),
    }
}

#[test]
fn periodic_timer_wakes_n_times_with_counter_and_time_n_times_period() {
    let built = build(Timer::Periodic, WAKEUPS);
    let mut pump = load(&built);

    // Run the prologue and STI + HLT: the CPU is halted at virtual time 0 and the
    // armed timer has its first deadline at one period.
    while !pump.is_waiting() {
        assert_eq!(pump.run(1, || false).exit, PumpExit::Budget);
    }
    assert_eq!(pump.state().rip, built.hlt + 1);
    assert_eq!(time(&pump), 0, "instructions take 0 ns");
    assert_eq!(pump.apic().next_timer_deadline(), Some(PERIOD_NS));
    assert_eq!(counter(&mut pump), 0);

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
        seen = pump.trace().len();
        if result.exit != PumpExit::Budget {
            break result.exit;
        }
        assert!(pump.trace().len() < 200, "runaway");
    };

    assert_eq!(stop, PumpExit::IdleHalt);
    assert_eq!(
        outcome(&mut pump, stop),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: usize::from(WAKEUPS),
            counter: u64::from(WAKEUPS),
            time_ns: u64::from(WAKEUPS) * PERIOD_NS,
        }
    );
    // The k-th wake happens exactly at k * P.
    let expected_times: Vec<u64> = (1..=u64::from(WAKEUPS)).map(|k| k * PERIOD_NS).collect();
    assert_eq!(entry_times, expected_times);
    assert!(
        entries(&pump)
            .iter()
            .all(|entry| *entry == (built.handler, PumpEntrySource::HaltWake))
    );
    // Every frame carried the RIP after HLT: IRETQ always returned to the JMP.
    assert_eq!(
        [FRAME, FRAME + 8, FRAME + 16, FRAME + 24, FRAME + 32]
            .map(|address| pump.peek_ram_qword(address).unwrap()),
        [built.hlt + 1, 8, 0x202, STACK_TOP, 0x10]
    );
    // IRR/ISR are clear after the last EOI; the guest cancelled the timer.
    assert_eq!(pump.apic().vector_state(VECTOR), (false, false));
    assert_eq!(pump.apic().next_timer_deadline(), None);
    assert!(pump.is_waiting());
    assert_eq!(
        (pump.state().rip, pump.state().rflags),
        (built.hlt + 1, 0x202)
    );
    assert_eq!(pump.state().gpr[od_isa::Register::Rsp as usize], STACK_TOP);
    // Trace: one IRR rise, one entry, one EOI, one IRETQ per wake.
    let n = usize::from(WAKEUPS);
    assert_eq!(count(&pump, &K::Irr { vector: VECTOR }), n);
    assert_eq!(count(&pump, &K::Eoi { vector: VECTOR }), n);
    assert_eq!(count(&pump, &K::Iretq), n);
    assert_eq!(count(&pump, &K::Sti), 1);
    assert_eq!(count(&pump, &K::Hlt), n + 1);
}

#[test]
fn injection_without_timer_dispatch_stays_idle_with_zero_counter() {
    let built = build(Timer::Periodic, WAKEUPS);
    let mut pump = load(&built).with_injection(PumpInjection::NoTimerDispatch);
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert!(pump.is_waiting());
    let injected = outcome(&mut pump, result.exit);
    assert_eq!(
        injected,
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 0,
            counter: 0,
            time_ns: 0,
        }
    );
    // The timer stays armed and never reaches the IRR.
    assert_eq!(pump.apic().next_timer_deadline(), Some(PERIOD_NS));
    assert_eq!(pump.apic().vector_state(VECTOR), (false, false));

    // The accepted run differs in (wakeups, counter, time).
    let mut accepted = load(&built);
    let result = accepted.run(10_000, || false);
    let accepted = outcome(&mut accepted, result.exit);
    assert_ne!(accepted, injected);
    assert_eq!(
        (accepted.wakeups, accepted.counter, accepted.time_ns),
        (5, 5, 5 * PERIOD_NS)
    );
}

#[test]
fn idle_without_deadline_does_not_advance_time() {
    for timer in [Timer::Unarmed, Timer::Masked] {
        let built = build(timer, WAKEUPS);
        let mut pump = load(&built);
        let result = pump.run(10_000, || false);
        assert_eq!(result.exit, PumpExit::IdleHalt);
        assert!(pump.is_waiting());
        assert_eq!(
            outcome(&mut pump, result.exit),
            Outcome {
                stop: PumpExit::IdleHalt,
                wakeups: 0,
                counter: 0,
                time_ns: 0,
            }
        );
        // No deadline: unarmed has no event; masked expiry would raise no IRQ.
        assert_eq!(pump.apic().next_timer_deadline(), None);
        assert_eq!(pump.state().rip, built.hlt + 1);
        // Running again changes nothing: still idle at time 0.
        let again = pump.run(10_000, || false);
        assert_eq!((again.exit, again.quanta), (PumpExit::IdleHalt, 0));
        assert_eq!(time(&pump), 0);
    }
}

#[test]
fn halted_with_if_clear_is_idle_even_with_an_armed_timer() {
    // CLI; HLT: no maskable vector can wake the CPU, so the clock is not advanced.
    let entry = [prologue(Timer::Periodic), vec![0xfa, 0xf4, 0xeb, 0xfd]].concat();
    let hlt = 0x4000 + prologue(Timer::Periodic).len() as u64 + 1;
    let built = build_with(entry, handler(WAKEUPS), hlt);
    let mut pump = load(&built);
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert_eq!(
        outcome(&mut pump, result.exit),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 0,
            counter: 0,
            time_ns: 0,
        }
    );
    assert_eq!(pump.apic().next_timer_deadline(), Some(PERIOD_NS));
}

#[test]
fn sti_quantum_accepts_a_session_clock() {
    // S4-27i2: ослаблен clock gate. The pump's session owns a virtual clock; STI
    // still goes through `run_sti_apic_quantum`, which no longer refuses it.
    let built = build(Timer::Periodic, WAKEUPS);
    let mut pump = load(&built);
    while count(&pump, &K::Sti) == 0 {
        let result = pump.run(1, || false);
        assert_eq!(result.exit, PumpExit::Budget, "STI must not be rejected");
        assert_eq!(result.retired, 1);
    }
    assert_eq!(pump.state().rflags & 0x200, 0x200);
    assert_eq!(pump.state().rip, built.hlt);
    assert_eq!(pump.virtual_time_ns(), Some(0));
}

#[test]
fn rdtsc_inside_the_sti_quantum_is_still_rejected() {
    // STI; RDTSC: the instruction after STI runs in the STI shadow, i.e. in the
    // STI quantum, which has no TSC source.
    let entry = [mov_edi(), vec![0xfb, 0x0f, 0x31, 0xf4]].concat();
    let rdtsc = 0x4000 + mov_edi().len() as u64 + 1;
    let built = build_with(entry, handler(WAKEUPS), rdtsc);
    let mut pump = load(&built);
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget); // MOV EDI
    assert_eq!(pump.run(1, || false).exit, PumpExit::Budget); // STI
    let before = pump.state().clone();
    let result = pump.run(1, || false);
    assert_eq!(
        result.exit,
        PumpExit::Rejected(PumpRejection::StiQuantumProfile)
    );
    assert_eq!(result.retired, 0);
    assert_eq!(pump.state(), &before, "the refusal changes no guest state");
    assert_eq!(pump.state().rip, rdtsc);
    // The refusal is final.
    assert_eq!(
        pump.run(1, || false).exit,
        PumpExit::Rejected(PumpRejection::StiQuantumProfile)
    );
}

#[test]
fn rdtsc_outside_the_sti_shadow_is_executed_by_the_ordinary_path() {
    // STI; NOP; RDTSC; HLT: RDTSC is at a clear boundary (IF=1, no shadow, no
    // eligible vector), so it is an ordinary instruction of the clock session.
    let entry = [mov_edi(), vec![0xfb, 0x90, 0x0f, 0x31, 0xf4]].concat();
    let hlt = 0x4000 + mov_edi().len() as u64 + 4;
    let built = build_with(entry, handler(WAKEUPS), hlt);
    let mut pump = load(&built);
    let result = pump.run(4, || false);
    assert_eq!((result.exit, result.retired), (PumpExit::Budget, 4));
    assert_eq!(pump.state().rip, hlt);
    // 0 ns per instruction: the virtual TSC is still 0.
    assert_eq!(pump.state().gpr[od_isa::Register::Rax as usize], 0);
    assert_eq!(pump.state().gpr[od_isa::Register::Rdx as usize], 0);
}
