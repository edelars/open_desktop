use od_isa::Register;
use od_machine::{
    AccessWidth, GuestPhysicalAddress, PagedProbeExit, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    load_stage4_alias_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

pub fn run(args: &[String]) -> ExitCode {
    let (path, budget, break_alias) = match parse(args) {
        Ok(value) => value,
        Err(e) => return emit(failure("arguments", e), 3),
    };
    let mut image = Vec::new();
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => return emit(failure("file", e.to_string()), 3),
    };
    if let Err(e) = file
        .take((STAGE4_FLAT_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
    {
        return emit(failure("file", e.to_string()), 3);
    }
    if image.len() != STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return emit(
            failure(
                "image_size",
                format!(
                    "expected {}, got {}",
                    STAGE4_FLAT_PROBE_IMAGE_BYTES,
                    image.len()
                ),
            ),
            3,
        );
    }
    let mut machine = match load_stage4_alias_probe(&image) {
        Ok(m) => m,
        Err(e) => return emit(failure("loader", format!("{e:?}")), 4),
    };
    let expected_store =
        u64::from_le_bytes(image[0x8008..0x8010].try_into().expect("fixed store value"));
    if break_alias {
        machine
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(0x6000),
                AccessWidth::Qword,
                (1_u64 << 63) | 0xa003,
                |_| {},
            )
            .expect("validated page-table RAM");
    }
    let mut write_trace = Vec::new();
    let report = machine.run_stack(
        budget,
        || false,
        |range| {
            write_trace
                .push(json!({"gpa":format!("0x{:x}",range.start().get()),"length":range.len()}))
        },
    );
    let uart = machine.uart();
    let mut serial = Vec::new();
    while uart.transfer_holding_to_shift() {
        if let Some(byte) = uart.complete_transmission() {
            serial.push(byte);
        }
    }
    let observed = machine
        .bus_mut()
        .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
        .ok();
    let s = machine.state();
    let exit = match report.exit {
        PagedProbeExit::Halted => "halted",
        PagedProbeExit::BudgetExhausted => "budget_exhausted",
        PagedProbeExit::Stopped => "stopped",
        PagedProbeExit::Rejected(_) => "rejected",
        PagedProbeExit::Unsupported { .. } => "unsupported",
        PagedProbeExit::Fault(_) => "fault",
        PagedProbeExit::Backend(_) => "backend_error",
        PagedProbeExit::PortIo(_) => "pio_error",
    };
    let rax = s.gpr[Register::Rax as usize];
    let complete = matches!(report.exit, PagedProbeExit::Halted);
    let ok = complete
        && observed == Some(expected_store)
        && rax == expected_store
        && report.instructions_executed == 4;
    emit(
        json!({"schema_version":1,"profile":"Stage4AliasProbeV1","status":if ok{"completed"}else if complete{"rejected"}else{"error"},"accepted":ok,"alias_verified":ok,"expected_store_qword":format!("0x{expected_store:016x}"),"exit":exit,"retired":report.instructions_executed,"rip":format!("0x{:016x}",s.rip),"activity":s.activity.map(|a|format!("{a:?}")),"serial_bytes":serial,"observed_alias_qword":observed.map(|v|format!("0x{v:016x}")),"observed_rax":format!("0x{rax:016x}"),"observed_rsp":format!("0x{:016x}",s.gpr[Register::Rsp as usize]),"committed_write_trace":write_trace,"error":if ok{Value::Null}else{json!({"kind":if complete{"alias_result_mismatch"}else{exit},"detail":format!("{report:?}")})}}),
        if ok {
            0
        } else if complete {
            7
        } else {
            5
        },
    )
}

fn parse(args: &[String]) -> Result<(String, u64, bool), String> {
    let mut p = None;
    let mut b = None;
    let mut break_alias = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--stage4-alias-probe" if p.is_none() => {
                p = Some(args.get(i + 1).ok_or("missing image path")?.clone());
                i += 2
            }
            "--budget" if b.is_none() => {
                let n = args
                    .get(i + 1)
                    .ok_or("missing budget")?
                    .parse()
                    .map_err(|_| "invalid budget")?;
                if n > 1024 {
                    return Err("budget must be between 0 and 1024".into());
                }
                b = Some(n);
                i += 2
            }
            "--budget" => return Err("--budget may be specified only once".into()),
            "--inject-break-alias" if !break_alias => {
                break_alias = true;
                i += 1;
            }
            x => return Err(format!("unexpected option {x}")),
        }
    }
    Ok((
        p.ok_or("--stage4-alias-probe IMAGE is required")?,
        b.unwrap_or(16),
        break_alias,
    ))
}
fn failure(kind: &str, detail: String) -> Value {
    json!({"schema_version":1,"profile":"Stage4AliasProbeV1","status":"error","exit":"error","retired":null,"rip":null,"activity":null,"serial_bytes":[],"observed_alias_qword":null,"observed_rax":null,"observed_rsp":null,"error":{"kind":kind,"detail":detail}})
}
fn emit(v: Value, code: u8) -> ExitCode {
    println!("{v}");
    ExitCode::from(code)
}
