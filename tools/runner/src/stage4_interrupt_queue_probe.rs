//! `--stage4-interrupt-queue-probe` (S4-27i3): the flat `Stage4InterruptQueueProbeV1`
//! guest driven by the event pump, which owns the IRQ event queue. The runner is
//! the caller (producer) of a fixed schedule in three phases (see the builder
//! `tools/build/build_stage4_interrupt_queue_probe.py`, which derives the expected
//! evidence): phase 1 with the APIC disabled by the host (the request of the due
//! event fails and the event is kept), phase 2 after enabling it, phase 3 with two
//! due events of one vector (one merged IRR edge).

use od_machine::{
    InterruptPump, PumpEntrySource, PumpExit, PumpInjection, PumpRejection, PumpTraceKind,
    STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA, STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES,
    STAGE4_INTERRUPT_QUEUE_PROBE_TSC_HZ, STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR,
    load_stage4_interrupt_queue_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4InterruptQueueProbeV1";
const DEFAULT_BUDGET: u64 = 4096;
const MAX_BUDGET: u64 = 100_000;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;
const VECTOR: u8 = STAGE4_INTERRUPT_QUEUE_PROBE_VECTOR;
/// Caller schedule (key, due_ns); the builder derives the expected evidence from it.
const PHASE1: [(u64, u64); 2] = [(1, 0), (2, 250_000)];
const PHASE3: [(u64, u64); 2] = [(3, 250_000), (4, 250_000)];

struct Options {
    path: String,
    budget: u64,
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
            "--stage4-interrupt-queue-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-interrupt-queue-probe" => {
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
            other => return Err(format!("unexpected option '{other}'")),
        }
        index += 2;
    }
    Ok(Options {
        path: path.ok_or_else(|| "--stage4-interrupt-queue-probe IMAGE is required".to_string())?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        inject_pop_before_request,
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {STAGE4_INTERRUPT_QUEUE_PROBE_IMAGE_BYTES} bytes; read {}",
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

fn trace_json(event: &od_machine::PumpTraceEvent) -> Value {
    let mut value = json!({
        "seq": event.seq,
        "rip": format!("0x{:016x}", event.rip),
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
            value["source"] = json!(match source {
                PumpEntrySource::Boundary => "boundary",
                PumpEntrySource::HaltWake => "halt_wake",
            });
        }
        PumpTraceKind::Sti | PumpTraceKind::Cli | PumpTraceKind::Hlt | PumpTraceKind::Iretq => {}
    }
    value
}

fn hex(value: u64) -> String {
    format!("0x{value:016x}")
}

/// Interrupt frame (low to high address) at the RSP right after an entry.
fn entry_record(pump: &mut InterruptPump, vector: u8, source: &str) -> Value {
    let rsp = pump.state().gpr[od_isa::Register::Rsp as usize];
    let frame: Vec<Value> = (0..5)
        .map(|index| {
            pump.peek_ram_qword(rsp + 8 * index)
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
/// time and the interrupt frame right after each entry.
struct Driver {
    budget: u64,
    quanta: u64,
    retired: u64,
    seen_trace: usize,
    entries: Vec<Value>,
}

impl Driver {
    fn run_phase(&mut self, pump: &mut InterruptPump) -> PumpExit {
        loop {
            let result = pump.run(1.min(self.budget.saturating_sub(self.quanta)), || false);
            self.quanta += result.quanta;
            self.retired += result.retired;
            let events: Vec<_> = pump.trace()[self.seen_trace..].to_vec();
            self.seen_trace += events.len();
            for event in events {
                if let PumpTraceKind::Entry { vector, source } = event.kind {
                    let name = match source {
                        PumpEntrySource::Boundary => "boundary",
                        PumpEntrySource::HaltWake => "halt_wake",
                    };
                    self.entries.push(entry_record(pump, vector, name));
                }
            }
            match result.exit {
                PumpExit::Budget if self.quanta < self.budget => {}
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

fn schedule(pump: &mut InterruptPump, events: &[(u64, u64)]) -> Result<(), (Value, u8)> {
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

fn run_inner(args: &[String]) -> Result<(Value, u8), (Value, u8)> {
    let options = parse_args(args).map_err(|detail| {
        (
            failure("invalid_input", "arguments", detail),
            EXIT_INVALID_INPUT,
        )
    })?;
    let image = read_image(&options.path)
        .map_err(|detail| (failure("invalid_input", "file", detail), EXIT_INVALID_INPUT))?;
    let mut pump = load_stage4_interrupt_queue_probe(&image).map_err(|error| {
        (
            failure("loader_error", "loader", format!("{error:?}")),
            EXIT_LOADER,
        )
    })?;
    if options.inject_pop_before_request {
        pump = pump.with_injection(PumpInjection::PopBeforeRequest);
    }
    let mut driver = Driver {
        budget: options.budget,
        quanta: 0,
        retired: 0,
        seen_trace: 0,
        entries: Vec::new(),
    };

    // Phase 1: the host disables the APIC, so the request of the due event fails;
    // the event stays queued (unless the injection removed it first).
    pump.apic().set_global_enabled(false).map_err(|error| {
        (
            failure("rejected", "apic", format!("{error:?}")),
            EXIT_GUEST,
        )
    })?;
    schedule(&mut pump, &PHASE1)?;
    let phase1_exit = driver.run_phase(&mut pump);
    let phase1_ok = matches!(
        phase1_exit,
        PumpExit::Rejected(PumpRejection::QueueRequest { .. })
    );
    let phase1 = json!({
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

    let mut exit = phase1_exit;
    if phase1_ok {
        // Phase 2: the APIC is enabled again; the guest runs and the due and the
        // future event are delivered.
        pump.apic().set_global_enabled(true).map_err(|error| {
            (
                failure("rejected", "apic", format!("{error:?}")),
                EXIT_GUEST,
            )
        })?;
        exit = driver.run_phase(&mut pump);
        if matches!(exit, PumpExit::IdleHalt) {
            // Phase 3: two due events of one vector at the current time.
            schedule(&mut pump, &PHASE3)?;
            exit = driver.run_phase(&mut pump);
        }
    }

    let (status, error) = match &exit {
        PumpExit::IdleHalt if phase1_ok => ("guest_idle", Value::Null),
        PumpExit::IdleHalt => (
            "rejected",
            json!({"kind": "phase1", "detail": "the request of the due event did not fail in phase 1"}),
        ),
        PumpExit::Budget => (
            "work_limit",
            json!({"kind": "budget_exhausted", "detail": "step budget exhausted before the guest went idle"}),
        ),
        PumpExit::Stopped => (
            "host_cancelled",
            json!({"kind": "stopped", "detail": "stop requested"}),
        ),
        PumpExit::Terminal(cause) => (
            "guest_terminal",
            json!({"kind": "guest_terminal", "detail": format!("{cause:?}")}),
        ),
        PumpExit::Rejected(rejection) => (
            "rejected",
            json!({"kind": "rejected", "detail": format!("{rejection:?}")}),
        ),
    };
    let counter = pump
        .peek_ram_qword(STAGE4_INTERRUPT_QUEUE_PROBE_COUNTER_GPA)
        .map(|value| value & 0xffff_ffff);
    let (irr, isr) = pump.apic().vector_state(VECTOR);
    let state = pump.state();
    let reg = |register: od_isa::Register| hex(state.gpr[register as usize]);
    let result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": exit_name(&exit),
        "budget": options.budget,
        "quanta": driver.quanta,
        "retired": driver.retired,
        "retired_total": pump.retired_total(),
        "waiting": pump.is_waiting(),
        "injection": options.inject_pop_before_request.then_some("PopBeforeRequest"),
        "tsc_hz": STAGE4_INTERRUPT_QUEUE_PROBE_TSC_HZ,
        "time_ns": pump.virtual_time_ns(),
        "phase1": phase1,
        "queue_len": pump.irq_queue_len(),
        "coalesced": pump.coalesced_irqs(),
        "counter": counter,
        "wakeups": driver.entries.len(),
        "rip": hex(state.rip),
        "rflags": hex(state.rflags),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rsp": reg(od_isa::Register::Rsp),
        "vectors": {format!("0x{VECTOR:x}"): {"irr": irr, "isr": isr}},
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
