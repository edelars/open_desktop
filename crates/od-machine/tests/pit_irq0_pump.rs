//! S4-29f: PIT channel 0 -> OUT0 -> IOAPIC INTIN2 (GSI 2) -> Local APIC -> guest
//! handler, driven by the event pump with the PIT bridge as its owner hook.
//!
//! The image is assembled here byte-for-byte (mirroring the Python builder's
//! validated encodings). Expected values come from the PIT formula and the
//! queue rules, never from interpreter output:
//!
//! * PIT input 1 193 182 Hz (S4-29c): the tick index at virtual time `t` is
//!   `T(t) = floor(t * 1_193_182 / 1e9)`. The guest loads channel 0 at time 0 (an
//!   instruction takes 0 ns), so a mode 2 channel with reload `N` has OUT0 rising
//!   edges at ticks `k * N`, `k >= 1`, and the edge at tick `E` is due at the first
//!   nanosecond with `T(t) >= E`, i.e. `ceil(E * 1e9 / 1_193_182)`.
//! * With `N = 1193` (about 1 ms): d1 = 999_848, d2 = 1_999_695, d3 = 2_999_543 ns.
//!   Mode 0 gives one edge, at tick `N`. The first program of the guest (count
//!   1790, edge due at 1_500_191 ns) is replaced by the second one at the same
//!   virtual time.
//! * The halted CPU's clock jumps to the next bridge deadline (idle advance), so
//!   the time after the k-th wake is exactly d_k.
//! * Quanta (one per pump step): 42 instructions up to and including STI, the HLT
//!   is the 43rd step; each idle advance is one step.

use od_isa::Register;
use od_machine::{
    EventGeneration, Pic8259InitPhase, PitChannel, PumpEntrySource, PumpExit, PumpHostEvent,
    PumpInjection, PumpRejection, PumpTraceEvent, PumpTraceKind as K, PumpTraceMode,
    PumpTraceModeError, STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES, STAGE4_PIT_IRQ0_PROBE_VECTOR as VECTOR,
    Stage4PitIrq0Injection, Stage4PitIrq0Probe, Stage4PitIrq0ProbeLoadError, isa_irq_vector,
    load_stage4_pit_irq0_probe,
};

const BASE: usize = 0x1000;
const HLT: u64 = 0x407d;
const HANDLER: u64 = 0x4080;
const PIT_HZ: u128 = 1_193_182;
const COUNT_A: u16 = 1790;
const COUNT_B: u16 = 1193;
/// Ticks `1193 * k` converted with `ceil(tick * 1e9 / 1_193_182)`.
const D1: u64 = 999_848;
const D2: u64 = 1_999_695;
const D3: u64 = 2_999_543;
/// Deadline of the edge of the replaced program (count 1790).
const OLD_A: u64 = 1_500_191;
/// Pump steps up to and including the HLT step.
const STEPS_TO_HLT: u64 = 43;

/// First nanosecond whose PIT tick index reaches `tick`.
fn deadline(tick: u64) -> u64 {
    (u128::from(tick) * 1_000_000_000).div_ceil(PIT_HZ) as u64
}

fn mov_edi() -> Vec<u8> {
    vec![0xbf, 0x00, 0x00, 0xe0, 0xfe]
}

fn mov_eax(value: u32) -> Vec<u8> {
    let mut code = vec![0xb8];
    code.extend(value.to_le_bytes());
    code
}

/// `MOV EAX,value; MOV [RDI+disp],EAX`.
fn write_reg(disp: u32, value: u32) -> Vec<u8> {
    let mut code = mov_eax(value);
    code.extend([0x89, 0x87]);
    code.extend(disp.to_le_bytes());
    code
}

/// `MOV EDX,port; MOV AL,value; OUT DX,AL`.
fn out8(port: u32, value: u8) -> Vec<u8> {
    let mut code = vec![0xba];
    code.extend(port.to_le_bytes());
    code.extend([0xb0, value, 0xee]);
    code
}

/// What differs between the images of the tests.
#[derive(Clone, Copy)]
struct Program {
    /// Control word of channel 0 (LSB+MSB, binary): `0x34` mode 2, `0x30` mode 0.
    control: u8,
    first: u16,
    second: u16,
    /// Interrupts after which the handler reads the PIC IRR and shuts channel 0 down.
    stop_after: u8,
}

