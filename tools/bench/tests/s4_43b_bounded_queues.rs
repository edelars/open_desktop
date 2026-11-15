//! S4-43b: bounded event queues and trace of `od-machine` under churn and reset.
//!
//! Scope: the queues that exist in the machine layer and can be driven through
//! public API without Linux: `EventQueue`, `UartRxEventPump` (its payload list),
//! the IRQ event queue and the ring/bounded trace of `InterruptPump`, and the
//! display event queue (`DisplayHandle` plus MMIO). No Linux boot, no real
//! timer/IRQ hardware model beyond the diagnostic pump, no JIT cache, no SMP, no
//! long run. A deterministic, bounded stress (explicit constants below), not
//! evidence of long-term reliability.
//!
//! Every queue answer (accepted/refused, order, length, delivered bytes, IRQ
//! entries) is compared with a plain-Rust model written here from the queue's
//! documented rules, not from queue output. The whole workload runs on a worker
//! thread under a watchdog.
//!
//! `S4_43B_JSON_OUT=<path>` additionally writes the summary JSON.

use od_machine::{
    AccessWidth, DISPLAY_EVENT_CAPACITY, DISPLAY_FORMAT_BGRX8888, DISPLAY_FRAMEBUFFER_BASE,
    DISPLAY_FRAMEBUFFER_BYTES, DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES, DisplayDevice,
    DisplayHandle, DisplayRequestError, EventQueue, EventQueueError, GuestPhysicalAddress,
    GuestPhysicalRange, INTERRUPT_PUMP_QUEUE_CAPACITY, INTERRUPT_PUMP_TRACE_LIMIT, InterruptPump,
    MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding,
    ModeRequestOutcome, PumpExit, PumpQueueError, PumpRejection, PumpTraceMode, PumpTraceModeError,
    RamLimits, STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA, STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES,
    STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR as VECTOR, Uart16550PollingTx, UartRxDispatchExit,
    UartRxEventPump, UartRxPumpError, load_stage4_interrupt_queue_probe,
};
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// ---- explicit limits (constants, not tunables) ------------------------------
/// Measured iterations per queue (the 2000-cycle run).
const MAIN_ITERATIONS: usize = 2000;
/// Baseline run used to compare retained memory with the long run.
const BASELINE_ITERATIONS: usize = 200;
/// Warm-up before measurement (allocator arenas, Vec growth, thread startup).
const WARMUP_ITERATIONS: usize = 100;
/// Operations per iteration in the standalone `EventQueue` scenario.
const OPS_PER_ITERATION: usize = 4;
/// Every N-th iteration is a reset episode (fill, overflow attempt, reset).
const EVENT_QUEUE_RESET_EVERY: usize = 50;
const UART_RESET_EVERY: usize = 25;
const PUMP_RESET_EVERY: usize = 25;
const DISPLAY_DISABLE_EVERY: usize = 50;
const DISPLAY_RESET_EVERY: usize = 100;
/// Declared capacity of the standalone queue and of the UART RX pump.
const EQ_CAPACITY: usize = 8;
const UART_CAPACITY: usize = 8;
/// `Ring(n)` capacity used for the long pump churn.
const RING_CAPACITY: usize = 64;
/// Events scheduled per pump iteration (more than the queue capacity on purpose).
const PUMP_MAX_EVENTS_PER_ITERATION: u64 = 20;
/// Whole-run watchdog.
const WATCHDOG: Duration = Duration::from_secs(120);
/// One scenario run must not take longer than this.
const SCENARIO_BUDGET: Duration = Duration::from_secs(30);
/// Process RSS growth limit over the main runs, bytes (5 MiB).
const RSS_GROWTH_LIMIT: u64 = 5 * 1024 * 1024;
/// Retained live heap difference allowed between the 200-iteration baseline and
/// the 2000-iteration run (queues alive in both), bytes.
const RETAINED_HEAP_GROWTH_LIMIT: i64 = 64 * 1024;
/// Live heap left after the queues are dropped, relative to before creation.
const LEAK_AFTER_DROP_LIMIT: i64 = 16 * 1024;
/// Peak live heap above the pre-scenario level during any scenario.
const PEAK_HEAP_LIMIT: u64 = 4 * 1024 * 1024;
/// Per-run step budget of the pump (a hang would end as `Budget`, not IdleHalt).
const PUMP_RUN_BUDGET: u64 = 100_000;

// ---- host allocation counter --------------------------------------------------
struct CountingAllocator;
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards to `System` unchanged; the atomics only count.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size() as u64);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_alloc(layout.size() as u64);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(pointer, layout) };
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
            record_alloc(new_size as u64);
        }
        new_pointer
    }
}

fn record_alloc(size: u64) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn live_bytes() -> u64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}

fn process_rss_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let kib: u64 = String::from_utf8(output.stdout).ok()?.trim().parse().ok()?;
    Some(kib * 1024)
}

/// Heap window of one scenario run: starts before the queues are created.
struct HeapWindow {
    before: u64,
    allocations_before: u64,
}

impl HeapWindow {
    fn begin() -> Self {
        let before = live_bytes();
        PEAK_BYTES.store(before, Ordering::Relaxed);
        Self {
            before,
            allocations_before: ALLOCATIONS.load(Ordering::Relaxed),
        }
    }
    fn retained(&self) -> i64 {
        live_bytes() as i64 - self.before as i64
    }
    fn finish(self, counters: Counters, retained_alive: i64, started: Instant) -> Outcome {
        let elapsed = started.elapsed();
        assert!(
            elapsed < SCENARIO_BUDGET,
            "scenario exceeded its time budget: {elapsed:?}"
        );
        Outcome {
            counters,
            retained_alive_bytes: retained_alive,
            leaked_after_drop_bytes: self.retained(),
            peak_above_before_bytes: PEAK_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(self.before),
            allocations: ALLOCATIONS.load(Ordering::Relaxed) - self.allocations_before,
            elapsed_ms: elapsed.as_millis(),
        }
    }
}

// ---- deterministic helpers ----------------------------------------------------
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

#[derive(Serialize, Default, Clone)]
struct Counters {
    iterations: u64,
    operations: u64,
    oracle_comparisons: u64,
    max_len_seen: u64,
    /// Operations that had to be refused because the queue was full.
    overflow_attempts: u64,
    /// Of those, refused with the documented explicit error (never silently).
    overflow_refused_explicitly: u64,
    resets: u64,
    empty_after_reset_checks: u64,
    stale_token_refusals: u64,
    extra: BTreeMap<&'static str, u64>,
}

