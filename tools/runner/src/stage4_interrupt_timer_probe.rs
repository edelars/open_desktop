//! `--stage4-interrupt-timer-probe` (S4-27i2): the flat `Stage4InterruptTimerProbeV1`
//! guest driven by the event pump, which owns the periodic APIC timer (dispatch on
//! every boundary, idle advance in the halt wait, 0 ns per instruction).

use od_machine::{
    InterruptPump, PumpEntrySource, PumpExit, PumpInjection, PumpRejection, PumpTraceKind,
    STAGE4_INTERRUPT_TIMER_PROBE_COUNTER_GPA, STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_BYTES,
    STAGE4_INTERRUPT_TIMER_PROBE_TIMER_HZ, STAGE4_INTERRUPT_TIMER_PROBE_TSC_HZ,
    STAGE4_INTERRUPT_TIMER_PROBE_VECTOR, load_stage4_interrupt_timer_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4InterruptTimerProbeV1";
const DEFAULT_BUDGET: u64 = 4096;
const MAX_BUDGET: u64 = 100_000;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;

struct Options {
    path: String,
    budget: u64,
    inject_no_dispatch: bool,
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
    let mut inject_no_dispatch = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--inject-no-dispatch" {
            if inject_no_dispatch {
                return Err("--inject-no-dispatch may be specified only once".into());
            }
            inject_no_dispatch = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-interrupt-timer-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-interrupt-timer-probe" => {
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
        path: path.ok_or_else(|| "--stage4-interrupt-timer-probe IMAGE is required".to_string())?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        inject_no_dispatch,
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {STAGE4_INTERRUPT_TIMER_PROBE_IMAGE_BYTES} bytes; read {}",
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
        "time_ns": pump.virtual_time_ns(),
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
    let mut pump = load_stage4_interrupt_timer_probe(&image).map_err(|error| {
        (
            failure("loader_error", "loader", format!("{error:?}")),
            EXIT_LOADER,
        )
    })?;
    if options.inject_no_dispatch {
        pump = pump.with_injection(PumpInjection::NoTimerDispatch);
    }

    // One pump step at a time, so the virtual time and the interrupt frame can be
    // sampled right after each entry.
    let mut quanta = 0_u64;
    let mut retired = 0_u64;
    let mut entries = Vec::new();
    let mut seen_trace = 0;
    let exit = loop {
        let result = pump.run(1.min(options.budget.saturating_sub(quanta)), || false);
        quanta += result.quanta;
        retired += result.retired;
        let events: Vec<_> = pump.trace()[seen_trace..].to_vec();
        seen_trace += events.len();
        for event in events {
            if let PumpTraceKind::Entry { vector, source } = event.kind {
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

    let (exit_name, status, error) = match &exit {
        PumpExit::IdleHalt => ("idle_halt", "guest_idle", Value::Null),
        PumpExit::Budget => (
            "budget",
            "work_limit",
            json!({"kind": "budget_exhausted", "detail": "step budget exhausted before the guest went idle"}),
        ),
        PumpExit::Stopped => (
            "stopped",
            "host_cancelled",
            json!({"kind": "stopped", "detail": "stop requested"}),
        ),
        PumpExit::Terminal(cause) => (
            "terminal",
            "guest_terminal",
            json!({"kind": "guest_terminal", "detail": format!("{cause:?}")}),
        ),
        PumpExit::Rejected(rejection) => (
            "rejected",
            "rejected",
            json!({"kind": "rejected", "detail": rejection_name(rejection)}),
        ),
    };
    let counter = pump
        .peek_ram_qword(STAGE4_INTERRUPT_TIMER_PROBE_COUNTER_GPA)
        .map(|value| value & 0xffff_ffff);
    let vector = STAGE4_INTERRUPT_TIMER_PROBE_VECTOR;
    let (irr, isr) = pump.apic().vector_state(vector);
    let state = pump.state();
    let reg = |register: od_isa::Register| hex(state.gpr[register as usize]);
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
        "injection": options.inject_no_dispatch.then_some("NoTimerDispatch"),
        "timer_hz": STAGE4_INTERRUPT_TIMER_PROBE_TIMER_HZ,
        "tsc_hz": STAGE4_INTERRUPT_TIMER_PROBE_TSC_HZ,
        "time_ns": pump.virtual_time_ns(),
        "timer_deadline_ns": pump.apic().next_timer_deadline(),
        "counter": counter,
        "wakeups": entries.len(),
        "rip": hex(state.rip),
        "rflags": hex(state.rflags),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rsp": reg(od_isa::Register::Rsp),
        "vectors": {format!("0x{vector:x}"): {"irr": irr, "isr": isr}},
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