const MODE2: Program = Program {
    control: 0x34,
    first: COUNT_A,
    second: COUNT_B,
    stop_after: 3,
};

fn program_channel(control: u8, count: u16) -> Vec<u8> {
    [
        out8(0x43, control),
        out8(0x40, count as u8),
        out8(0x40, (count >> 8) as u8),
    ]
    .concat()
}

fn entry(program: Program) -> Vec<u8> {
    let mut code = mov_edi();
    code.extend(write_reg(0xf0, 0x1ff)); // SVR: software enable
    // Master 8259: ICW1..ICW4 (vector base 0x30), IMR = 0xff.
    for (port, value) in [
        (0x20, 0x11),
        (0x21, 0x30),
        (0x21, 0x04),
        (0x21, 0x01),
        (0x21, 0xff),
    ] {
        code.extend(out8(port, value));
    }
    code.extend([0xbe, 0x00, 0x00, 0xc0, 0xfe]); // MOV ESI,0xFEC00000 (IOAPIC)
    code.extend(mov_eax(0x14)); // IOREGSEL = entry 2 low dword
    code.extend([0x89, 0x06]); // MOV [RSI],EAX
    code.extend(mov_eax(u32::from(VECTOR))); // edge, unmasked, fixed, vector
    code.extend([0x89, 0x46, 0x10]); // MOV [RSI+0x10],EAX (IOWIN)
    code.extend(program_channel(program.control, program.first));
    code.extend(program_channel(program.control, program.second)); // reprogram
    code.extend([0xfb, 0xf4, 0xeb, 0xfd]); // STI; loop: HLT; JMP loop
    code
}

