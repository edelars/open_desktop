//! `--stage4-interrupt-pump-paged-probe` (S4-27i4b): the paged
//! `Stage4InterruptPumpPagedProbeV1` guest (S4-27h layout, main `STI; HLT; HLT; HLT`,
//! vector 0x41) driven by the event pump over the paged backend.
//!
//! Two modes. Without `--request` the runner is the producer of a fixed IRQ queue
//! schedule in two phases (the paged form of the S4-27i3 probe): phase 1 with the
//! Local APIC disabled by the host (the request of the due event fails and the
//! event is kept), phase 2 after the owner re-enabled the APIC (the due event and
//! the future one at 250000 ns are delivered; the guest goes idle at the third HLT).
//! With `--request VECTOR@AFTER_QUANTA` the runner is a caller that raises the vector
//! in the Local APIC after that many pump steps (S4-27i1 style, no queue events; a
//! request left over when the guest idles is raised then and the run continues).

use od_machine::{
    DiagnosticTerminalExit, InterruptPump, PagedBackend, PumpEntrySource, PumpExit, PumpInjection,
    PumpRejection, PumpTraceKind, STAGE4_INTERRUPT_PUMP_PAGED_IMAGE_BYTES,
    STAGE4_INTERRUPT_PUMP_PAGED_STACK_GPA, STAGE4_INTERRUPT_PUMP_PAGED_STACK_GVA,
    STAGE4_INTERRUPT_PUMP_PAGED_TSC_HZ, STAGE4_INTERRUPT_PUMP_PAGED_VECTOR,
    Stage4InterruptPumpPagedStart, enable_stage4_interrupt_pump_paged_apic,
    load_stage4_interrupt_pump_paged_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4InterruptPumpPagedProbeV1";
const DEFAULT_BUDGET: u64 = 4096;
const MAX_BUDGET: u64 = 100_000;
const MAX_REQUESTS: usize = 8;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;
const VECTOR: u8 = STAGE4_INTERRUPT_PUMP_PAGED_VECTOR;
const IMAGE_BYTES: usize = STAGE4_INTERRUPT_PUMP_PAGED_IMAGE_BYTES;
/// Queue schedule (key, due_ns): one due event and one future event.
const PHASE1: [(u64, u64); 2] = [(1, 0), (2, 250_000)];
/// GPAs of the leaf PTEs of main, handler, IDT, GDT, Local APIC page and stack.
const PTE_GPAS: [u64; 6] = [0x4008, 0x4010, 0x5000, 0x5008, 0x5010, 0x5800];

struct Options {
    path: String,
    budget: u64,
    /// `(vector, after_quanta)` caller requests; empty selects the queue schedule.
    requests: Vec<(u8, u64)>,
    inject_pop_before_request: bool,
}

pub fn run(args: &[String]) -> ExitCode {
    let (result, code) = match run_inner(args) {
        Ok(result) | Err(result) => result,
    };
    println!("{result}");
    ExitCode::from(code)
}

fn parse_number(value: &str) -> Result<u64, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse::<u64>()
    }
    .map_err(|_| format!("invalid number '{value}'"))
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut path = None;
    let mut budget = None;
    let mut requests = Vec::new();
    let mut inject_pop_before_request = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--inject-pop-before-request" {
            if inject_pop_before_request {
                return Err("--inject-pop-before-request may be specified only once".into());
            }
            inject_pop_before_request = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-interrupt-pump-paged-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-interrupt-pump-paged-probe" => {
                return Err("probe may be specified only once".into());
            }
            "--budget" if budget.is_none() => {
                let parsed = parse_number(value)?;
                if parsed > MAX_BUDGET {
                    return Err(format!("--budget must be between 0 and {MAX_BUDGET}"));
                }
                budget = Some(parsed);
            }
            "--budget" => return Err("--budget may be specified only once".into()),
            "--request" => {
                let (vector, after) = value.split_once('@').ok_or_else(|| {
                    format!("--request expects VECTOR@AFTER_QUANTA, got '{value}'")
                })?;
                if requests.len() >= MAX_REQUESTS {
                    return Err(format!(
                        "at most {MAX_REQUESTS} --request events are accepted"
                    ));
                }
                let vector = u8::try_from(parse_number(vector)?)
                    .map_err(|_| format!("vector out of range '{vector}'"))?;
                requests.push((vector, parse_number(after)?));
            }
            other => return Err(format!("unexpected option '{other}'")),
        }
        index += 2;
    }
    if inject_pop_before_request && !requests.is_empty() {
        return Err(
            "--inject-pop-before-request applies to the queue schedule, not --request".into(),
        );
    }
    requests.sort_by_key(|(_, after)| *after);
    Ok(Options {
        path: path
            .ok_or_else(|| "--stage4-interrupt-pump-paged-probe IMAGE is required".to_string())?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        requests,
        inject_pop_before_request,
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(IMAGE_BYTES + 1);
    file.take((IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {IMAGE_BYTES} bytes; read {}",
            image.len()
        ));
    }
    Ok(image)
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": status,
        "error": {"kind": kind, "detail": detail},
    })
}