impl Counters {
    fn bump(&mut self, name: &'static str) {
        *self.extra.entry(name).or_default() += 1;
    }
    fn add(&mut self, name: &'static str, value: u64) {
        *self.extra.entry(name).or_default() += value;
    }
    fn max_extra(&mut self, name: &'static str, value: u64) {
        let slot = self.extra.entry(name).or_default();
        *slot = (*slot).max(value);
    }
    fn saw_len(&mut self, len: usize, capacity: usize) {
        assert!(
            len <= capacity,
            "queue length {len} exceeds declared capacity {capacity}"
        );
        self.max_len_seen = self.max_len_seen.max(len as u64);
    }
}

#[derive(Serialize)]
struct Outcome {
    counters: Counters,
    retained_alive_bytes: i64,
    leaked_after_drop_bytes: i64,
    peak_above_before_bytes: u64,
    allocations: u64,
    elapsed_ms: u128,
}

// ---- scenario 1: EventQueue vs a plain model ----------------------------------
type ModelEntry = (u64, u64, u64); // key, deadline, sequence

fn model_min(model: &[ModelEntry], now: u64) -> Option<ModelEntry> {
    model
        .iter()
        .filter(|entry| entry.1 <= now)
        .min_by_key(|entry| (entry.1, entry.2))
        .copied()
}

fn scenario_event_queue(iterations: usize) -> Outcome {
    let started = Instant::now();
    let window = HeapWindow::begin();
    let mut c = Counters::default();
    let mut rng = Rng(0x5334_3433_6200_0001);
    let retained_alive;
    {
        let mut queue = EventQueue::new(EQ_CAPACITY);
        assert_eq!(queue.capacity(), EQ_CAPACITY);
        let mut generation = queue.generation();
        let mut model: Vec<ModelEntry> = Vec::new();
        let mut next_sequence = 0_u64;
        let mut now = 0_u64;
        for i in 0..iterations {
            c.iterations += 1;
            for _ in 0..OPS_PER_ITERATION {
                c.operations += 1;
                match rng.next() % 5 {
                    0..=1 => {
                        let key = rng.next() % 12;
                        let deadline = now + rng.next() % 50;
                        let expected = if model.iter().any(|e| e.0 == key) {
                            Err(EventQueueError::DuplicateKey)
                        } else if model.len() == EQ_CAPACITY {
                            Err(EventQueueError::Full)
                        } else {
                            Ok(())
                        };
                        let actual = queue.schedule(generation, key, deadline);
                        assert_eq!(actual, expected, "schedule key {key}");
                        c.oracle_comparisons += 1;
                        if expected == Err(EventQueueError::Full) {
                            c.overflow_attempts += 1;
                            if actual == Err(EventQueueError::Full) {
                                c.overflow_refused_explicitly += 1;
                            }
                        }
                        if expected.is_ok() {
                            model.push((key, deadline, next_sequence));
                            next_sequence += 1;
                        }
                    }
                    2 => {
                        now += rng.next() % 20;
                        let expected = model_min(&model, now);
                        let actual = queue.pop_due(generation, now).unwrap();
                        c.oracle_comparisons += 1;
                        match (expected, actual) {
                            (None, None) => {}
                            (Some(e), Some(a)) => {
                                assert_eq!((a.key, a.deadline_ns), (e.0, e.1), "pop order");
                                assert_eq!(a.generation, generation);
                                model.retain(|entry| entry.0 != e.0);
                            }
                            other => panic!("pop_due: model and queue disagree: {other:?}"),
                        }
                    }
                    3 => {
                        let key = rng.next() % 12;
                        let expected = if model.iter().any(|e| e.0 == key) {
                            Ok(())
                        } else {
                            Err(EventQueueError::NotFound)
                        };
                        assert_eq!(queue.cancel(generation, key), expected, "cancel {key}");
                        c.oracle_comparisons += 1;
                        model.retain(|entry| entry.0 != key);
                    }
                    _ => {
                        let key = rng.next() % 12;
                        let deadline = now + rng.next() % 50;
                        let expected = if model.iter().any(|e| e.0 == key) {
                            Ok(())
                        } else {
                            Err(EventQueueError::NotFound)
                        };
                        assert_eq!(queue.reschedule(generation, key, deadline), expected);
                        c.oracle_comparisons += 1;
                        if expected.is_ok() {
                            for entry in model.iter_mut().filter(|e| e.0 == key) {
                                entry.1 = deadline;
                                entry.2 = next_sequence;
                            }
                            next_sequence += 1;
                        }
                    }
                }
                assert_eq!(queue.len(), model.len(), "length vs model");
                c.saw_len(queue.len(), EQ_CAPACITY);
                assert_eq!(
                    queue.next_deadline(),
                    model.iter().map(|e| e.1).min(),
                    "earliest deadline"
                );
            }
            if i % EVENT_QUEUE_RESET_EVERY == EVENT_QUEUE_RESET_EVERY - 1 {
                // Fill to the declared capacity, then one more: explicit Full.
                for key in 100..100 + EQ_CAPACITY as u64 + 1 {
                    if model.iter().any(|e| e.0 == key) {
                        continue;
                    }
                    let full = model.len() == EQ_CAPACITY;
                    let actual = queue.schedule(generation, key, now + 1000);
                    if full {
                        c.overflow_attempts += 1;
                        assert_eq!(actual, Err(EventQueueError::Full), "overflow must refuse");
                        c.overflow_refused_explicitly += 1;
                    } else {
                        assert_eq!(actual, Ok(()));
                        model.push((key, now + 1000, next_sequence));
                        next_sequence += 1;
                    }
                }
                assert_eq!(queue.len(), EQ_CAPACITY, "queue must be exactly full");
                c.saw_len(queue.len(), EQ_CAPACITY);
                let old = generation;
                generation = queue.reset(old).unwrap();
                assert_ne!(generation, old);
                model.clear();
                next_sequence = 0;
                c.resets += 1;
                assert!(queue.is_empty() && queue.len() == 0, "queue after reset");
                assert_eq!(queue.next_deadline(), None);
                assert_eq!(queue.capacity(), EQ_CAPACITY, "capacity survives reset");
                c.empty_after_reset_checks += 1;
                for (name, result) in [
                    ("schedule", queue.schedule(old, 1, 1).err()),
                    ("cancel", queue.cancel(old, 1).err()),
                    ("pop_due", queue.pop_due(old, u64::MAX).err()),
                    ("reschedule", queue.reschedule(old, 1, 2).err()),
                    ("reset", queue.reset(old).err()),
                ] {
                    assert_eq!(
                        result,
                        Some(EventQueueError::StaleGeneration),
                        "{name} with a pre-reset token"
                    );
                    c.stale_token_refusals += 1;
                }
                assert!(queue.is_empty(), "stale calls must not change the queue");
                // Baseline behaviour is restored: first event after reset is
                // ordered exactly as on a new queue.
                queue.schedule(generation, 1, 5).unwrap();
                model.push((1, 5, next_sequence));
                next_sequence += 1;
            }
        }
        retained_alive = window.retained();
    }
    window.finish(c, retained_alive, started)
}

