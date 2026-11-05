use od_machine::{
    PagedProbeExit, STAGE4_FLAT_PROBE_DATA_GVA, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    load_stage4_flat_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const DEFAULT_BUDGET: u64 = 16;
const MAX_BUDGET: u64 = 1024;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;
const EXIT_PIO: u8 = 6;

pub fn run(args: &[String]) -> ExitCode {
    match run_inner(args) {
        Ok((result, code)) => emit(result, code),
        Err((result, code)) => emit(result, code),
    }
}

fn run_inner(args: &[String]) -> Result<(Value, u8), (Value, u8)> {
    let (path, budget) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(detail) => {
            return Err((
                failure("invalid_input", "arguments", detail),
                EXIT_INVALID_INPUT,
            ));
        }
    };

    let image = match read_image(&path) {
        Ok(image) => image,
        Err((kind, detail)) => {
            return Err((failure("invalid_input", kind, detail), EXIT_INVALID_INPUT));
        }
    };

    let mut machine = match load_stage4_flat_probe(&image) {
        Ok(machine) => machine,
        Err(error) => {
            return Err((
                failure("loader_error", "loader", format!("{error:?}")),
                EXIT_LOADER,
            ));
        }
    };

    let report = machine.run(budget, || false, |_| {});
    let uart = machine.uart();
    let mut serial_bytes = Vec::new();
    while uart.transfer_holding_to_shift() {
        if let Some(byte) = uart.complete_transmission() {
            serial_bytes.push(byte);
        }
    }

    let state = machine.state();
    let retired = report.instructions_executed;
    let observed_data_qword = (retired > 0).then_some(state.gpr[od_isa::Register::Rax as usize]);
    let exit = match &report.exit {
        PagedProbeExit::Halted => "halted",
        PagedProbeExit::BudgetExhausted => "budget_exhausted",
        PagedProbeExit::Stopped => "stopped",
        PagedProbeExit::Rejected(_) => "rejected",
        PagedProbeExit::Unsupported { .. } => "unsupported",
        PagedProbeExit::Fault(_) => "fault",
        PagedProbeExit::Backend(_) => "backend_error",
        PagedProbeExit::PortIo(_) => "pio_error",
    };
    let error = error_details(
        &report.exit,
        report.page_fault.as_ref(),
        report.memory_error.as_ref(),
    );
    let result = json!({
        "schema_version": 1,
        "profile": "Stage4FlatProbeV1",
        "status": if matches!(report.exit, PagedProbeExit::Halted) { "completed" } else { "error" },
        "exit": exit,
        "retired": retired,
        "rip": format!("0x{:016x}", state.rip),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rflags_defined": format!("0x{:016x}", report.rflags_defined),
        "serial_bytes": serial_bytes,
        "observed_data_qword": observed_data_qword.map(|value| format!("0x{value:016x}")),
        "error": error,
        "data_gva": format!("0x{STAGE4_FLAT_PROBE_DATA_GVA:016x}"),
    });

    let code = match report.exit {
        PagedProbeExit::Halted => 0,
        PagedProbeExit::PortIo(_) => EXIT_PIO,
        _ => EXIT_GUEST,
    };
    if code == 0 {
        Ok((result, code))
    } else {
        Err((result, code))
    }
}

fn parse_args(args: &[String]) -> Result<(String, u64), String> {
    let mut path = None;
    let mut budget = None;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-probe" => return Err("--stage4-probe may be specified only once".into()),
            "--budget" if budget.is_none() => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid budget '{value}'"))?;
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
    let path = path.ok_or_else(|| "--stage4-probe IMAGE is required".to_string())?;
    Ok((path, budget.unwrap_or(DEFAULT_BUDGET)))
}

fn read_image(path: &str) -> Result<Vec<u8>, (&'static str, String)> {
    let file =
        File::open(path).map_err(|error| ("file", format!("could not open '{path}': {error}")))?;
    let mut image = Vec::with_capacity(STAGE4_FLAT_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_FLAT_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| ("file", format!("could not read '{path}': {error}")))?;
    if image.len() != STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return Err((
            "image_size",
            format!(
                "raw image must be exactly {STAGE4_FLAT_PROBE_IMAGE_BYTES} bytes; read {}",
                image.len()
            ),
        ));
    }
    Ok(image)
}

fn error_details(
    exit: &PagedProbeExit,
    page_fault: Option<&od_isa::memory::PageFaultRecord>,
    memory_error: Option<&od_machine::TrackedAccessError>,
) -> Value {
    match exit {
        PagedProbeExit::Halted => Value::Null,
        PagedProbeExit::PortIo(error) => json!({"kind": "pio", "detail": format!("{error:?}")}),
        PagedProbeExit::BudgetExhausted => {
            json!({"kind": "budget_exhausted", "detail": "instruction budget was exhausted before HLT"})
        }
        PagedProbeExit::Stopped => {
            json!({"kind": "stopped", "detail": "probe stop predicate returned true"})
        }
        PagedProbeExit::Rejected(error) => {
            json!({"kind": "rejected", "detail": format!("{error:?}")})
        }
        PagedProbeExit::Unsupported { opcode } => {
            json!({"kind": "unsupported", "detail": format!("unsupported opcode 0x{opcode:02x}")})
        }
        PagedProbeExit::Fault(error) => {
            json!({"kind": "guest_fault", "detail": format!("{error:?}"), "page_fault": page_fault.map(|fault| format!("{fault:?}"))})
        }
        PagedProbeExit::Backend(error) => {
            json!({"kind": "memory_backend", "detail": format!("{error:?}"), "memory_error": memory_error.map(|memory_error| format!("{memory_error:?}"))})
        }
    }
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema_version": 1,
        "profile": "Stage4FlatProbeV1",
        "status": status,
        "exit": status,
        "retired": Value::Null,
        "rip": Value::Null,
        "activity": Value::Null,
        "rflags_defined": Value::Null,
        "serial_bytes": [],
        "observed_data_qword": Value::Null,
        "error": {"kind": kind, "detail": detail},
    })
}

fn emit(result: Value, code: u8) -> ExitCode {
    println!("{result}");
    ExitCode::from(code)
}