fn source_name(source: PumpEntrySource) -> &'static str {
    match source {
        PumpEntrySource::Boundary => "boundary",
        PumpEntrySource::HaltWake => "halt_wake",
    }
}

fn trace_json(event: &od_machine::PumpTraceEvent) -> Value {
    let mut value = json!({
        "seq": event.seq,
        "rip": format!("0x{:016x}", event.rip),
        "clock_ns": event.clock_ns,
        "event": event.kind.name(),
    });
    match &event.kind {
        PumpTraceKind::Irr { vector }
        | PumpTraceKind::Eoi { vector }
        | PumpTraceKind::Coalesced { vector } => {
            value["vector"] = json!(vector);
        }
        PumpTraceKind::Entry { vector, source } => {
            value["vector"] = json!(vector);
            value["source"] = json!(source_name(*source));
        }
        PumpTraceKind::Sti | PumpTraceKind::Cli | PumpTraceKind::Hlt | PumpTraceKind::Iretq => {}
        // Owner events (S4-27i5): this probe attaches no owner hook.
        PumpTraceKind::Host(_) => {}
    }
    value
}

fn hex(value: u64) -> String {
    format!("0x{value:016x}")
}

type Pump = InterruptPump<PagedBackend>;

/// Interrupt frame (low to high address) at the RSP right after an entry; the
/// stack is one page, mapped GVA 0x700000 -> GPA 0xa000.
fn entry_record(pump: &mut Pump, vector: u8, source: &str) -> Value {
    let rsp = pump.state().gpr[od_isa::Register::Rsp as usize];
    let frame: Vec<Value> = (0..5)
        .map(|index| {
            let gpa = rsp
                .checked_sub(STAGE4_INTERRUPT_PUMP_PAGED_STACK_GVA)
                .map(|offset| STAGE4_INTERRUPT_PUMP_PAGED_STACK_GPA + offset + 8 * index);
            gpa.and_then(|gpa| pump.peek_ram_qword(gpa))
                .map_or(Value::Null, |word| json!(format!("0x{word:x}")))
        })
        .collect();
    json!({
        "vector": vector,
        "source": source,
        "time_ns": pump.virtual_time_ns(),
        "rip_after_entry": hex(pump.state().rip),
        "rsp_after_entry": hex(rsp),
        "rflags_after_entry": hex(pump.state().rflags),
        "frame_low_to_high": frame,
    })
}

/// One-pump-step driver with a budget shared by all phases; samples the virtual
/// time and the interrupt frame right after each entry and raises caller requests
/// at exact quanta.
struct Driver {
    budget: u64,
    quanta: u64,
    retired: u64,
    seen_trace: usize,
    entries: Vec<Value>,
    requests: Vec<(u8, u64)>,
    next_request: usize,
    integration_error: Option<String>,
}

impl Driver {
    fn raise_requests(&mut self, pump: &Pump, all: bool) -> bool {
        let mut raised = false;
        while let Some((vector, after)) = self.requests.get(self.next_request).copied() {
            if !all && after > self.quanta {
                break;
            }
            self.next_request += 1;
            raised = true;
            if let Err(error) = pump.apic().request_fixed_edge(vector) {
                self.integration_error = Some(format!("request {vector:#x}: {error:?}"));
                break;
            }
        }
        raised
    }