// ---- scenario 2: UartRxEventPump (event queue plus payload list) -------------
fn scenario_uart_rx_pump(iterations: usize) -> Outcome {
    let started = Instant::now();
    let window = HeapWindow::begin();
    let mut c = Counters::default();
    let mut rng = Rng(0x5334_3433_6200_0002);
    let retained_alive;
    {
        let (_device, uart) = Uart16550PollingTx::new_with_handle();
        let mut pump = UartRxEventPump::new(UART_CAPACITY, uart.clone());
        assert_eq!(pump.capacity(), UART_CAPACITY);
        let mut generation = pump.generation();
        // key, deadline, sequence, value
        let mut model: Vec<(u64, u64, u64, u8)> = Vec::new();
        let mut sequence = 0_u64;
        for i in 0..iterations {
            c.iterations += 1;
            let events = rng.next() % 12;
            for j in 0..events {
                let key = (i as u64) * 16 + j;
                let deadline = 1 + rng.next() % 100;
                let value = rng.next() as u8;
                c.operations += 1;
                let full = model.len() == UART_CAPACITY;
                let actual = pump.schedule(generation, key, deadline, value);
                if full {
                    c.overflow_attempts += 1;
                    assert_eq!(actual, Err(EventQueueError::Full), "overflow must refuse");
                    c.overflow_refused_explicitly += 1;
                } else {
                    assert_eq!(actual, Ok(()));
                    model.push((key, deadline, sequence, value));
                    sequence += 1;
                }
                c.oracle_comparisons += 1;
                assert_eq!(pump.len(), model.len());
                c.saw_len(pump.len(), UART_CAPACITY);
            }
            if !model.is_empty() && rng.next() % 4 == 0 {
                // Cancel one accepted key; payload list must shrink with it.
                let (key, ..) = model[(rng.next() as usize) % model.len()];
                assert_eq!(pump.cancel(generation, key), Ok(()));
                model.retain(|entry| entry.0 != key);
                assert_eq!(
                    pump.cancel(generation, key),
                    Err(EventQueueError::NotFound),
                    "second cancel of the same key"
                );
                c.oracle_comparisons += 2;
                assert_eq!(pump.len(), model.len());
            }
            // Non-delivering exits leave the queue untouched.
            if !model.is_empty() {
                let before = pump.len();
                let stopped = pump.dispatch_due(generation, 1000, 100, || true).unwrap();
                assert_eq!(
                    (stopped.delivered, stopped.exit),
                    (0, UartRxDispatchExit::Stopped)
                );
                let budget = pump.dispatch_due(generation, 1000, 0, || false).unwrap();
                assert_eq!(
                    (budget.delivered, budget.exit),
                    (0, UartRxDispatchExit::BudgetExhausted)
                );
                let early = pump.dispatch_due(generation, 0, 100, || false).unwrap();
                assert_eq!(
                    (early.delivered, early.exit),
                    (0, UartRxDispatchExit::DeadlinePending)
                );
                assert_eq!(pump.len(), before, "STOP/budget/early must not drop events");
                assert_eq!(uart.received_byte(), None);
                c.bump("non_delivering_exits_checked");
            }
            if i % UART_RESET_EVERY == UART_RESET_EVERY - 1 {
                // Reset with events pending: they must not be delivered later.
                let pending = pump.len();
                let old = generation;
                generation = pump.reset(old).unwrap();
                c.add("events_dropped_by_reset", pending as u64);
                model.clear();
                sequence = 0;
                c.resets += 1;
                assert!(pump.is_empty() && pump.next_deadline().is_none());
                c.empty_after_reset_checks += 1;
                assert_eq!(
                    pump.dispatch_due(old, 1000, 100, || false).err(),
                    Some(UartRxPumpError::Queue(EventQueueError::StaleGeneration))
                );
                assert_eq!(
                    pump.schedule(old, 1, 1, 0xAA),
                    Err(EventQueueError::StaleGeneration)
                );
                c.stale_token_refusals += 2;
                let report = pump.dispatch_due(generation, 1000, 100, || false).unwrap();
                assert_eq!(
                    (report.delivered, report.exit),
                    (0, UartRxDispatchExit::NoEvent)
                );
                assert_eq!(uart.received_byte(), None, "reset events must not arrive");
                // Payload list was cleared: the same key now carries the new byte.
                pump.schedule(generation, 1, 1, 0x5A).unwrap();
                model.push((1, 1, sequence, 0x5A));
                sequence += 1;
            }
            // Drain everything: one byte per dispatch (RX register holds one).
            model.sort_by_key(|entry| (entry.1, entry.2));
            let mut expected = model.iter().copied();
            loop {
                let report = pump.dispatch_due(generation, 1000, 100, || false).unwrap();
                match expected.next() {
                    None => {
                        assert_eq!(
                            (report.delivered, report.exit),
                            (0, UartRxDispatchExit::NoEvent)
                        );
                        break;
                    }
                    Some(entry) => {
                        assert_eq!(report.delivered, 1, "one byte per dispatch");
                        assert_eq!(uart.received_byte(), Some(entry.3), "delivery order");
                        c.oracle_comparisons += 1;
                        c.bump("bytes_delivered");
                        uart.reset();
                        if report.exit == UartRxDispatchExit::NoEvent {
                            assert!(expected.next().is_none(), "queue ended early");
                            break;
                        }
                        assert_eq!(report.exit, UartRxDispatchExit::ReceiverBusy);
                    }
                }
            }
            model.clear();
            assert!(pump.is_empty(), "drained queue must be empty");
        }
        retained_alive = window.retained();
    }
    window.finish(c, retained_alive, started)
}

