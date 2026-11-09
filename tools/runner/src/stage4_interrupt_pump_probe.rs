//! `--stage4-interrupt-pump-probe` (S4-27i1): the flat `Stage4InterruptPumpProbeV1`
//! guest driven by the event pump. Interrupt requests come only from the caller
//! (`--request VECTOR@AFTER_QUANTA`) or from guest MMIO (self-IPI).

use od_machine::{
    InterruptPump, PumpEntrySource, PumpExit, PumpInjection, PumpRejection, PumpTraceKind,
    STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES, STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_A,
    STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_B, load_stage4_interrupt_pump_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4InterruptPumpProbeV1";
const DEFAULT_BUDGET: u64 = 4096;
const MAX_BUDGET: u64 = 100_000;
const MAX_REQUESTS: usize = 8;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;

struct Options {
    path: String,
    budget: u64,
    /// `(vector, after_quanta)` caller requests.
    requests: Vec<(u8, u64)>,
    injection: Option<PumpInjection>,
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
    let mut injection = None;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        let inject = match option {
            "--inject-skip-shadow-check" => Some(PumpInjection::SkipShadowCheck),
            "--inject-hlt-as-ordinary" => Some(PumpInjection::HltAsOrdinary),
            _ => None,
        };
        if let Some(inject) = inject {
            if injection.replace(inject).is_some() {
                return Err("at most one --inject-* flag is accepted".into());
            }
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-interrupt-pump-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-interrupt-pump-probe" => {
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
                // VECTOR@AFTER_QUANTA: a fixed edge raised after that many pump steps.
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
    requests.sort_by_key(|(_, after)| *after);
    Ok(Options {
        path: path.ok_or_else(|| "--stage4-interrupt-pump-probe IMAGE is required".to_string())?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        requests,
        injection,
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {STAGE4_INTERRUPT_PUMP_PROBE_IMAGE_BYTES} bytes; read {}",
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
        PumpTraceKind::Irr { vector } | PumpTraceKind::Eoi { vector } => {
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
        "rip_after_entry": hex(pump.state().rip),
        "rsp_after_entry": hex(rsp),
        "rflags_after_entry": hex(pump.state().rflags),
        "frame_low_to_high": frame,
    })
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
    let mut pump = load_stage4_interrupt_pump_probe(&image).map_err(|error| {
        (
            failure("loader_error", "loader", format!("{error:?}")),
            EXIT_LOADER,
        )
    })?;
    if let Some(injection) = options.injection {
        pump = pump.with_injection(injection);
    }

    // One pump step at a time, so that caller requests land at an exact quantum
    // and the interrupt frame can be sampled right after each entry.
    let mut quanta = 0_u64;
    let mut retired = 0_u64;
    let mut requests = options.requests.iter().peekable();
    let mut entries = Vec::new();
    let mut seen_trace = 0;
    let mut integration_error = None;
    let exit = loop {
        while let Some((vector, _)) = requests.next_if(|(_, after)| *after <= quanta) {
            if let Err(error) = pump.apic().request_fixed_edge(*vector) {
                integration_error = Some(format!("request {vector:#x}: {error:?}"));
            }
        }
        if integration_error.is_some() {
            break PumpExit::Budget;
        }
        let result = pump.run(1.min(options.budget.saturating_sub(quanta)), || false);
        quanta += result.quanta;
        retired += result.retired;
        let events: Vec<_> = pump.trace()[seen_trace..].to_vec();
        seen_trace += events.len();
        for event in events {
            if let PumpTraceKind::Entry { vector, source } = event.kind {
                // The first handler instruction of a boundary entry retired
                // inside the same quantum; the frame is still at the RSP.
                let name = match source {
                    PumpEntrySource::Boundary => "boundary",
                    PumpEntrySource::HaltWake => "halt_wake",
                };
                entries.push(entry_record(&mut pump, vector, name));
            }
        }
        match result.exit {
            PumpExit::Budget if quanta < options.budget => {}
            exit => break exit,
        }
    };

    let (exit_name, status, error) = match (&integration_error, &exit) {
        (Some(detail), _) => (
            "integration_error",
            "integration_error",
            json!({"kind": "integration_error", "detail": detail}),
        ),
        (None, PumpExit::IdleHalt) => ("idle_halt", "guest_idle", Value::Null),
        (None, PumpExit::Budget) => (
            "budget",
            "work_limit",
            json!({"kind": "budget_exhausted", "detail": "step budget exhausted before the guest went idle"}),
        ),
        (None, PumpExit::Stopped) => (
            "stopped",
            "host_cancelled",
            json!({"kind": "stopped", "detail": "stop requested"}),
        ),
        (None, PumpExit::Terminal(cause)) => (
            "terminal",
            "guest_terminal",
            json!({"kind": "guest_terminal", "detail": format!("{cause:?}")}),
        ),
        (None, PumpExit::Rejected(rejection)) => (
            "rejected",
            "rejected",
            json!({"kind": "rejected", "detail": rejection_name(rejection)}),
        ),
    };
    let state = pump.state();
    let reg = |register: od_isa::Register| hex(state.gpr[register as usize]);
    let vectors: serde_json::Map<String, Value> = [
        STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_B,
        STAGE4_INTERRUPT_PUMP_PROBE_VECTOR_A,
    ]
    .iter()
    .map(|vector| {
        let (irr, isr) = pump.apic().vector_state(*vector);
        (format!("0x{vector:x}"), json!({"irr": irr, "isr": isr}))
    })
    .collect();
    let result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": exit_name,
        "budget": options.budget,
        "quanta": quanta,
        "retired": retired,
        "retired_total": pump.retired_total(),
        "waiting": pump.is_waiting(),
        "injection": options.injection.map(|injection| format!("{injection:?}")),
        "rip": hex(state.rip),
        "rflags": hex(state.rflags),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rsp": reg(od_isa::Register::Rsp),
        "rdi": reg(od_isa::Register::Rdi),
        "rcx": reg(od_isa::Register::Rcx),
        "vectors": vectors,
        "requests": options.requests.iter().map(|(vector, after)| {
            json!({"vector": vector, "after_quanta": after})
        }).collect::<Vec<_>>(),
        "entries": entries,
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

fn rejection_name(rejection: &PumpRejection) -> String {
    format!("{rejection:?}")
}