    fn run_phase(&mut self, pump: &mut Pump) -> PumpExit {
        loop {
            self.raise_requests(pump, false);
            if self.integration_error.is_some() {
                return PumpExit::Budget;
            }
            let result = pump.run(1.min(self.budget.saturating_sub(self.quanta)), || false);
            self.quanta += result.quanta;
            self.retired += result.retired;
            let events: Vec<_> = pump.trace()[self.seen_trace..].to_vec();
            self.seen_trace += events.len();
            for event in events {
                if let PumpTraceKind::Entry { vector, source } = event.kind {
                    self.entries
                        .push(entry_record(pump, vector, source_name(source)));
                }
            }
            match result.exit {
                PumpExit::Budget if self.quanta < self.budget => {}
                // Idle with a request still to come: the caller raises it now.
                PumpExit::IdleHalt if self.next_request < self.requests.len() => {
                    self.raise_requests(pump, true);
                    if self.integration_error.is_some() {
                        return PumpExit::Budget;
                    }
                }
                exit => return exit,
            }
        }
    }
}

fn exit_name(exit: &PumpExit) -> &'static str {
    match exit {
        PumpExit::IdleHalt => "idle_halt",
        PumpExit::Budget => "budget",
        PumpExit::Stopped => "stopped",
        PumpExit::Terminal(_) => "terminal",
        PumpExit::Rejected(_) => "rejected",
    }
}

fn schedule(pump: &mut Pump, events: &[(u64, u64)]) -> Result<(), (Value, u8)> {
    let generation = pump.irq_generation();
    for (key, due_ns) in events {
        pump.schedule_irq(generation, *key, VECTOR, *due_ns)
            .map_err(|error| {
                (
                    failure("rejected", "schedule", format!("{error:?}")),
                    EXIT_GUEST,
                )
            })?;
    }
    Ok(())
}

fn guest_error(kind: &str, detail: String) -> (Value, u8) {
    (failure("rejected", kind, detail), EXIT_GUEST)
}