// ---- scenario 3: InterruptPump IRQ queue and Ring trace ----------------------
const BASE: usize = 0x1000;
const COUNTER: u64 = STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA;

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
/// Counter handler with EOI, then IRETQ.
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

/// SVR enable, `STI; loop: HLT; JMP loop`; vector 0x62 handler counts in RAM.
fn build_queue_image() -> Vec<u8> {
    let entry = [
        mov_edi(),
        write_reg(0xf0, 0x1ff),
        vec![0xfb, 0xf4, 0xeb, 0xfd],
    ]
    .concat();
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
        0x3000 + usize::from(VECTOR) * 16,
        &gate(handler_gpa),
    );
    put(&mut image, 0x4000, &entry);
    put(&mut image, handler_gpa as usize, &handler());
    image
}

fn load_pump(image: &[u8]) -> InterruptPump {
    let mut pump = load_stage4_interrupt_queue_probe(image).unwrap();
    // Prologue, STI, HLT: halted at virtual time 0 with no deadline.
    let result = pump.run(10_000, || false);
    assert_eq!(result.exit, PumpExit::IdleHalt);
    assert!(pump.is_waiting());
    pump
}

fn pump_counter(pump: &mut InterruptPump) -> u64 {
    pump.peek_ram_qword(COUNTER).unwrap() & 0xffff_ffff
}

/// Trace invariants of every mode: length bound, no silent loss, contiguous seq.
fn check_trace(pump: &InterruptPump, capacity: usize, c: &mut Counters) {
    let trace = pump.trace();
    assert!(
        trace.len() <= capacity,
        "trace length {} exceeds capacity {capacity}",
        trace.len()
    );
    assert_eq!(
        pump.trace_dropped() + trace.len() as u64,
        pump.trace_next_seq(),
        "every recorded event is retained or counted as dropped"
    );
    assert_eq!(pump.trace_first_seq(), pump.trace_dropped());
    for (offset, event) in trace.iter().enumerate() {
        assert_eq!(event.seq, pump.trace_first_seq() + offset as u64, "seq gap");
    }
    c.max_extra("trace_max_len", trace.len() as u64);
}

fn scenario_interrupt_pump_ring(iterations: usize) -> Outcome {
    let started = Instant::now();
    let window = HeapWindow::begin();
    let mut c = Counters::default();
    let mut rng = Rng(0x5334_3433_6200_0003);
    let retained_alive;
    {
        let image = build_queue_image();
        let mut pump = load_stage4_interrupt_queue_probe(&image).unwrap();
        // Ring mode must be chosen before the first event is recorded.
        pump.set_trace_mode(PumpTraceMode::Ring(RING_CAPACITY))
            .unwrap();
        assert_eq!(pump.run(10_000, || false).exit, PumpExit::IdleHalt);
        assert!(pump.is_waiting());
        let mut generation = pump.irq_generation();
        let mut next_key = 0_u64;
        let mut counter_model = 0_u64;
        let mut coalesced_model = 0_u64;
        for i in 0..iterations {
            c.iterations += 1;
            let now = pump.virtual_time_ns().unwrap();
            let requested = rng.next() % (PUMP_MAX_EVENTS_PER_ITERATION + 1);
            let mut accepted: Vec<(u64, u64)> = Vec::new(); // key, due
            for _ in 0..requested {
                let key = next_key;
                next_key += 1;
                let due = now + (rng.next() % 6) * 1000;
                c.operations += 1;
                let full = accepted.len() == INTERRUPT_PUMP_QUEUE_CAPACITY;
                let actual = pump.schedule_irq(generation, key, VECTOR, due);
                if full {
                    c.overflow_attempts += 1;
                    assert_eq!(
                        actual,
                        Err(PumpQueueError::Queue(EventQueueError::Full)),
                        "IRQ queue overflow must refuse explicitly"
                    );
                    c.overflow_refused_explicitly += 1;
                } else {
                    assert_eq!(actual, Ok(()));
                    accepted.push((key, due));
                }
                c.oracle_comparisons += 1;
                assert_eq!(pump.irq_queue_len(), accepted.len());
                c.saw_len(pump.irq_queue_len(), INTERRUPT_PUMP_QUEUE_CAPACITY);
            }
            // Invalid keys and vectors are refused, not queued.
            assert_eq!(
                pump.cancel_irq(generation, u64::MAX),
                Err(PumpQueueError::Queue(EventQueueError::NotFound))
            );
            assert_eq!(
                pump.schedule_irq(generation, u64::MAX - 1, 3, now),
                Err(PumpQueueError::InvalidVector(3))
            );
            assert_eq!(pump.irq_queue_len(), accepted.len());
            if !accepted.is_empty() && rng.next() % 4 == 0 {
                let index = (rng.next() as usize) % accepted.len();
                let (key, _) = accepted.remove(index);
                assert_eq!(pump.cancel_irq(generation, key), Ok(()));
                c.bump("cancelled");
                assert_eq!(pump.irq_queue_len(), accepted.len());
            }

            let counter_before = pump_counter(&mut pump);
            let time_before = pump.virtual_time_ns().unwrap();
            if i % PUMP_RESET_EVERY == PUMP_RESET_EVERY - 1 {
                // Reset with events pending: none of them may be delivered.
                let old = generation;
                c.add("irq_events_dropped_by_reset", pump.irq_queue_len() as u64);
                generation = pump.reset_irq_queue().unwrap();
                assert_ne!(generation, old);
                c.resets += 1;
                assert_eq!(pump.irq_queue_len(), 0);
                assert_eq!(pump.irq_queue_next_deadline(), None);
                c.empty_after_reset_checks += 1;
                assert_eq!(
                    pump.schedule_irq(old, u64::MAX - 2, VECTOR, now),
                    Err(PumpQueueError::Queue(EventQueueError::StaleGeneration))
                );
                assert_eq!(
                    pump.cancel_irq(old, 0),
                    Err(PumpQueueError::Queue(EventQueueError::StaleGeneration))
                );
                c.stale_token_refusals += 2;
                accepted.clear();
            }

            let result = pump.run(PUMP_RUN_BUDGET, || false);
            assert_eq!(
                result.exit,
                PumpExit::IdleHalt,
                "run must idle, not hang or be rejected"
            );
            assert!(result.quanta < PUMP_RUN_BUDGET);
            assert_eq!(pump.irq_queue_len(), 0, "all accepted events consumed");
            assert_eq!(pump.irq_queue_next_deadline(), None);

            // Model: events with one due time share one IRR edge (one entry).
            let distinct: BTreeSet<u64> = accepted.iter().map(|e| e.1).collect();
            counter_model += distinct.len() as u64;
            coalesced_model += (accepted.len() - distinct.len()) as u64;
            assert_eq!(
                pump_counter(&mut pump),
                counter_before + distinct.len() as u64,
                "handler entries vs distinct due times"
            );
            assert_eq!(pump_counter(&mut pump), counter_model);
            assert_eq!(pump.coalesced_irqs(), coalesced_model, "coalesced counter");
            let expected_time = accepted
                .iter()
                .map(|e| e.1)
                .max()
                .unwrap_or(time_before)
                .max(time_before);
            assert_eq!(pump.virtual_time_ns().unwrap(), expected_time);
            c.oracle_comparisons += 4;
            c.add("interrupts_delivered", distinct.len() as u64);
            c.add("irq_events_accepted", accepted.len() as u64);
            check_trace(&pump, RING_CAPACITY, &mut c);
        }
        assert!(
            pump.trace_dropped() > 0,
            "ring must have wrapped in this run"
        );
        c.add("trace_events_recorded", pump.trace_next_seq());
        c.add("trace_events_dropped", pump.trace_dropped());
        assert_eq!(pump.trace_mode(), PumpTraceMode::Ring(RING_CAPACITY));

        // STOP with events pending: immediate Stopped, no events lost, final.
        for key in 0..3 {
            let due = pump.virtual_time_ns().unwrap() + 1000;
            pump.schedule_irq(generation, 1_000_000 + key, VECTOR, due)
                .unwrap();
        }
        let stopped = pump.run(PUMP_RUN_BUDGET, || true);
        assert_eq!(
            (stopped.exit.clone(), stopped.quanta),
            (PumpExit::Stopped, 0)
        );
        assert_eq!(
            pump.irq_queue_len(),
            3,
            "STOP must not consume queued events"
        );
        let again = pump.run(PUMP_RUN_BUDGET, || false);
        assert_eq!(again.exit, PumpExit::Stopped, "STOP is final, never a hang");
        generation = pump.reset_irq_queue().unwrap();
        assert_eq!(pump.irq_queue_len(), 0);
        assert_eq!(
            pump.schedule_irq(generation, 1, VECTOR, 0),
            Ok(()),
            "queue still usable after STOP and reset"
        );
        c.bump("stop_checks");
        retained_alive = window.retained();
    }
    window.finish(c, retained_alive, started)
}