fn handler(program: Program) -> Vec<u8> {
    let stop = [
        out8(0x20, 0x0a),          // OCW3: read IRR
        vec![0xba, 0x20, 0, 0, 0], // MOV EDX,0x20
        vec![0xec],                // IN AL,DX
        out8(0x43, 0x30),          // shutdown of channel 0
    ]
    .concat();
    let mut code = mov_edi();
    code.extend([0xbe, 0x00, 0x50, 0x00, 0x00]); // MOV ESI,0x5000
    code.extend([0x8b, 0x06]); // MOV EAX,[RSI]
    code.extend([0x83, 0xc0, 0x01]); // ADD EAX,1
    code.extend([0x89, 0x06]); // MOV [RSI],EAX
    code.extend([0x83, 0xf8, program.stop_after]); // CMP EAX,imm8
    code.extend([0x75, stop.len() as u8]); // JNE ack
    code.extend(stop);
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

fn build(program: Program) -> Vec<u8> {
    let entry = entry(program);
    assert_eq!(0x4000 + entry.len() as u64, HANDLER);
    let mut image = vec![0; STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES];
    for (i, d) in [0_u64, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff]
        .iter()
        .enumerate()
    {
        put(&mut image, 0x2000 + 8 * i, &d.to_le_bytes());
    }
    put(
        &mut image,
        0x3000 + usize::from(VECTOR) * 16,
        &gate(HANDLER),
    );
    put(&mut image, 0x4000, &entry);
    put(&mut image, HANDLER as usize, &handler(program));
    image
}

fn load(image: &[u8], injection: Option<Stage4PitIrq0Injection>) -> Stage4PitIrq0Probe {
    load_stage4_pit_irq0_probe(image, injection).unwrap()
}

/// Evidence that distinguishes the accepted run from the injections.
#[derive(Debug, PartialEq)]
struct Outcome {
    stop: PumpExit,
    wakeups: usize,
    counter: u32,
    time_ns: u64,
}

/// Run one pump step at a time (bounded by `budget`), sampling the virtual time
/// at every entry of the INTIN2 vector.
fn drive(probe: &mut Stage4PitIrq0Probe, budget: u64) -> (PumpExit, Vec<u64>) {
    let mut times = Vec::new();
    let mut seen = 0;
    let mut steps = 0;
    let stop = loop {
        if steps == budget {
            break PumpExit::Budget;
        }
        let result = probe.run(1, || false);
        steps += 1;
        for event in &probe.pump().trace()[seen..] {
            if matches!(event.kind, K::Entry { vector, .. } if vector == VECTOR) {
                times.push(probe.time_ns());
            }
        }
        seen = probe.pump().trace().len();
        if result.exit != PumpExit::Budget {
            break result.exit;
        }
    };
    (stop, times)
}

fn outcome(probe: &mut Stage4PitIrq0Probe, stop: PumpExit) -> Outcome {
    Outcome {
        stop,
        wakeups: probe.wakeups(),
        counter: probe.counter(),
        time_ns: probe.time_ns(),
    }
}

fn host_events(probe: &Stage4PitIrq0Probe, name: &str) -> Vec<PumpHostEvent> {
    probe
        .pump()
        .trace()
        .iter()
        .filter_map(|event| match &event.kind {
            K::Host(host) if host.name == name => Some(host.clone()),
            _ => None,
        })
        .collect()
}

fn count(probe: &Stage4PitIrq0Probe, wanted: &K) -> usize {
    probe
        .pump()
        .trace()
        .iter()
        .filter(|event| &event.kind == wanted)
        .count()
}

#[test]
fn deadlines_follow_the_pit_formula() {
    // The constants of this file are the formula's values.
    assert_eq!(
        [1, 2, 3].map(|k| deadline(u64::from(COUNT_B) * k)),
        [D1, D2, D3]
    );
    assert_eq!(deadline(u64::from(COUNT_A)), OLD_A);
    // The deadline is the first nanosecond whose tick index reaches the edge.
    for tick in [1_u64, 1193, 1790, 2386, 3579] {
        let t = deadline(tick);
        let ticks = |ns: u64| (u128::from(ns) * PIT_HZ / 1_000_000_000) as u64;
        assert!(ticks(t) >= tick && ticks(t - 1) < tick, "tick {tick}");
    }
}

#[test]
fn mode2_gives_n_irq0_through_gsi2_with_counter_and_time() {
    let mut probe = load(&build(MODE2), None);
    let (stop, times) = drive(&mut probe, 500);

    assert_eq!(stop, PumpExit::IdleHalt);
    assert_eq!(
        outcome(&mut probe, stop),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 3,
            counter: 3,
            time_ns: D3,
        }
    );
    // The k-th wake is exactly at the k-th edge; the replaced program's edge
    // (OLD_A) is never delivered.
    assert_eq!(times, [D1, D2, D3]);
    // Every entry has the vector of IOAPIC entry 2, at the handler's first
    // instruction, from the halt wait.
    let entries: Vec<_> = probe
        .pump()
        .trace()
        .iter()
        .filter_map(|event| match event.kind {
            K::Entry { vector, source } => Some((vector, source, event.rip)),
            _ => None,
        })
        .collect();
    assert_eq!(
        entries,
        vec![(VECTOR, PumpEntrySource::HaltWake, HANDLER); 3]
    );
    // Each entry has its own EOI and IRETQ; the vector is clear afterwards.
    assert_eq!(count(&probe, &K::Eoi { vector: VECTOR }), 3);
    assert_eq!(count(&probe, &K::Iretq), 3);
    assert_eq!(count(&probe, &K::Irr { vector: VECTOR }), 3);
    assert_eq!(probe.pump().apic().vector_state(VECTOR), (false, false));
    assert_eq!(probe.pump().state().rip, HLT + 1);
    assert_eq!(probe.pump().state().gpr[Register::Rsp as usize], 0x6ff0);
    assert_eq!(probe.pump().state().rflags, 0x202);
    // Five scheduled edges (the old program, d1, d2, d3, d4), three delivered; the
    // shutdown in the third handler dropped d4. Five generation changes: control
    // word, count, control word, count, shutdown.
    let stats = probe.stats();
    assert_eq!(
        (
            stats.edges,
            stats.scheduled,
            stats.generation_changes,
            stats.cancelled
        ),
        (3, 5, 5, 2)
    );
}

#[test]
fn mode0_gives_one_irq0_and_no_more() {
    let program = Program {
        control: 0x30,
        stop_after: 1,
        ..MODE2
    };
    let mut probe = load(&build(program), None);
    let (stop, times) = drive(&mut probe, 500);
    assert_eq!(
        outcome(&mut probe, stop),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 1,
            counter: 1,
            time_ns: D1,
        }
    );
    assert_eq!(times, [D1]);
    // Scheduled: the replaced program and the edge; no chain after a mode 0 edge.
    // Changes: control word, count, control word, count, shutdown. Dropped: the
    // replaced program's event.
    let stats = probe.stats();
    assert_eq!(
        (
            stats.edges,
            stats.scheduled,
            stats.generation_changes,
            stats.cancelled
        ),
        (1, 2, 5, 1)
    );
    assert_eq!(probe.pending_events(), 0);

    // Mode 0 without the guest's shutdown: still exactly one edge.
    let program = Program {
        stop_after: 100,
        ..program
    };
    let mut probe = load(&build(program), None);
    let (stop, times) = drive(&mut probe, 500);
    assert_eq!(stop, PumpExit::IdleHalt);
    assert_eq!((probe.wakeups(), probe.counter()), (1, 1));
    assert_eq!(times, [D1]);
    assert_eq!(probe.stats().edges, 1);
    assert_eq!(
        probe.stats().generation_changes,
        4,
        "no shutdown, no fifth change"
    );
}