fn run_inner(args: &[String]) -> Result<(Value, u8), (Value, u8)> {
    let options = parse_args(args).map_err(|detail| {
        (
            failure("invalid_input", "arguments", detail),
            EXIT_INVALID_INPUT,
        )
    })?;
    let image = read_image(&options.path)
        .map_err(|detail| (failure("invalid_input", "file", detail), EXIT_INVALID_INPUT))?;
    let mut pump = load_stage4_interrupt_pump_paged_probe(
        &image,
        Stage4InterruptPumpPagedStart::InterruptsOff,
    )
    .map_err(|error| {
        (
            failure("loader_error", "loader", format!("{error:?}")),
            EXIT_LOADER,
        )
    })?;
    if options.inject_pop_before_request {
        pump = pump.with_injection(PumpInjection::PopBeforeRequest);
    }
    let caller_mode = !options.requests.is_empty();
    let mut driver = Driver {
        budget: options.budget,
        quanta: 0,
        retired: 0,
        seen_trace: 0,
        entries: Vec::new(),
        requests: options.requests.clone(),
        next_request: 0,
        integration_error: None,
    };

    let mut phase1 = Value::Null;
    let mut phase1_ok = true;
    let mut exit;
    if caller_mode {
        exit = driver.run_phase(&mut pump);
    } else {
        // Phase 1: the host disables the APIC, so the request of the due event fails;
        // the event stays queued (unless the injection removed it first).
        pump.apic()
            .set_global_enabled(false)
            .map_err(|error| guest_error("apic", format!("{error:?}")))?;
        schedule(&mut pump, &PHASE1)?;
        let phase1_exit = driver.run_phase(&mut pump);
        phase1_ok = matches!(
            phase1_exit,
            PumpExit::Rejected(PumpRejection::QueueRequest { .. })
        );
        phase1 = json!({
            "exit": exit_name(&phase1_exit),
            "error": match &phase1_exit {
                PumpExit::Rejected(PumpRejection::QueueRequest { error, .. }) => {
                    Value::String(format!("QueueRequest({error:?})"))
                }
                PumpExit::Rejected(other) => Value::String(format!("{other:?}")),
                _ => Value::Null,
            },
            "retired": driver.retired,
            "queue_len": pump.irq_queue_len(),
            "rip": hex(pump.state().rip),
        });
        exit = phase1_exit;
        if phase1_ok {
            // Phase 2: the APIC is enabled again (a global disable resets it, so the
            // owner repeats the loader's SVR write); the due and the future event
            // are delivered.
            pump.apic()
                .set_global_enabled(true)
                .map_err(|error| guest_error("apic", format!("{error:?}")))?;
            enable_stage4_interrupt_pump_paged_apic(&mut pump)
                .map_err(|error| guest_error("apic", format!("{error:?}")))?;
            exit = driver.run_phase(&mut pump);
        }
    }

    let (status, error) = match (&driver.integration_error, &exit) {
        (Some(detail), _) => (
            "integration_error",
            json!({"kind": "integration_error", "detail": detail}),
        ),
        (None, PumpExit::IdleHalt) if phase1_ok => ("guest_idle", Value::Null),
        (None, PumpExit::IdleHalt) => (
            "rejected",
            json!({"kind": "phase1", "detail": "the request of the due event did not fail in phase 1"}),
        ),
        (None, PumpExit::Budget) => (
            "work_limit",
            json!({"kind": "budget_exhausted", "detail": "step budget exhausted before the guest went idle"}),
        ),
        (None, PumpExit::Stopped) => (
            "host_cancelled",
            json!({"kind": "stopped", "detail": "stop requested"}),
        ),
        (None, PumpExit::Terminal(cause)) => (
            "guest_terminal",
            json!({"kind": "guest_terminal", "detail": terminal_name(cause)}),
        ),
        (None, PumpExit::Rejected(rejection)) => (
            "rejected",
            json!({"kind": "rejected", "detail": format!("{rejection:?}")}),
        ),
    };
    let handler_returns = pump
        .trace()
        .iter()
        .filter(|event| event.kind == PumpTraceKind::Iretq)
        .count();
    let eois = pump
        .trace()
        .iter()
        .filter(|event| matches!(event.kind, PumpTraceKind::Eoi { .. }))
        .count();
    let ptes: Vec<Value> = PTE_GPAS
        .iter()
        .map(|gpa| {
            pump.peek_ram_qword(*gpa)
                .map_or(Value::Null, |pte| json!(format!("0x{pte:x}")))
        })
        .collect();
    let (irr, isr) = pump.apic().vector_state(VECTOR);
    let state = pump.state();
    let reg = |register: od_isa::Register| hex(state.gpr[register as usize]);
    let result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": exit_name(&exit),
        "mode": if caller_mode { "caller_requests" } else { "queue_schedule" },
        "budget": options.budget,
        "quanta": driver.quanta,
        "retired": driver.retired,
        "retired_total": pump.retired_total(),
        "waiting": pump.is_waiting(),
        "injection": options.inject_pop_before_request.then_some("PopBeforeRequest"),
        "tsc_hz": STAGE4_INTERRUPT_PUMP_PAGED_TSC_HZ,
        "time_ns": pump.virtual_time_ns(),
        "phase1": phase1,
        "queue_len": pump.irq_queue_len(),
        "coalesced": pump.coalesced_irqs(),
        "wakeups": driver.entries.len(),
        "handler_returns": handler_returns,
        "eois": eois,
        "rip": hex(state.rip),
        "rflags": hex(state.rflags),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rsp": reg(od_isa::Register::Rsp),
        "vectors": {format!("0x{VECTOR:x}"): {"irr": irr, "isr": isr}},
        "ptes_main_handler_idt_gdt_lapic_stack": ptes,
        "requests": options.requests.iter().map(|(vector, after)| {
            json!({"vector": vector, "after_quanta": after})
        }).collect::<Vec<_>>(),
        "entries": driver.entries,
        "trace": pump.trace().iter().map(trace_json).collect::<Vec<_>>(),
        "error": error,
    });
    let code = if status == "guest_idle" {
        0
    } else {
        EXIT_GUEST
    };
    if code == 0 {
        Ok((result, code))
    } else {
        Err((result, code))
    }
}

fn terminal_name(cause: &DiagnosticTerminalExit) -> String {
    format!("{cause:?}")
}