/// Bounded trace (the default mode): the limit ends the run with an explicit
/// rejection; it never grows past 512 and never drops silently.
#[derive(Serialize)]
struct BoundedTraceCheck {
    limit: usize,
    iterations_to_limit: u64,
    trace_len_at_limit: usize,
    trace_dropped: u64,
    trace_next_seq: u64,
    exit: String,
    second_run_exit: String,
    ring_zero_rejected: bool,
    mode_change_after_record_rejected: bool,
    irq_queue_len_at_rejection: usize,
    irq_queue_len_after_reset: usize,
}

fn bounded_trace_check() -> BoundedTraceCheck {
    let image = build_queue_image();
    let mut pump = load_pump(&image);
    assert_eq!(pump.trace_mode(), PumpTraceMode::Bounded);
    assert_eq!(INTERRUPT_PUMP_TRACE_LIMIT, 512);
    let generation = pump.irq_generation();
    let mut key = 0_u64;
    let mut rounds = 0_u64;
    let exit = loop {
        rounds += 1;
        assert!(rounds < 200, "trace limit never reached: unbounded growth");
        let now = pump.virtual_time_ns().unwrap();
        for j in 0..INTERRUPT_PUMP_QUEUE_CAPACITY as u64 {
            pump.schedule_irq(generation, key, VECTOR, now + 1000 * (j + 1))
                .unwrap();
            key += 1;
        }
        let result = pump.run(PUMP_RUN_BUDGET, || false);
        if result.exit != PumpExit::IdleHalt {
            break result.exit;
        }
        assert!(
            pump.trace().len() <= INTERRUPT_PUMP_TRACE_LIMIT,
            "trace over the bound"
        );
    };
    assert_eq!(
        exit,
        PumpExit::Rejected(PumpRejection::TraceLimit),
        "overflow of the bounded trace must be an explicit rejection"
    );
    assert_eq!(pump.trace().len(), INTERRUPT_PUMP_TRACE_LIMIT);
    assert_eq!(pump.trace_dropped(), 0, "Bounded mode never drops");
    assert_eq!(pump.trace_next_seq(), INTERRUPT_PUMP_TRACE_LIMIT as u64);
    let queue_len = pump.irq_queue_len();
    let second = pump.run(PUMP_RUN_BUDGET, || false);
    assert_eq!(second.exit, exit, "rejection is final, not a hang");
    assert_eq!(pump.trace().len(), INTERRUPT_PUMP_TRACE_LIMIT);
    pump.reset_irq_queue().unwrap();
    assert_eq!(pump.irq_queue_len(), 0);
    let ring_zero = pump.set_trace_mode(PumpTraceMode::Ring(0));
    assert_eq!(ring_zero, Err(PumpTraceModeError::ZeroCapacity));
    let late = pump.set_trace_mode(PumpTraceMode::Ring(8));
    assert!(matches!(late, Err(PumpTraceModeError::Recorded { .. })));
    BoundedTraceCheck {
        limit: INTERRUPT_PUMP_TRACE_LIMIT,
        iterations_to_limit: rounds,
        trace_len_at_limit: pump.trace().len(),
        trace_dropped: pump.trace_dropped(),
        trace_next_seq: pump.trace_next_seq(),
        exit: format!("{exit:?}"),
        second_run_exit: format!("{:?}", second.exit),
        ring_zero_rejected: ring_zero == Err(PumpTraceModeError::ZeroCapacity),
        mode_change_after_record_rejected: matches!(late, Err(PumpTraceModeError::Recorded { .. })),
        irq_queue_len_at_rejection: queue_len,
        irq_queue_len_after_reset: pump.irq_queue_len(),
    }
}

