//! S4-33d CLI: `Stage4CrossPageProbeV1` reads across data pages and a fetch across code frames.
use od_isa::Register;
use od_machine::{
    STAGE4_CROSS_PAGE_FINAL_RIP, STAGE4_CROSS_PAGE_IMAGE_BYTES, STAGE4_CROSS_PAGE_INITIAL_RAX,
    STAGE4_CROSS_PAGE_INSTRUCTIONS, STAGE4_CROSS_PAGE_PTES, STAGE4_CROSS_PAGE_STEP_RIPS,
    STAGE4_CROSS_PAGE_STEPS, Stage4CrossPageEvent, Stage4CrossPageInjection,
    Stage4CrossPageOperand, Stage4CrossPageStep, Stage4CrossPageStop, load_stage4_cross_page_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4CrossPageProbeV1";
const A: u64 = 1 << 5;
const D: u64 = 1 << 6;
const PTE_NAMES: [&str; 4] = ["code0", "code1", "data0", "data1"];

struct Options {
    path: String,
    budget: u64,
    injection: Stage4CrossPageInjection,
    operand: Stage4CrossPageOperand,
}

pub fn run(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(value) => value,
        Err(e) => return emit(failure("arguments", e), 3),
    };
    let mut image = Vec::new();
    let file = match File::open(&options.path) {
        Ok(f) => f,
        Err(e) => return emit(failure("file", e.to_string()), 3),
    };
    if let Err(e) = file
        .take((STAGE4_CROSS_PAGE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
    {
        return emit(failure("file", e.to_string()), 3);
    }
    if image.len() != STAGE4_CROSS_PAGE_IMAGE_BYTES {
        return emit(
            failure(
                "image_size",
                format!(
                    "expected {}, got {}",
                    STAGE4_CROSS_PAGE_IMAGE_BYTES,
                    image.len()
                ),
            ),
            3,
        );
    }
    let mut machine = match load_stage4_cross_page_probe(&image) {
        Ok(m) => m,
        Err(e) => return emit(failure("loader", format!("{e:?}")), 4),
    };
    machine.select_operand(options.operand);
    machine.inject(options.injection);
    let (cpl, ac, nxe, efer) = (machine.cpl(), machine.ac(), machine.nxe(), machine.efer());
    let value = machine.value();
    let mut write_trace = Vec::new();
    let report = machine.run(options.budget, |range| {
        write_trace.push(json!({"gpa":format!("0x{:x}",range.start().get()),"length":range.len()}))
    });
    let ptes = machine.ptes();
    let s = machine.state();
    let rax = s.gpr[Register::Rax as usize];
    let (stop_reason, stop_detail) = stop_reason(&report.stop);
    let complete = report.stop == Stage4CrossPageStop::Completed;

    // Expected values come from the architecture: little-endian bytes b0..b7 of the value, a word
    // read replaces AX only, a dword read zero-extends, a qword read replaces RAX.
    let word = value & 0xffff;
    let dword = value & 0xffff_ffff;
    let expected_rax = [
        (STAGE4_CROSS_PAGE_INITIAL_RAX & !0xffff) | word,
        value,
        dword,
        dword,
        dword,
        dword,
    ];
    let crossing = options.operand == Stage4CrossPageOperand::CrossPage;
    // Pages walked (A set) after each step. Step 1 fetches both code frames and reads data page 0;
    // step 2 is the first read that reaches data page 1, unless the control keeps all in page 0.
    let accessed_after = |i: usize| [true, true, true, i >= 1 && crossing];
    let event_ok = |i: usize, e: &Stage4CrossPageEvent| {
        let accessed = accessed_after(i);
        e.retired
            && e.rip
                == [
                    0x40_1001, 0x40_1004, 0x40_1006, 0x40_1009, 0x40_100a, 0x40_100b,
                ][i]
            && e.rax == expected_rax[i]
            && e.cpl == 0
            && e.page_fault.is_none()
            && e.ptes.iter().enumerate().all(|(k, &pte)| {
                pte & D == 0
                    && (pte & A != 0) == accessed[k]
                    && pte & !(A | D) == STAGE4_CROSS_PAGE_PTES[k]
            })
    };
    let steps_ok = report.events.len() == STAGE4_CROSS_PAGE_STEPS as usize
        && report
            .events
            .iter()
            .enumerate()
            .all(|(i, e)| event_ok(i, e));
    let checks = [
        ("case_complete", complete),
        (
            "all_steps_executed",
            report.steps == STAGE4_CROSS_PAGE_STEPS
                && report.retired == STAGE4_CROSS_PAGE_INSTRUCTIONS,
        ),
        ("handoff_cpl0_ac_clear_nxe_set", cpl == 0 && !ac && nxe),
        ("per_step_rip_rax_and_accessed_bits", steps_ok),
        (
            "rax_follows_architecture_per_step",
            report.events.len() == expected_rax.len()
                && report
                    .events
                    .iter()
                    .zip(expected_rax)
                    .all(|(e, expected)| e.rax == expected),
        ),
        (
            "second_data_page_walked_only_when_crossing",
            (ptes[3] & A != 0) == crossing,
        ),
        ("no_dirty_bits", ptes.iter().all(|p| p & D == 0)),
        (
            "no_page_fault_or_backend_error",
            report
                .events
                .iter()
                .all(|e| e.page_fault.is_none() && e.memory_error.is_none()),
        ),
        (
            "uart_byte_is_low_byte_of_value",
            report.serial == [(value & 0xff) as u8],
        ),
        ("final_rax", rax == dword),
        ("final_rip", s.rip == STAGE4_CROSS_PAGE_FINAL_RIP),
        ("halted", s.activity == Some(od_isa::CpuActivity::Halted)),
    ];
    let failed: Vec<&str> = checks
        .iter()
        .filter_map(|&(name, pass)| (!pass).then_some(name))
        .collect();
    let ok = failed.is_empty();
    let events: Vec<Value> = report
        .events
        .iter()
        .enumerate()
        .map(|(index, e)| event_json(index, e))
        .collect();
    emit(
        json!({
            "schema_version": 1,
            "profile": PROFILE,
            "status": if ok { "completed" } else if complete { "rejected" } else { "error" },
            "accepted": ok,
            "injection": injection_name(options.injection),
            "operand": if crossing { "cross_page" } else { "same_page_control" },
            "budget": options.budget,
            "stop_reason": stop_reason,
            "stop_detail": stop_detail,
            "steps": report.steps,
            "retired": report.retired,
            "expected_steps": STAGE4_CROSS_PAGE_STEPS,
            "expected_instructions": STAGE4_CROSS_PAGE_INSTRUCTIONS,
            "value_qword": format!("0x{value:016x}"),
            "first_page_bytes": hex_bytes(&value.to_le_bytes()[..3]),
            "second_page_bytes": hex_bytes(&value.to_le_bytes()[3..]),
            "expected_rax_per_step": expected_rax.iter().map(|v| format!("0x{v:016x}")).collect::<Vec<_>>(),
            "expected_step_rips": STAGE4_CROSS_PAGE_STEP_RIPS.iter().map(|v| format!("0x{v:x}")).collect::<Vec<_>>(),
            "handoff": {
                "cpl": cpl,
                "rflags_ac": ac,
                "efer": format!("0x{efer:x}"),
                "efer_nxe": nxe,
                "initial_rax": format!("0x{STAGE4_CROSS_PAGE_INITIAL_RAX:016x}"),
            },
            "guest_state": {
                "rip": format!("0x{:016x}", s.rip),
                "rflags": format!("0x{:x}", s.rflags),
                "cr2": s.cr2.map(|v| format!("0x{v:x}")),
                "cr3": s.cr3.map(|v| format!("0x{v:x}")),
                "rax": format!("0x{rax:016x}"),
                "rbx": format!("0x{:016x}", s.gpr[Register::Rbx as usize]),
                "rdx": format!("0x{:016x}", s.gpr[Register::Rdx as usize]),
                "activity": s.activity.map(|a| format!("{a:?}")),
                "cpl": cpl,
            },
            "observed_ptes": pte_json(&ptes),
            "serial_bytes": report.serial,
            "event_trace": events,
            "committed_write_trace": write_trace,
            "checks": checks.iter().map(|&(n, p)| (n.to_string(), Value::Bool(p))).collect::<serde_json::Map<_, _>>(),
            "failed_checks": failed,
            "error": if ok { Value::Null } else { json!({
                "kind": if complete { "cross_page_result_mismatch" } else { stop_reason },
                "failed_checks": failed,
            }) },
        }),
        if ok {
            0
        } else if complete {
            7
        } else {
            5
        },
    )
}

fn hex(v: u64) -> String {
    format!("0x{v:x}")
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn pte_json(ptes: &[u64; 4]) -> Value {
    let mut map = serde_json::Map::new();
    for (name, &pte) in PTE_NAMES.iter().zip(ptes) {
        map.insert(
            (*name).to_string(),
            json!({
                "pte": format!("0x{pte:016x}"),
                "present": pte & 1 != 0,
                "accessed": pte & A != 0,
                "dirty": pte & D != 0,
                "nx": pte >> 63 != 0,
            }),
        );
    }
    Value::Object(map)
}

fn event_json(index: usize, e: &Stage4CrossPageEvent) -> Value {
    json!({
        "step": index + 1,
        "name": step_name(e.step),
        "retired": e.retired,
        "exit": e.exit,
        "detail": e.detail,
        "rip_after": format!("0x{:016x}", e.rip),
        "rax_after": format!("0x{:016x}", e.rax),
        "cpl": e.cpl,
        "cr2_after": e.cr2.map(hex),
        "page_fault": e.page_fault.as_ref().map(|f| json!({
            "vector": f.vector,
            "error_code": f.error_code,
            "error_code_hex": f.error_code.map(|c| format!("0x{c:02x}")),
            "cr2": hex(f.cr2),
            "saved_rip": hex(f.saved_rip),
            "access": f.access,
        })),
        "memory_error": e.memory_error,
        "pte_after": pte_json(&e.ptes),
        "committed_writes": e.committed_writes.iter().map(|r| json!({
            "gpa": hex(r.start().get()),
            "length": r.len(),
        })).collect::<Vec<_>>(),
    })
}

fn step_name(step: Stage4CrossPageStep) -> &'static str {
    match step {
        Stage4CrossPageStep::WordFetchCross => "word read with fetch across code frames: 66 8B 03",
        Stage4CrossPageStep::QwordRead => "qword read: 48 8B 03",
        Stage4CrossPageStep::DwordRead => "dword read: 8B 03",
        Stage4CrossPageStep::WordRead => "word read: 66 8B 03",
        Stage4CrossPageStep::Out => "OUT DX,AL",
        Stage4CrossPageStep::Halt => "HLT",
    }
}

fn stop_reason(stop: &Stage4CrossPageStop) -> (&'static str, Option<String>) {
    match stop {
        Stage4CrossPageStop::Completed => ("case_complete", None),
        Stage4CrossPageStop::BudgetExhausted => ("budget_exhausted", None),
        Stage4CrossPageStop::PageFault(step) => ("page_fault", Some(step_name(*step).to_string())),
        Stage4CrossPageStop::Backend(step) => {
            ("backend_rejected", Some(step_name(*step).to_string()))
        }
        Stage4CrossPageStop::Step(step, exit) => (exit, Some(step_name(*step).to_string())),
        Stage4CrossPageStop::Error(e) => ("integration_error", Some(e.clone())),
    }
}

fn injection_name(i: Stage4CrossPageInjection) -> &'static str {
    match i {
        Stage4CrossPageInjection::None => "none",
        Stage4CrossPageInjection::SecondDataPteNotPresent => "second_data_pte_not_present",
        Stage4CrossPageInjection::SecondCodePteNx => "second_code_pte_nx",
        Stage4CrossPageInjection::DataFramesSwapped => "data_frames_swapped",
        Stage4CrossPageInjection::SecondDataPageHole => "second_data_page_hole",
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut path = None;
    let mut budget = None;
    let mut injection = None;
    let mut operand = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--stage4-cross-page-probe" if path.is_none() => {
                path = Some(args.get(i + 1).ok_or("missing image path")?.clone());
                i += 2
            }
            "--budget" if budget.is_none() => {
                let n = args
                    .get(i + 1)
                    .ok_or("missing budget")?
                    .parse()
                    .map_err(|_| "invalid budget")?;
                if n > 1024 {
                    return Err("budget must be between 0 and 1024".into());
                }
                budget = Some(n);
                i += 2
            }
            "--budget" => return Err("--budget may be specified only once".into()),
            "--same-page-control" if operand.is_none() => {
                operand = Some(Stage4CrossPageOperand::SamePageControl);
                i += 1;
            }
            "--same-page-control" => {
                return Err("--same-page-control may be specified only once".into());
            }
            flag @ ("--inject-data-pte-not-present"
            | "--inject-code-pte-nx"
            | "--inject-swap-data-frames"
            | "--inject-data-hole") => {
                if injection.is_some() {
                    return Err("only one injection may be specified".into());
                }
                injection = Some(match flag {
                    "--inject-data-pte-not-present" => {
                        Stage4CrossPageInjection::SecondDataPteNotPresent
                    }
                    "--inject-code-pte-nx" => Stage4CrossPageInjection::SecondCodePteNx,
                    "--inject-swap-data-frames" => Stage4CrossPageInjection::DataFramesSwapped,
                    _ => Stage4CrossPageInjection::SecondDataPageHole,
                });
                i += 1;
            }
            x => return Err(format!("unexpected option {x}")),
        }
    }
    Ok(Options {
        path: path.ok_or("--stage4-cross-page-probe IMAGE is required")?,
        budget: budget.unwrap_or(16),
        injection: injection.unwrap_or(Stage4CrossPageInjection::None),
        operand: operand.unwrap_or(Stage4CrossPageOperand::CrossPage),
    })
}

fn failure(kind: &str, detail: String) -> Value {
    json!({"schema_version":1,"profile":PROFILE,"status":"error","accepted":false,"stop_reason":"error","retired":null,"guest_state":null,"event_trace":[],"error":{"kind":kind,"detail":detail}})
}

fn emit(v: Value, code: u8) -> ExitCode {
    println!("{v}");
    ExitCode::from(code)
}