#[test]
fn reprogram_drops_the_old_generation_and_its_event_is_never_delivered() {
    let mut probe = load(&build(MODE2), None);
    let first_generation: EventGeneration = probe.generation();
    // Run the program, STI and HLT: the CPU is halted at virtual time 0.
    while !probe.pump().is_waiting() {
        assert_eq!(probe.run(1, || false).exit, PumpExit::Budget);
    }
    assert_eq!(probe.time_ns(), 0);
    // The guest programmed twice: the event of the first program (OLD_A) was
    // scheduled and then dropped with its generation; only the second program's
    // edge is pending.
    assert_eq!(
        host_events(&probe, "pit_schedule")
            .iter()
            .map(|event| event.a)
            .collect::<Vec<_>>(),
        [OLD_A, D1]
    );
    assert_eq!(
        host_events(&probe, "pit_cancel")[2].a,
        1,
        "one event dropped"
    );
    assert!(probe.is_stale(first_generation));
    assert_eq!(probe.pending_events(), 1);
    assert_eq!(probe.next_event_deadline(), Some(D1));
    assert_eq!(probe.pit().count(PitChannel::Channel0), Some(COUNT_B));
    // A token of the old generation stays refused for the whole run.
    let (stop, times) = drive(&mut probe, 500);
    assert_eq!(stop, PumpExit::IdleHalt);
    assert!(!times.contains(&OLD_A));
    assert_eq!(times, [D1, D2, D3]);
    assert!(probe.is_stale(first_generation));
}

#[test]
fn shutdown_control_word_0x30_cancels_the_pending_event() {
    let mut probe = load(&build(MODE2), None);
    let (stop, _) = drive(&mut probe, 500);
    assert_eq!(stop, PumpExit::IdleHalt);
    // The third handler wrote 0x30 to port 0x43 after d4 was scheduled.
    let cancels = host_events(&probe, "pit_cancel");
    assert_eq!(
        cancels.last().map(|event| event.a),
        Some(1),
        "d4 was dropped"
    );
    assert_eq!(probe.pending_events(), 0);
    assert_eq!(probe.next_event_deadline(), None);
    assert_eq!(probe.pit().count(PitChannel::Channel0), None);
    let outs: Vec<_> = host_events(&probe, "port_out")
        .iter()
        .map(|event| (event.a, event.b))
        .collect();
    assert_eq!(outs.last(), Some(&(0x43, 0x30)));
    // More running changes nothing: no deadline, no wake, time stands at d3.
    let before = (probe.wakeups(), probe.counter(), probe.time_ns());
    assert_eq!(probe.run(100, || false).exit, PumpExit::IdleHalt);
    assert_eq!(before, (3, 3, D3));
    assert_eq!((probe.wakeups(), probe.counter(), probe.time_ns()), before);
}

#[test]
fn machine_reset_of_the_pit_cancels_the_pending_event() {
    let mut probe = load(&build(MODE2), None);
    while !probe.pump().is_waiting() {
        probe.run(1, || false);
    }
    let token = probe.generation();
    assert_eq!(probe.pending_events(), 1);
    probe.reset_pit().unwrap();
    assert!(probe.is_stale(token));
    assert_eq!(probe.pending_events(), 0);
    // After reset channel 0 is loaded with the fixed count, not counting (S4-38L7bm).
    assert_eq!(probe.pit().count(PitChannel::Channel0), Some(0));
    // Nothing can wake the guest: idle halt at time 0 with no interrupt.
    assert_eq!(probe.run(100, || false).exit, PumpExit::IdleHalt);
    assert_eq!(
        (probe.wakeups(), probe.counter(), probe.time_ns()),
        (0, 0, 0)
    );
}