// ---- scenario 4: display event queue ------------------------------------------
fn range(start: u64, len: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
}

fn display_bus() -> (MachineBus, DisplayHandle) {
    const RAM_SIZE: u64 = 0x20_000;
    let (device, handle) = DisplayDevice::new_with_handle();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(range(0, RAM_SIZE), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(
                range(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
                MemoryRegionKind::Framebuffer,
            ),
            MemoryRegionSpec::new(
                range(DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
                MemoryRegionKind::Mmio,
            ),
        ],
    )
    .unwrap();
    let bus = MachineBus::new(
        map,
        RamLimits::new(RAM_SIZE),
        vec![Box::new(device)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    (bus, handle)
}

mod display_reg {
    pub const DRIVER_PROTOCOL: u64 = 0x104;
    pub const EVENT_PENDING: u64 = 0x140;
    pub const EVENT_CAPACITY: u64 = 0x144;
    pub const EVENT_POP: u64 = 0x148;
    pub const EVENT_SEQ_LO: u64 = 0x150;
}

fn rd(bus: &mut MachineBus, offset: u64) -> u32 {
    bus.read_physical(
        GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE + offset),
        AccessWidth::Dword,
    )
    .unwrap() as u32
}

fn wr(bus: &mut MachineBus, offset: u64, value: u32) {
    bus.write_physical(
        GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE + offset),
        AccessWidth::Dword,
        u64::from(value),
        |_| {},
    )
    .unwrap();
}

fn scenario_display_events(iterations: usize) -> Outcome {
    let started = Instant::now();
    let window = HeapWindow::begin();
    let mut c = Counters::default();
    let mut rng = Rng(0x5334_3433_6200_0004);
    let retained_alive;
    {
        let (mut bus, handle) = display_bus();
        let modes = [(640_u32, 480_u32), (800, 600), (1024, 768), (1280, 720)];
        assert_eq!(
            rd(&mut bus, display_reg::EVENT_CAPACITY) as usize,
            DISPLAY_EVENT_CAPACITY
        );
        wr(&mut bus, display_reg::DRIVER_PROTOCOL, 1);
        assert_eq!(handle.driver_protocol(), 1);
        let mut last_seq: Option<u64> = None;
        for i in 0..iterations {
            c.iterations += 1;
            c.operations += 3;
            let (width, height) = modes[(rng.next() as usize) % modes.len()];
            let seq = match handle.post_mode_request(width, height, DISPLAY_FORMAT_BGRX8888) {
                Ok(seq) => seq,
                Err(DisplayRequestError::QueueFull) => {
                    c.overflow_attempts += 1;
                    c.overflow_refused_explicitly += 1;
                    c.bump("queue_full_observed");
                    panic!("display queue full through the public API: unexpected");
                }
                Err(error) => panic!("post_mode_request: {error:?}"),
            };
            if let Some(last) = last_seq {
                assert_eq!(seq, last + 1, "event seq must advance by one");
            }
            last_seq = Some(seq);
            // An untaken MODE_REQUEST of screen 0 is merged: one slot, not a chain.
            let mut model_pending = 1_u32;
            let state = handle.mode_request_state().unwrap();
            assert_eq!(
                (state.seq, state.outcome, state.width, state.height),
                (seq, ModeRequestOutcome::Pending, width, height)
            );
            assert_eq!(
                handle.post_mode_request(0, 0, DISPLAY_FORMAT_BGRX8888),
                Err(DisplayRequestError::InvalidMode)
            );
            c.oracle_comparisons += 3;
            if rng.next() % 2 == 0 {
                wr(&mut bus, display_reg::EVENT_POP, 1);
                assert_eq!(
                    u64::from(rd(&mut bus, display_reg::EVENT_SEQ_LO)),
                    seq & 0xffff_ffff
                );
                model_pending = 0;
                c.bump("events_popped");
            }
            let pending = rd(&mut bus, display_reg::EVENT_PENDING);
            assert_eq!(pending, model_pending, "pending vs model");
            c.saw_len(pending as usize, DISPLAY_EVENT_CAPACITY);
            if i % DISPLAY_DISABLE_EVERY == DISPLAY_DISABLE_EVERY - 1 {
                // Protocol off clears the queue and refuses new posts.
                let had_pending = model_pending;
                wr(&mut bus, display_reg::DRIVER_PROTOCOL, 0);
                c.resets += 1;
                c.add("events_dropped_by_reset", u64::from(had_pending));
                assert_eq!(rd(&mut bus, display_reg::EVENT_PENDING), 0);
                c.empty_after_reset_checks += 1;
                assert_eq!(
                    handle.post_mode_request(640, 480, DISPLAY_FORMAT_BGRX8888),
                    Err(DisplayRequestError::NoDriver)
                );
                c.stale_token_refusals += 1;
                if had_pending == 1 {
                    assert_eq!(
                        handle.mode_request_state().unwrap().outcome,
                        ModeRequestOutcome::Cancelled,
                        "open request is cancelled, not left pending"
                    );
                }
                wr(&mut bus, display_reg::DRIVER_PROTOCOL, 1);
            }
            if i % DISPLAY_RESET_EVERY == DISPLAY_RESET_EVERY - 1 {
                let had_pending = model_pending;
                handle.reset();
                c.resets += 1;
                c.add("events_dropped_by_reset", u64::from(had_pending));
                assert_eq!(handle.driver_protocol(), 0);
                assert_eq!(rd(&mut bus, display_reg::EVENT_PENDING), 0);
                c.empty_after_reset_checks += 1;
                assert_eq!(
                    handle.post_mode_request(640, 480, DISPLAY_FORMAT_BGRX8888),
                    Err(DisplayRequestError::NoDriver)
                );
                wr(&mut bus, display_reg::DRIVER_PROTOCOL, 1);
                assert_eq!(handle.driver_protocol(), 1);
                // Sequence numbers after a host reset are re-read, not assumed.
                last_seq = None;
            }
        }
        retained_alive = window.retained();
    }
    window.finish(c, retained_alive, started)
}

// ---- runner, report, watchdog ------------------------------------------------
type Scenario = fn(usize) -> Outcome;

struct QueueSpec {
    name: &'static str,
    source: &'static str,
    capacity: usize,
    overflow: &'static str,
    run: Scenario,
    /// The public API can reach the declared overflow.
    overflow_reachable: bool,
    /// Extra host allocation the scenario owns by construction (framebuffer).
    fixed_heap_bytes: u64,
}

fn queue_specs() -> Vec<QueueSpec> {
    vec![
        QueueSpec {
            name: "EventQueue (standalone)",
            source: "crates/od-machine/src/event_queue.rs",
            capacity: EQ_CAPACITY,
            overflow: "error: EventQueueError::Full; queue unchanged",
            run: scenario_event_queue,
            overflow_reachable: true,
            fixed_heap_bytes: 0,
        },
        QueueSpec {
            name: "UartRxEventPump events + payloads",
            source: "crates/od-machine/src/uart_rx_events.rs",
            capacity: UART_CAPACITY,
            overflow: "error: EventQueueError::Full from schedule; payload list not extended",
            run: scenario_uart_rx_pump,
            overflow_reachable: true,
            fixed_heap_bytes: 0,
        },
        QueueSpec {
            name: "InterruptPump IRQ event queue + Ring trace",
            source: "crates/od-machine/src/interrupt_pump.rs (INTERRUPT_PUMP_QUEUE_CAPACITY)",
            capacity: INTERRUPT_PUMP_QUEUE_CAPACITY,
            overflow: "error: PumpQueueError::Queue(Full); due merge counted in coalesced_irqs",
            run: scenario_interrupt_pump_ring,
            overflow_reachable: true,
            fixed_heap_bytes: 0,
        },
        QueueSpec {
            name: "Display event queue",
            source: "crates/od-machine/src/display_device.rs (DISPLAY_EVENT_CAPACITY)",
            capacity: DISPLAY_EVENT_CAPACITY,
            overflow: "error: DisplayRequestError::QueueFull (not reachable from the public API: mode requests merge)",
            run: scenario_display_events,
            overflow_reachable: false,
            fixed_heap_bytes: DISPLAY_FRAMEBUFFER_BYTES,
        },
    ]
}

#[derive(Serialize)]
struct PhaseReport {
    phase: &'static str,
    iterations: usize,
    outcome: Outcome,
}

#[derive(Serialize)]
struct QueueReport {
    queue: &'static str,
    source: &'static str,
    declared_capacity: usize,
    overflow_behavior: &'static str,
    overflow_reachable_through_public_api: bool,
    fixed_heap_bytes: u64,
    phases: Vec<PhaseReport>,
    retained_heap_main_vs_baseline_bytes: i64,
}

#[derive(Serialize)]
struct UnexercisedQueue {
    queue: &'static str,
    source: &'static str,
    declared_capacity: &'static str,
    overflow_behavior: &'static str,
    why_not_exercised: &'static str,
}

enum Watchdog<T> {
    Done(T),
    Panicked(Box<dyn std::any::Any + Send>),
    TimedOut,
}

fn run_with_watchdog<T: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Watchdog<T> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)));
    });
    match receiver.recv_timeout(timeout) {
        Ok(Ok(value)) => Watchdog::Done(value),
        Ok(Err(payload)) => Watchdog::Panicked(payload),
        Err(_) => Watchdog::TimedOut,
    }
}

