//! S4-33c CLI: `Stage4PfRepairProbeV1` CR0.WP fault -> #PF delivery -> PTE repair -> INVLPG ->
//! LEA -> IRETQ -> retried store -> load.
use od_isa::Register;
use od_machine::{
    STAGE4_PF_REPAIR_DATA_GVA, STAGE4_PF_REPAIR_FINAL_RIP, STAGE4_PF_REPAIR_HANDLER_GVA,
    STAGE4_PF_REPAIR_IMAGE_BYTES, STAGE4_PF_REPAIR_INITIAL_RSP, STAGE4_PF_REPAIR_INSTRUCTIONS,
    STAGE4_PF_REPAIR_MAIN_GVA, STAGE4_PF_REPAIR_PTE_RO_ACCESSED, STAGE4_PF_REPAIR_PTE_RW,
    STAGE4_PF_REPAIR_PTE_RW_DIRTY, STAGE4_PF_REPAIR_STEPS, Stage4PfRepairEvent,
    Stage4PfRepairInjection, Stage4PfRepairStep, Stage4PfRepairStop, load_stage4_pf_repair_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4PfRepairProbeV1";
const RF: u64 = 1 << 16;

pub fn run(args: &[String]) -> ExitCode {
    let (path, budget, injection) = match parse(args) {
        Ok(value) => value,
        Err(e) => return emit(failure("arguments", e), 3),
    };
    let mut image = Vec::new();
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => return emit(failure("file", e.to_string()), 3),
    };
    if let Err(e) = file
        .take((STAGE4_PF_REPAIR_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
    {
        return emit(failure("file", e.to_string()), 3);
    }
    if image.len() != STAGE4_PF_REPAIR_IMAGE_BYTES {
        return emit(
            failure(
                "image_size",
                format!(
                    "expected {}, got {}",
                    STAGE4_PF_REPAIR_IMAGE_BYTES,
                    image.len()
                ),
            ),
            3,
        );
    }
    let mut machine = match load_stage4_pf_repair_probe(&image) {
        Ok(m) => m,
        Err(e) => return emit(failure("loader", format!("{e:?}")), 4),
    };
    machine.inject(injection);
    let initial = machine.initial_qword();
    let canary = machine.canary_qword();
    let store = machine.store_qword();
    let mut write_trace = Vec::new();
    let report = machine.run(budget, |range| {
        write_trace.push(json!({"gpa":format!("0x{:x}",range.start().get()),"length":range.len()}))
    });
    let data = machine.data_qword();
    let observed_canary = machine.canary();
    let pte = machine.pte();
    let s = machine.state();
    let rax = s.gpr[Register::Rax as usize];
    let rsp = s.gpr[Register::Rsp as usize];
    let (stop_reason, stop_detail) = stop_reason(&report.stop);
    let complete = report.stop == Stage4PfRepairStop::Completed;
    let event = |step| report.events.iter().find(|e| e.step == step);
    let fault_ok = event(Stage4PfRepairStep::Fault).is_some_and(|e| {
        e.page_fault.is_some_and(|f| {
            f.vector == 14
                && f.error_code == Some(3)
                && f.cr2 == STAGE4_PF_REPAIR_DATA_GVA
                && f.saved_rip == STAGE4_PF_REPAIR_MAIN_GVA
        }) && e.data_qword == initial
    });
    let checks = [
        ("case_complete", complete),
        (
            "all_steps_executed",
            report.steps == STAGE4_PF_REPAIR_STEPS
                && report.retired == STAGE4_PF_REPAIR_INSTRUCTIONS,
        ),
        ("fault_raised_on_readonly_store", fault_ok),
        (
            "delivered_to_handler",
            event(Stage4PfRepairStep::Delivery).is_some_and(|e| {
                e.rip == STAGE4_PF_REPAIR_HANDLER_GVA && e.error_code_on_stack == Some(3)
            }),
        ),
        (
            "pte_repaired_to_rw",
            event(Stage4PfRepairStep::Repair).is_some_and(|e| e.pte == STAGE4_PF_REPAIR_PTE_RW),
        ),
        (
            "error_code_discarded_before_iretq",
            event(Stage4PfRepairStep::Iretq)
                .is_some_and(|e| e.iretq_frame_rip == Some(STAGE4_PF_REPAIR_MAIN_GVA)),
        ),
        (
            "iretq_returned_to_fault_rip",
            event(Stage4PfRepairStep::Iretq).is_some_and(|e| {
                e.retired
                    && e.rip == STAGE4_PF_REPAIR_MAIN_GVA
                    && e.rsp == STAGE4_PF_REPAIR_INITIAL_RSP
                    && e.rflags & RF != 0
            }),
        ),
        (
            "retry_store_committed",
            data == store
                && event(Stage4PfRepairStep::Retry)
                    .is_some_and(|e| e.retired && e.data_qword == store && e.rflags & RF == 0),
        ),
        ("load_observed_stored_value", rax == store),
        ("canary_intact", observed_canary == canary),
        ("pte_accessed_dirty", pte == STAGE4_PF_REPAIR_PTE_RW_DIRTY),
        (
            "no_second_fault",
            report.events.iter().skip(1).all(|e| e.page_fault.is_none())
                && !matches!(report.stop, Stage4PfRepairStop::SecondPageFault(_)),
        ),
        ("final_rip", s.rip == STAGE4_PF_REPAIR_FINAL_RIP),
        ("rsp_preserved", rsp == STAGE4_PF_REPAIR_INITIAL_RSP),
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
            "injection": injection_name(injection),
            "budget": budget,
            "stop_reason": stop_reason,
            "stop_detail": stop_detail,
            "steps": report.steps,
            "retired": report.retired,
            "expected_steps": STAGE4_PF_REPAIR_STEPS,
            "expected_instructions": STAGE4_PF_REPAIR_INSTRUCTIONS,
            "initial_qword": format!("0x{initial:016x}"),
            "canary_qword": format!("0x{canary:016x}"),
            "expected_store_qword": format!("0x{store:016x}"),
            "observed_data_qword": format!("0x{data:016x}"),
            "observed_pte": format!("0x{pte:x}"),
            "expected_pte_ro_accessed": format!("0x{STAGE4_PF_REPAIR_PTE_RO_ACCESSED:x}"),
            "expected_pte_final": format!("0x{STAGE4_PF_REPAIR_PTE_RW_DIRTY:x}"),
            "guest_state": {
                "rip": format!("0x{:016x}", s.rip),
                "rsp": format!("0x{rsp:016x}"),
                "rflags": format!("0x{:x}", s.rflags),
                "rf": s.rflags & RF != 0,
                "cr2": s.cr2.map(|v| format!("0x{v:x}")),
                "cr3": s.cr3.map(|v| format!("0x{v:x}")),
                "rax": format!("0x{rax:016x}"),
                "rcx": format!("0x{:016x}", s.gpr[Register::Rcx as usize]),
                "rdi": format!("0x{:016x}", s.gpr[Register::Rdi as usize]),
                "rsi": format!("0x{:016x}", s.gpr[Register::Rsi as usize]),
                "rbx": format!("0x{:016x}", s.gpr[Register::Rbx as usize]),
            },
            "event_trace": events,
            "committed_write_trace": write_trace,
            "checks": checks.iter().map(|&(n, p)| (n.to_string(), Value::Bool(p))).collect::<serde_json::Map<_, _>>(),
            "failed_checks": failed,
            "error": if ok { Value::Null } else { json!({
                "kind": if complete { "pf_repair_result_mismatch" } else { stop_reason },
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

fn event_json(index: usize, e: &Stage4PfRepairEvent) -> Value {
    json!({
        "step": index + 1,
        "name": step_name(e.step),
        "retired": e.retired,
        "exit": e.exit,
        "detail": e.detail,
        "rip_after": format!("0x{:016x}", e.rip),
        "rsp_after": format!("0x{:016x}", e.rsp),
        "rflags_after": hex(e.rflags),
        "rf_after": e.rflags & RF != 0,
        "cr2_after": e.cr2.map(hex),
        "rax_after": format!("0x{:016x}", e.rax),
        "pte_after": hex(e.pte),
        "data_qword_after": format!("0x{:016x}", e.data_qword),
        "canary_qword_after": format!("0x{:016x}", e.canary_qword),
        "page_fault": e.page_fault.map(|f| json!({
            "vector": f.vector,
            "error_code": f.error_code,
            "cr2": hex(f.cr2),
            "saved_rip": hex(f.saved_rip),
        })),
        "error_code_on_stack": e.error_code_on_stack,
        "iretq_frame_rip": e.iretq_frame_rip.map(hex),
        "ad_updates": e.ad_updates.iter().map(|u| json!({
            "gpa": hex(u.address.get()),
            "level": format!("{:?}", u.level),
            "bit": format!("{:?}", u.bit),
            "before": hex(u.before),
            "after": hex(u.after),
        })).collect::<Vec<_>>(),
        "committed_writes": e.committed_writes.iter().map(|r| json!({
            "gpa": hex(r.start().get()),
            "length": r.len(),
        })).collect::<Vec<_>>(),
    })
}

fn step_name(step: Stage4PfRepairStep) -> &'static str {
    match step {
        Stage4PfRepairStep::Fault => "fault: MOV [RBX],RCX",
        Stage4PfRepairStep::Delivery => "delivery: #PF through IDT gate 14",
        Stage4PfRepairStep::Repair => "repair: MOV [RDI],RSI",
        Stage4PfRepairStep::Invlpg => "INVLPG [RBX]",
        Stage4PfRepairStep::Discard => "discard: LEA RSP,[RSP+8]",
        Stage4PfRepairStep::Iretq => "IRETQ",
        Stage4PfRepairStep::Retry => "retry: MOV [RBX],RCX",
        Stage4PfRepairStep::Load => "load: MOV RAX,[RBX]",
    }
}

fn stop_reason(stop: &Stage4PfRepairStop) -> (&'static str, Option<String>) {
    match stop {
        Stage4PfRepairStop::Completed => ("case_complete", None),
        Stage4PfRepairStop::BudgetExhausted => ("budget_exhausted", None),
        Stage4PfRepairStop::NoPageFault => ("no_page_fault", None),
        Stage4PfRepairStop::SecondPageFault(step) => {
            ("second_page_fault", Some(step_name(*step).to_string()))
        }
        Stage4PfRepairStop::DeliveryFailed(outcome) => {
            ("delivery_failed", Some(format!("{outcome:?}")))
        }
        Stage4PfRepairStop::Step(step, exit) => (exit, Some(step_name(*step).to_string())),
        Stage4PfRepairStop::Error(e) => ("integration_error", Some(e.clone())),
    }
}

fn injection_name(i: Stage4PfRepairInjection) -> &'static str {
    match i {
        Stage4PfRepairInjection::None => "none",
        Stage4PfRepairInjection::HandlerKeepsPteReadOnly => "pte_stays_readonly",
        Stage4PfRepairInjection::HandlerSkipsLea => "skip_lea",
        Stage4PfRepairInjection::LoaderMapsPageWritable => "pte_writable_at_load",
    }
}

fn parse(args: &[String]) -> Result<(String, u64, Stage4PfRepairInjection), String> {
    let mut p = None;
    let mut b = None;
    let mut injection = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--stage4-pf-repair-probe" if p.is_none() => {
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
            flag @ ("--inject-pte-readonly" | "--inject-skip-lea" | "--inject-pte-writable") => {
                if injection.is_some() {
                    return Err("only one injection may be specified".into());
                }
                injection = Some(match flag {
                    "--inject-pte-readonly" => Stage4PfRepairInjection::HandlerKeepsPteReadOnly,
                    "--inject-skip-lea" => Stage4PfRepairInjection::HandlerSkipsLea,
                    _ => Stage4PfRepairInjection::LoaderMapsPageWritable,
                });
                i += 1;
            }
            x => return Err(format!("unexpected option {x}")),
        }
    }
    Ok((
        p.ok_or("--stage4-pf-repair-probe IMAGE is required")?,
        b.unwrap_or(16),
        injection.unwrap_or(Stage4PfRepairInjection::None),
    ))
}

fn failure(kind: &str, detail: String) -> Value {
    json!({"schema_version":1,"profile":PROFILE,"status":"error","accepted":false,"stop_reason":"error","retired":null,"guest_state":null,"event_trace":[],"error":{"kind":kind,"detail":detail}})
}

fn emit(v: Value, code: u8) -> ExitCode {
    println!("{v}");
    ExitCode::from(code)
}