#[test]
fn out0_goes_to_intin2_only_and_the_pic_ir0_gets_nothing() {
    let mut probe = load(&build(MODE2), None);
    let (stop, _) = drive(&mut probe, 500);
    assert_eq!(stop, PumpExit::IdleHalt);
    // The guest initialised the master (vector base 0x30, everything masked);
    // the PIC state is exactly that, and the slave was not touched.
    let master = probe.pic_master().snapshot();
    assert_eq!(master.phase, Pic8259InitPhase::Initialized);
    assert_eq!((master.imr, master.icw2), (0xff, Some(isa_irq_vector(0))));
    assert_eq!(
        probe.pic_slave().snapshot().phase,
        Pic8259InitPhase::Uninitialized
    );
    // The guest read the PIC IRR (OCW3 0x0A, IN 0x20) while OUT0 had pulsed three
    // times: it is 0.
    let ins: Vec<_> = host_events(&probe, "port_in")
        .iter()
        .map(|event| (event.a, event.b))
        .collect();
    assert_eq!(ins, [(0x20, 0)]);
    // No vector of the PIC IR0 ever rose or entered; the only vector in the
    // trace is the INTIN2 one.
    let pic_vector = isa_irq_vector(0);
    assert_eq!(probe.pump().apic().vector_state(pic_vector), (false, false));
    assert!(probe.pump().trace().iter().all(|event| !matches!(
        event.kind,
        K::Irr { vector } | K::Entry { vector, .. } | K::Eoi { vector } if vector == pic_vector
    )));
    assert_eq!(probe.stats().edges, 3);
}

#[test]
fn masked_entry2_loses_every_edge_and_is_distinguishable() {
    let budget = 200;
    let mut probe = load(&build(MODE2), Some(Stage4PitIrq0Injection::MaskedEntry2));
    let (stop, times) = drive(&mut probe, budget);
    // Edges keep coming, the IOAPIC drops them (S4-29e): no entry, no counter. The
    // time after step 43 + m is d_m, so the budget ends the run at d_(budget - 43).
    let m = budget - STEPS_TO_HLT;
    assert_eq!(
        outcome(&mut probe, stop),
        Outcome {
            stop: PumpExit::Budget,
            wakeups: 0,
            counter: 0,
            time_ns: deadline(u64::from(COUNT_B) * m),
        }
    );
    assert!(times.is_empty());
    assert_eq!(probe.stats().edges, m - 1, "d1..d(m-1) were presented");
    assert_eq!(probe.pump().apic().vector_state(VECTOR), (false, false));
    assert_eq!(count(&probe, &K::Irr { vector: VECTOR }), 0);
}

#[test]
fn missing_eoi_blocks_the_second_entry() {
    let budget = 200;
    let mut probe = load(&build(MODE2), Some(Stage4PitIrq0Injection::NoEoi));
    let (stop, times) = drive(&mut probe, budget);
    // Step 43 HLT, 44 idle advance to d1, 45 wake, 46..=57 the handler, IRETQ, JMP
    // and HLT; from step 58 each step advances one edge (an edge raises the IRR bit
    // behind the in-service vector; nothing is eligible). After step 58 + j the
    // time is d_(2 + j), so the budget ends the run at d_(budget - 56).
    assert_eq!(
        outcome(&mut probe, stop),
        Outcome {
            stop: PumpExit::Budget,
            wakeups: 1,
            counter: 1,
            time_ns: deadline(u64::from(COUNT_B) * (budget - 56)),
        }
    );
    assert_eq!(times, [D1]);
    assert_eq!(count(&probe, &K::Eoi { vector: VECTOR }), 0);
    assert_eq!(probe.stats().edges, budget - 57);
    assert_eq!(probe.pump().apic().vector_state(VECTOR), (true, true));
}

#[test]
fn reprogram_without_a_generation_change_delivers_the_old_event() {
    let mut probe = load(&build(MODE2), Some(Stage4PitIrq0Injection::NoGeneration));
    let first_generation = probe.generation();
    let (stop, times) = drive(&mut probe, 500);
    // The old program's edge (OLD_A) is delivered between d1 and d2, so the
    // counter reaches 3 (and the guest's shutdown runs) at d2; the shutdown does not
    // drop d3 either, which is delivered as the fourth interrupt.
    assert_eq!(times, [D1, OLD_A, D2, D3]);
    assert_eq!(
        outcome(&mut probe, stop),
        Outcome {
            stop: PumpExit::IdleHalt,
            wakeups: 4,
            counter: 4,
            time_ns: D3,
        }
    );
    assert!(!probe.is_stale(first_generation));
    assert_eq!(probe.stats().generation_changes, 0);
}