#[derive(Serialize)]
struct Limits {
    main_iterations: usize,
    baseline_iterations: usize,
    warmup_iterations: usize,
    ops_per_iteration_event_queue: usize,
    eq_capacity: usize,
    uart_capacity: usize,
    ring_capacity: usize,
    pump_max_events_per_iteration: u64,
    watchdog_seconds: u64,
    scenario_budget_seconds: u64,
    rss_growth_limit_bytes: u64,
    retained_heap_growth_limit_bytes: i64,
    leak_after_drop_limit_bytes: i64,
    peak_heap_limit_bytes: u64,
    pump_run_budget: u64,
}

#[derive(Serialize)]
struct Summary {
    schema: &'static str,
    scope: &'static str,
    not_covered: [&'static str; 8],
    os: &'static str,
    arch: &'static str,
    limits: Limits,
    queues: Vec<QueueReport>,
    bounded_trace_check: BoundedTraceCheck,
    unexercised_queues: Vec<UnexercisedQueue>,
    rss_after_warmup_bytes: Option<u64>,
    rss_after_main_bytes: Option<u64>,
    rss_growth_bytes: Option<i64>,
    watchdog_self_test_timed_out: bool,
    total_elapsed_ms: u128,
}

fn workload() -> Summary {
    let started = Instant::now();
    let specs = queue_specs();
    let mut reports: Vec<QueueReport> = specs
        .iter()
        .map(|spec| QueueReport {
            queue: spec.name,
            source: spec.source,
            declared_capacity: spec.capacity,
            overflow_behavior: spec.overflow,
            overflow_reachable_through_public_api: spec.overflow_reachable,
            fixed_heap_bytes: spec.fixed_heap_bytes,
            phases: Vec::new(),
            retained_heap_main_vs_baseline_bytes: 0,
        })
        .collect();
    let mut rss_after_warmup = None;
    for (phase, iterations) in [
        ("warmup", WARMUP_ITERATIONS),
        ("baseline", BASELINE_ITERATIONS),
        ("main", MAIN_ITERATIONS),
    ] {
        for (spec, report) in specs.iter().zip(reports.iter_mut()) {
            let outcome = (spec.run)(iterations);
            report.phases.push(PhaseReport {
                phase,
                iterations,
                outcome,
            });
        }
        if phase == "warmup" {
            rss_after_warmup = process_rss_bytes();
        }
    }
    let rss_after_main = process_rss_bytes();
    let bounded_trace_check = bounded_trace_check();
    for report in &mut reports {
        let baseline = report.phases[1].outcome.retained_alive_bytes;
        let main = report.phases[2].outcome.retained_alive_bytes;
        report.retained_heap_main_vs_baseline_bytes = main - baseline;
    }
    Summary {
        schema: "s4-43b-queues-v1",
        scope: "machine-layer event queues, pump trace and display event queue; bounded stress, not long-run reliability",
        not_covered: [
            "Linux boot",
            "IRQ and timer on a real kernel",
            "executable (JIT) cache and its limit",
            "SMP",
            "long-run reliability",
            "PIT IRQ-source queues (private, capacity 4)",
            "NVMe queues (guest-sized, bounded by MAX_QUEUE_ENTRIES)",
            "runner trace and journal outside od-machine",
        ],
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        limits: Limits {
            main_iterations: MAIN_ITERATIONS,
            baseline_iterations: BASELINE_ITERATIONS,
            warmup_iterations: WARMUP_ITERATIONS,
            ops_per_iteration_event_queue: OPS_PER_ITERATION,
            eq_capacity: EQ_CAPACITY,
            uart_capacity: UART_CAPACITY,
            ring_capacity: RING_CAPACITY,
            pump_max_events_per_iteration: PUMP_MAX_EVENTS_PER_ITERATION,
            watchdog_seconds: WATCHDOG.as_secs(),
            scenario_budget_seconds: SCENARIO_BUDGET.as_secs(),
            rss_growth_limit_bytes: RSS_GROWTH_LIMIT,
            retained_heap_growth_limit_bytes: RETAINED_HEAP_GROWTH_LIMIT,
            leak_after_drop_limit_bytes: LEAK_AFTER_DROP_LIMIT,
            peak_heap_limit_bytes: PEAK_HEAP_LIMIT,
            pump_run_budget: PUMP_RUN_BUDGET,
        },
        queues: reports,
        bounded_trace_check,
        unexercised_queues: vec![
            UnexercisedQueue {
                queue: "PIT/PIC and PIT/IOAPIC IRQ-source EventQueue",
                source: "crates/od-machine/src/pit_pic_irq_source.rs, pit_ioapic_irq_source.rs",
                declared_capacity: "4 (private QUEUE_CAPACITY)",
                overflow_behavior: "schedule error counted in stats.queue_faults; source stops scheduling",
                why_not_exercised: "private constant and private owner; only reachable through guest PIT programming",
            },
            UnexercisedQueue {
                queue: "TranslationProbeCache entries",
                source: "crates/od-machine/src/translation_probe_cache.rs",
                declared_capacity: "single resident entry",
                overflow_behavior: "overwrite (cache replacement); no error",
                why_not_exercised: "covered by S4-43a",
            },
            UnexercisedQueue {
                queue: "NVMe submission/completion queues",
                source: "crates/od-machine/src/nvme/",
                declared_capacity: "2..=MAX_QUEUE_ENTRIES (1024), sized by the guest",
                overflow_behavior: "invalid size refused by admin command; ring indices wrap",
                why_not_exercised: "needs an NVMe driver workload; not an event queue",
            },
        ],
        rss_after_warmup_bytes: rss_after_warmup,
        rss_after_main_bytes: rss_after_main,
        rss_growth_bytes: match (rss_after_warmup, rss_after_main) {
            (Some(before), Some(after)) => Some(after as i64 - before as i64),
            _ => None,
        },
        watchdog_self_test_timed_out: false,
        total_elapsed_ms: started.elapsed().as_millis(),
    }
}

#[test]
fn bounded_queues_and_trace_under_churn() {
    // Watchdog self-test: a stuck closure must be reported, not waited for.
    let stuck = run_with_watchdog(Duration::from_millis(200), || {
        std::thread::sleep(Duration::from_secs(2));
    });
    assert!(matches!(stuck, Watchdog::TimedOut));
    let quick = run_with_watchdog(Duration::from_secs(5), || 7u32);
    assert!(matches!(quick, Watchdog::Done(7)));

    let mut summary = match run_with_watchdog(WATCHDOG, workload) {
        Watchdog::Done(summary) => summary,
        Watchdog::Panicked(payload) => std::panic::resume_unwind(payload),
        Watchdog::TimedOut => panic!("queue churn did not finish within {WATCHDOG:?}: hang"),
    };
    summary.watchdog_self_test_timed_out = true;

    for report in &summary.queues {
        let name = report.queue;
        let main = &report.phases[2];
        let c = &main.outcome.counters;
        assert_eq!(c.iterations as usize, MAIN_ITERATIONS, "{name}");
        assert!(
            c.max_len_seen as usize <= report.declared_capacity,
            "{name}"
        );
        assert!(c.oracle_comparisons >= MAIN_ITERATIONS as u64, "{name}");
        assert_eq!(c.overflow_refused_explicitly, c.overflow_attempts, "{name}");
        if report.overflow_reachable_through_public_api {
            assert!(
                c.overflow_attempts > 0,
                "{name}: overflow must actually be exercised"
            );
            assert_eq!(c.max_len_seen as usize, report.declared_capacity, "{name}");
        } else {
            assert_eq!(c.overflow_attempts, 0, "{name}");
        }
        assert!(c.resets > 0, "{name}");
        assert_eq!(c.empty_after_reset_checks, c.resets, "{name}");
        assert!(c.stale_token_refusals > 0, "{name}");
        for phase in &report.phases {
            let o = &phase.outcome;
            assert!(
                o.peak_above_before_bytes <= PEAK_HEAP_LIMIT + report.fixed_heap_bytes,
                "{name}/{}: peak heap {} bytes",
                phase.phase,
                o.peak_above_before_bytes
            );
            assert!(
                o.leaked_after_drop_bytes.abs() <= LEAK_AFTER_DROP_LIMIT,
                "{name}/{}: {} bytes left after drop",
                phase.phase,
                o.leaked_after_drop_bytes
            );
        }
        assert!(
            report.retained_heap_main_vs_baseline_bytes.abs() <= RETAINED_HEAP_GROWTH_LIMIT,
            "{name}: retained heap after 2000 iterations differs from the 200-iteration baseline by {}",
            report.retained_heap_main_vs_baseline_bytes
        );
    }
    let pump = &summary.queues[2].phases[2].outcome.counters;
    assert!(pump.extra["trace_max_len"] as usize <= RING_CAPACITY);
    assert!(pump.extra["trace_events_dropped"] > 0);
    assert!(pump.extra["interrupts_delivered"] > 0);
    assert!(pump.extra["irq_events_dropped_by_reset"] > 0);
    assert!(pump.extra["stop_checks"] > 0);
    let uart = &summary.queues[1].phases[2].outcome.counters;
    assert!(uart.extra["bytes_delivered"] > 0 && uart.extra["events_dropped_by_reset"] > 0);
    let growth = summary
        .rss_growth_bytes
        .expect("process RSS must be measurable (ps)");
    assert!(
        growth < RSS_GROWTH_LIMIT as i64,
        "RSS grew by {growth} bytes over the baseline and main runs"
    );

    if let Ok(path) = std::env::var("S4_43B_JSON_OUT") {
        let json = serde_json::to_string_pretty(&summary).unwrap();
        std::fs::write(path, json + "\n").unwrap();
    }
}
