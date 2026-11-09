//! `--stage4-pit-irq0-probe` (S4-29f): PIT channel 0 -> OUT0 -> IOAPIC INTIN2
//! (GSI 2) -> Local APIC -> guest handler on the flat `Stage4PitIrq0ProbeV1`
//! kernel, driven by the event pump with the PIT bridge as its owner hook.
//!
//! `--stage4-pit-irq0-probe IMAGE [--budget N] [--inject-masked-entry2 |
//! --inject-no-eoi | --inject-no-generation]`. The expected evidence of the
//! accepted image is derived by `tools/build/build_stage4_pit_irq0_probe.py`
//! (PIT formula and bridge rules), not by this runner.

use od_machine::{
    PitChannel, PumpEntrySource, PumpExit, PumpRejection, PumpTraceEvent, PumpTraceKind,
    STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES, STAGE4_PIT_IRQ0_PROBE_VECTOR, Stage4PitIrq0Injection,
    isa_irq_vector, load_stage4_pit_irq0_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4PitIrq0ProbeV1";
const DEFAULT_BUDGET: u64 = 4096;
const MAX_BUDGET: u64 = 100_000;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;
const VECTOR: u8 = STAGE4_PIT_IRQ0_PROBE_VECTOR;

struct Options {
    path: String,
    budget: u64,
    injection: Option<Stage4PitIrq0Injection>,
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
    let mut injection = None;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        let flag = match option {
            "--inject-masked-entry2" => Some(Stage4PitIrq0Injection::MaskedEntry2),
            "--inject-no-eoi" => Some(Stage4PitIrq0Injection::NoEoi),
            "--inject-no-generation" => Some(Stage4PitIrq0Injection::NoGeneration),
            _ => None,
        };
        if let Some(flag) = flag {
            if injection.replace(flag).is_some() {
                return Err("at most one --inject-* option may be given".into());
            }
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-pit-irq0-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-pit-irq0-probe" => return Err("probe may be specified only once".into()),
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
        path: path.ok_or_else(|| "--stage4-pit-irq0-probe IMAGE is required".to_string())?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        injection,
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES} bytes; read {}",
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

fn trace_json(event: &PumpTraceEvent) -> Value {
    let mut value = json!({
        "seq": event.seq,
        "clock_ns": event.clock_ns,
        "rip": format!("0x{:016x}", event.rip),
        "event": event.kind.name(),
    });
    match &event.kind {
        PumpTraceKind::Irr { vector }
        | PumpTraceKind::Eoi { vector }
        | PumpTraceKind::Coalesced { vector } => value["vector"] = json!(vector),
        PumpTraceKind::Entry { vector, source } => {
            value["vector"] = json!(vector);
            value["source"] = json!(match source {
                PumpEntrySource::Boundary => "boundary",
                PumpEntrySource::HaltWake => "halt_wake",
            });
        }
        PumpTraceKind::Host(host) => {
            // Owner events: `a` and `b` are defined by the PIT bridge.
            match host.name {
                "port_in" | "port_out" => {
                    value["port"] = json!(format!("0x{:03x}", host.a));
                    value["value"] = json!(host.b);
                }
                "out0_edge" => value["deadline_ns"] = json!(host.a),
                "pit_schedule" => {
                    value["deadline_ns"] = json!(host.a);
                    value["tick"] = json!(host.b);
                }
                "pit_cancel" => {
                    value["dropped"] = json!(host.a);
                    value["generation_change"] = json!(host.b);
                }
                "idle_advance" => value["to_ns"] = json!(host.a),
                _ => {
                    value["a"] = json!(host.a);
                    value["b"] = json!(host.b);
                }
            }
        }
        PumpTraceKind::Sti | PumpTraceKind::Cli | PumpTraceKind::Hlt | PumpTraceKind::Iretq => {}
    }
    value
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
    let mut probe = load_stage4_pit_irq0_probe(&image, options.injection).map_err(|error| {
        (
            failure("loader_error", "loader", format!("{error:?}")),
            EXIT_LOADER,
        )
    })?;
    let report = probe.run(options.budget, || false);

    let (exit_name, status, error) = match &report.exit {
        PumpExit::IdleHalt => ("idle_halt", "guest_idle", Value::Null),
        PumpExit::Budget => (
            "budget_exhausted",
            "work_limit",
            json!({"kind": "budget_exhausted", "detail": "step budget exhausted before the guest went idle"}),
        ),
        PumpExit::Stopped => (
            "stopped",
            "host_cancelled",
            json!({"kind": "stopped", "detail": "probe stop predicate returned true"}),
        ),
        PumpExit::Terminal(cause) => (
            "terminal",
            "guest_terminal",
            json!({"kind": "guest_terminal", "detail": format!("{cause:?}")}),
        ),
        PumpExit::Rejected(PumpRejection::Host(detail)) => (
            "integration_error",
            "integration_error",
            json!({"kind": "integration_error", "detail": detail}),
        ),
        PumpExit::Rejected(rejection) => (
            "rejected",
            "rejected",
            json!({"kind": "rejected", "detail": format!("{rejection:?}")}),
        ),
    };
    let counter = probe.counter();
    let wakeups = probe.wakeups();
    let clock_ns = probe.time_ns();
    let stats = probe.stats();
    let pic_vector = isa_irq_vector(0);
    let trace = probe.pump().trace();
    let count = |f: fn(&PumpTraceKind) -> bool| trace.iter().filter(|e| f(&e.kind)).count();
    let entries = count(|k| matches!(k, PumpTraceKind::Entry { .. }));
    let eois = count(|k| matches!(k, PumpTraceKind::Eoi { .. }));
    let iretqs = count(|k| matches!(k, PumpTraceKind::Iretq));
    let irrs = count(|k| matches!(k, PumpTraceKind::Irr { .. }));
    let pic_events = trace
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                PumpTraceKind::Irr { vector } | PumpTraceKind::Entry { vector, .. }
                    | PumpTraceKind::Eoi { vector } if vector == pic_vector
            )
        })
        .count();
    let entry_times: Vec<Value> = trace
        .iter()
        .filter(|e| matches!(e.kind, PumpTraceKind::Entry { vector, .. } if vector == VECTOR))
        .map(|e| json!(e.clock_ns))
        .collect();
    let pic_irr_reads: Vec<Value> = trace
        .iter()
        .filter_map(|e| match &e.kind {
            PumpTraceKind::Host(host) if host.name == "port_in" && host.a == 0x20 => {
                Some(json!(host.b))
            }
            _ => None,
        })
        .collect();
    let trace_values: Vec<Value> = trace.iter().map(trace_json).collect();
    let (vector_irr, vector_isr) = probe.pump().apic().vector_state(VECTOR);
    let pic_state = probe.pump().apic().vector_state(pic_vector);
    let master = probe.pic_master().snapshot();
    let state = probe.pump().state();
    let reg = |r: od_isa::Register| format!("0x{:016x}", state.gpr[r as usize]);
    let result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": exit_name,
        "budget": options.budget,
        "injection": options.injection.map(|injection| format!("{injection:?}")),
        "quanta": report.quanta,
        "retired": report.retired,
        "retired_total": probe.pump().retired_total(),
        "clock_ns": clock_ns,
        "rip": format!("0x{:016x}", state.rip),
        "rflags": format!("0x{:016x}", state.rflags),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rax": reg(od_isa::Register::Rax),
        "rdx": reg(od_isa::Register::Rdx),
        "rsp": reg(od_isa::Register::Rsp),
        "counter": counter,
        "wakeups": wakeups,
        "entry_times_ns": entry_times,
        "counts": {
            "irr": irrs,
            "irq_entry": entries,
            "eoi": eois,
            "iretq": iretqs,
        },
        "apic": {
            "vector": VECTOR,
            "irr": vector_irr,
            "isr": vector_isr,
        },
        "pic": {
            "vector": pic_vector,
            "irr": pic_state.0,
            "isr": pic_state.1,
            "events": pic_events,
            "irr_reads": pic_irr_reads,
            "master_imr": master.imr,
            "master_icw2": master.icw2,
            "master_phase": format!("{:?}", master.phase),
        },
        "bridge": {
            "edges": stats.edges,
            "scheduled": stats.scheduled,
            "generation_changes": stats.generation_changes,
            "cancelled": stats.cancelled,
            "pending_events": probe.pending_events(),
            "next_deadline_ns": probe.next_event_deadline(),
        },
        "pit": {
            "channel0_count": probe.pit().count(PitChannel::Channel0),
            "channel0_out": probe.pit().out(PitChannel::Channel0),
        },
        "trace": trace_values,
        "error": error,
    });
    if status == "guest_idle" {
        Ok((result, 0))
    } else {
        Err((result, EXIT_GUEST))
    }
}