#[test]
fn a_refused_intin2_delivery_keeps_the_event() {
    let mut probe = load(&build(MODE2), None);
    while !probe.pump().is_waiting() {
        probe.run(1, || false);
    }
    // The Local APIC refuses the request while globally disabled: the edge is
    // not consumed and the event stays in the queue (S4-29e).
    probe.pump().apic().set_global_enabled(false).unwrap();
    let result = probe.run(100, || false);
    assert!(
        matches!(result.exit, PumpExit::Rejected(PumpRejection::Host(ref detail)) if detail.contains("set_intin2(true)")),
        "{:?}",
        result.exit
    );
    assert_eq!(probe.pending_events(), 1);
    assert_eq!(probe.next_event_deadline(), Some(D1));
    assert_eq!(probe.stats().edges, 0);
}

#[test]
fn loader_refuses_wrong_images_and_missing_injection_sites() {
    let image = build(MODE2);
    assert!(matches!(
        load_stage4_pit_irq0_probe(&image[1..], None),
        Err(Stage4PitIrq0ProbeLoadError::ImageSize { .. })
    ));
    // A gate for another vector is refused.
    let mut extra_gate = image.clone();
    put(&mut extra_gate, 0x3000 + 0x30 * 16, &gate(HANDLER));
    assert!(matches!(
        load_stage4_pit_irq0_probe(&extra_gate, None),
        Err(Stage4PitIrq0ProbeLoadError::Layout(_))
    ));
    // An injection needs its patch site to hold the profile's own bytes.
    let mut moved = image.clone();
    moved[0x4044 - BASE + 1] = 0x61; // the unmask value is 0x61, not 0x60
    assert!(matches!(
        load_stage4_pit_irq0_probe(&moved, Some(Stage4PitIrq0Injection::MaskedEntry2)),
        Err(Stage4PitIrq0ProbeLoadError::Layout(_))
    ));
    let mut moved = image;
    moved[0x40b1 - BASE + 2] = 0xb1;
    assert!(matches!(
        load_stage4_pit_irq0_probe(&moved, Some(Stage4PitIrq0Injection::NoEoi)),
        Err(Stage4PitIrq0ProbeLoadError::Layout(_))
    ));
}

// ---- S4-37a5 (ISSUE-072): Bounded and Ring pump trace on a periodic PIT ----
//
// Choice of the stream: the real PIT image with the handler's stop disabled
// (`stop_after: 0`, the counter never equals 0 within the run), so channel 0
// delivers one IRQ0 per period forever and the guest needs no 1000 HLTs in the
// image. About 7 trace events per period make the 512-event bound end the run
// after ~170 periods; the budget below covers well over 1000 periods.

const FOREVER: Program = Program {
    stop_after: 0,
    ..MODE2
};
/// Pump steps of the long runs (every period costs about 20).
const LONG_BUDGET: u64 = 30_000;
const RING: usize = 256;

fn periodic(mode: PumpTraceMode, injection: Option<PumpInjection>) -> Stage4PitIrq0Probe {
    let mut probe = load(&build(FOREVER), None);
    probe.pump_mut().set_trace_mode(mode).unwrap();
    probe.pump_mut().set_injection(injection);
    probe
}

/// `seq` of the retained events runs without gaps or repeats.
fn seq_continuous(trace: &[PumpTraceEvent]) -> bool {
    trace.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1)
}

#[test]
fn bounded_trace_ends_a_periodic_pit_run_with_trace_limit() {
    let mut probe = periodic(PumpTraceMode::Bounded, None);
    assert_eq!(probe.pump().trace_mode(), PumpTraceMode::Bounded);
    let result = probe.run(LONG_BUDGET, || false);
    assert_eq!(result.exit, PumpExit::Rejected(PumpRejection::TraceLimit));
    let pump = probe.pump();
    assert_eq!(pump.trace().len(), od_machine::INTERRUPT_PUMP_TRACE_LIMIT);
    assert_eq!(pump.trace_dropped(), 0);
    assert_eq!(pump.trace_first_seq(), 0);
    assert_eq!(pump.trace_next_seq(), pump.trace().len() as u64);
    assert!(seq_continuous(pump.trace()));
    // The limit is hit far below 1000 periods.
    assert!(probe.counter() < 250, "counter {}", probe.counter());
}

