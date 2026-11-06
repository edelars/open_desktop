use od_machine::{
    AccessWidth, GuestPhysicalAddress, PagedProbeExit, STAGE4_FLAT_PROBE_DATA_GVA,
    STAGE4_FLAT_PROBE_IMAGE_BYTES, load_stage4_store_load_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

pub fn run(args: &[String]) -> ExitCode {
    let (path, budget) = match parse(args) {
        Ok(parsed) => parsed,
        Err(detail) => return emit(failure("invalid_input", "arguments", detail), 3),
    };
    let mut image = Vec::with_capacity(STAGE4_FLAT_PROBE_IMAGE_BYTES + 1);
    let opened = match File::open(&path) {
        Ok(file) => file,
        Err(error) => return emit(failure("invalid_input", "file", error.to_string()), 3),
    };
    if let Err(error) = opened
        .take((STAGE4_FLAT_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
    {
        return emit(failure("invalid_input", "file", error.to_string()), 3);
    }
    if image.len() != STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return emit(
            failure(
                "invalid_input",
                "image_size",
                format!(
                    "expected {STAGE4_FLAT_PROBE_IMAGE_BYTES} bytes, got {}",
                    image.len()
                ),
            ),
            3,
        );
    }
    let mut machine = match load_stage4_store_load_probe(&image) {
        Ok(machine) => machine,
        Err(error) => return emit(failure("loader_error", "loader", format!("{error:?}")), 4),
    };
    let report = machine.run(budget, || false, |_| {});
    let mut serial = Vec::new();
    let uart = machine.uart();
    while uart.transfer_holding_to_shift() {
        if let Some(byte) = uart.complete_transmission() {
            serial.push(byte);
        }
    }
    let value = machine
        .bus_mut()
        .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
        .ok();
    let state = machine.state();
    let exit = match report.exit {
        PagedProbeExit::Halted => "halted",
        PagedProbeExit::BudgetExhausted => "budget_exhausted",
        PagedProbeExit::Stopped => "stopped",
        PagedProbeExit::Rejected(_) => "rejected",
        PagedProbeExit::Unsupported { .. } => "unsupported",
        PagedProbeExit::Fault(_) => "fault",
        PagedProbeExit::Backend(_) => "backend_error",
        PagedProbeExit::PortIo(_) => "pio_error",
        PagedProbeExit::IntegrationError(_) => "integration_error",
    };
    let ok = matches!(report.exit, PagedProbeExit::Halted);
    let result = json!({
        "schema_version": 1,
        "profile": "Stage4StoreLoadProbeV1",
        "status": if ok {"completed"} else {"error"},
        "exit": exit,
        "retired": report.instructions_executed,
        "rip": format!("0x{:016x}", state.rip),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rflags_defined": format!("0x{:016x}", report.rflags_defined),
        "serial_bytes": serial,
        "observed_data_qword": value.map(|v| format!("0x{v:016x}")),
        "observed_rax": format!("0x{:016x}", state.gpr[od_isa::Register::Rax as usize]),
        "error": if ok { Value::Null } else { json!({"kind": exit, "detail": format!("{report:?}")}) },
        "data_gva": format!("0x{STAGE4_FLAT_PROBE_DATA_GVA:016x}"),
    });
    emit(
        result,
        if ok {
            0
        } else if matches!(report.exit, PagedProbeExit::PortIo(_)) {
            6
        } else {
            5
        },
    )
}

fn parse(args: &[String]) -> Result<(String, u64), String> {
    let mut path = None;
    let mut budget = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--stage4-store-load-probe" if path.is_none() => {
                path = Some(args.get(index + 1).ok_or("missing image path")?.clone());
                index += 2;
            }
            "--budget" if budget.is_none() => {
                let parsed = args
                    .get(index + 1)
                    .ok_or("missing budget")?
                    .parse()
                    .map_err(|_| "invalid budget")?;
                if parsed > 1024 {
                    return Err("budget must be between 0 and 1024".into());
                }
                budget = Some(parsed);
                index += 2;
            }
            "--budget" => return Err("--budget may be specified only once".into()),
            option => return Err(format!("unexpected or duplicate option {option}")),
        }
    }
    Ok((
        path.ok_or("--stage4-store-load-probe IMAGE is required")?,
        budget.unwrap_or(16),
    ))
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({"schema_version":1,"profile":"Stage4StoreLoadProbeV1","status":status,"exit":status,"retired":null,"rip":null,"activity":null,"rflags_defined":null,"serial_bytes":[],"observed_data_qword":null,"observed_rax":null,"error":{"kind":kind,"detail":detail}})
}

fn emit(value: Value, code: u8) -> ExitCode {
    println!("{value}");
    ExitCode::from(code)
}