#[test]
fn ring_trace_runs_a_periodic_pit_past_1000_periods_and_keeps_the_last_events() {
    let mut probe = periodic(PumpTraceMode::Ring(RING), None);
    let result = probe.run(LONG_BUDGET, || false);
    assert_eq!(result.exit, PumpExit::Budget);
    assert_eq!(result.quanta, LONG_BUDGET);
    assert!(probe.counter() >= 1000, "counter {}", probe.counter());

    // The reference run keeps everything (no event is dropped).
    let mut full = periodic(PumpTraceMode::Ring(1 << 20), None);
    assert_eq!(full.run(LONG_BUDGET, || false).exit, PumpExit::Budget);
    assert_eq!(full.pump().trace_dropped(), 0);
    let all = full.pump().trace();
    let total = all.len() as u64;
    assert!(total > RING as u64 * 10);

    let pump = probe.pump();
    assert_eq!(pump.trace_next_seq(), total);
    assert_eq!(pump.trace().len(), RING);
    assert_eq!(pump.trace_dropped(), total - RING as u64);
    assert_eq!(pump.trace_first_seq(), total - RING as u64);
    assert!(seq_continuous(pump.trace()));
    assert_eq!(pump.trace().first().unwrap().seq, total - RING as u64);
    assert_eq!(pump.trace().last().unwrap().seq, total - 1);
    // The retained events are exactly the last RING events of the full run.
    assert_eq!(pump.trace(), &all[all.len() - RING..]);

    // `trace_since` cuts by seq; older seq values return everything retained.
    let first = pump.trace_first_seq();
    assert_eq!(pump.trace_since(0), pump.trace());
    assert_eq!(pump.trace_since(first), pump.trace());
    assert_eq!(pump.trace_since(first + 3), &pump.trace()[3..]);
    assert!(pump.trace_since(total).is_empty());
    assert!(pump.trace_since(total + 5).is_empty());
}

#[test]
fn the_trace_mode_is_chosen_before_the_first_event_only() {
    let mut probe = periodic(PumpTraceMode::Bounded, None);
    probe.run(STEPS_TO_HLT, || false);
    let recorded = probe.pump().trace_next_seq();
    assert!(recorded > 0);
    let error = probe
        .pump_mut()
        .set_trace_mode(PumpTraceMode::Ring(8))
        .unwrap_err();
    assert_eq!(error, PumpTraceModeError::Recorded { recorded });
    assert_eq!(probe.pump().trace_mode(), PumpTraceMode::Bounded);
}

#[test]
fn ring_zero_is_rejected_and_leaves_the_trace_mode_unchanged() {
    let mut probe = periodic(PumpTraceMode::Bounded, None);
    let error = probe
        .pump_mut()
        .set_trace_mode(PumpTraceMode::Ring(0))
        .unwrap_err();
    assert_eq!(error, PumpTraceModeError::ZeroCapacity);
    assert_eq!(probe.pump().trace_mode(), PumpTraceMode::Bounded);
    assert!(
        probe
            .pump_mut()
            .set_trace_mode(PumpTraceMode::Ring(1))
            .is_ok()
    );
}

#[test]
fn ring_with_the_limit_left_in_place_ends_with_trace_limit() {
    let mut probe = periodic(
        PumpTraceMode::Ring(RING),
        Some(PumpInjection::RingKeepsLimit),
    );
    let result = probe.run(LONG_BUDGET, || false);
    assert_eq!(result.exit, PumpExit::Rejected(PumpRejection::TraceLimit));
    assert_eq!(probe.pump().trace().len(), RING);
    assert_eq!(probe.pump().trace_dropped(), 0);
}

#[test]
fn ring_that_resets_seq_when_it_drops_fails_the_continuity_check() {
    let mut probe = periodic(
        PumpTraceMode::Ring(RING),
        Some(PumpInjection::RingResetsSeq),
    );
    let result = probe.run(LONG_BUDGET, || false);
    assert_eq!(result.exit, PumpExit::Budget);
    assert!(probe.pump().trace_dropped() > 0);
    assert!(!seq_continuous(probe.pump().trace()));
}
