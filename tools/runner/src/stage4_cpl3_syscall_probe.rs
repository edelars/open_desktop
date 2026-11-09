//! S4-33b CLI: `Stage4Cpl3SyscallProbeV1` CPL3 SYSCALL -> kernel store -> SYSRETQ -> user load.
use od_isa::Register;
use od_isa::syscall::{SyscallInstruction, SyscallStepExit};
use od_machine::{
    STAGE4_CPL3_SYSCALL_FINAL_RIP, STAGE4_CPL3_SYSCALL_INITIAL_RSP,
    STAGE4_CPL3_SYSCALL_INSTRUCTIONS, STAGE4_FLAT_PROBE_IMAGE_BYTES, Stage4Cpl3SyscallInjection,
    Stage4Cpl3SyscallStop, load_stage4_cpl3_syscall_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4Cpl3SyscallProbeV1";

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
    let mut machine = match load_stage4_cpl3_syscall_probe(&image) {
        Ok(m) => m,
        Err(e) => return emit(failure("loader", format!("{e:?}")), 4),
    };
    machine.inject(injection);
    let initial = machine.initial_qword();
    let expected = machine.store_qword();
    let mut write_trace = Vec::new();
    let report = machine.run(budget, |range| {
        write_trace.push(json!({"gpa":format!("0x{:x}",range.start().get()),"length":range.len()}))
    });
    let observed = machine.data_qword();
    let cpl = machine.cpl();
    let s = machine.state();
    let rax = s.gpr[Register::Rax as usize];
    let rsp = s.gpr[Register::Rsp as usize];
    let (stop_reason, stop_detail) = stop_reason(&report.stop);
    let complete = report.stop == Stage4Cpl3SyscallStop::Completed;
    let checks = [
        ("roundtrip_complete", complete),
        (
            "retired_all_instructions",
            report.retired == STAGE4_CPL3_SYSCALL_INSTRUCTIONS,
        ),
        ("kernel_qword_written", observed == Some(expected)),
        ("user_load_observed_kernel_qword", rax == expected),
        ("returned_to_cpl3", cpl == Some(3)),
        ("final_rip", s.rip == STAGE4_CPL3_SYSCALL_FINAL_RIP),
        ("rsp_preserved", rsp == STAGE4_CPL3_SYSCALL_INITIAL_RSP),
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
        .map(|(index, e)| {
            let (exit, detail) = step_exit(&e.report.cpu.exit);
            json!({
                "step": index + 1,
                "instruction": instruction(e.instruction),
                "exit": exit,
                "detail": detail,
                "rip_after": format!("0x{:016x}", e.rip_after),
                "cs_after": e.cs_after.map(|v| format!("0x{v:x}")),
                "cpl_after": e.cpl_after,
                "committed_writes": e.report.committed_writes.iter().map(|r| json!({
                    "gpa": format!("0x{:x}", r.start().get()),
                    "length": r.len(),
                })).collect::<Vec<_>>(),
            })
        })
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
            "retired": report.retired,
            "expected_instructions": STAGE4_CPL3_SYSCALL_INSTRUCTIONS,
            "initial_qword": format!("0x{initial:016x}"),
            "expected_store_qword": format!("0x{expected:016x}"),
            "observed_kernel_qword": observed.map(|v| format!("0x{v:016x}")),
            "guest_state": {
                "rip": format!("0x{:016x}", s.rip),
                "rsp": format!("0x{rsp:016x}"),
                "cpl": cpl,
                "cs": s.cs_selector.map(|v| format!("0x{v:x}")),
                "ss": s.ss_selector.map(|v| format!("0x{v:x}")),
                "rax": format!("0x{rax:016x}"),
                "rcx": format!("0x{:016x}", s.gpr[Register::Rcx as usize]),
                "r11": format!("0x{:016x}", s.gpr[Register::R11 as usize]),
                "rflags": format!("0x{:x}", s.rflags),
                "cr3": s.cr3.map(|v| format!("0x{v:x}")),
            },
            "event_trace": events,
            "committed_write_trace": write_trace,
            "checks": checks.iter().map(|&(n, p)| (n.to_string(), Value::Bool(p))).collect::<serde_json::Map<_, _>>(),
            "failed_checks": failed,
            "error": if ok { Value::Null } else { json!({
                "kind": if complete { "cpl3_syscall_result_mismatch" } else { stop_reason },
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

fn instruction(i: SyscallInstruction) -> &'static str {
    match i {
        SyscallInstruction::Syscall => "SYSCALL",
        SyscallInstruction::KernelStore => "MOV [RDI],R10",
        SyscallInstruction::Sysret => "SYSRETQ",
        SyscallInstruction::UserLoad => "MOV RAX,[RBX]",
    }
}

fn step_exit(exit: &SyscallStepExit) -> (&'static str, Option<String>) {
    match exit {
        SyscallStepExit::Retired => ("retired", None),
        SyscallStepExit::Unsupported => ("unsupported", None),
        SyscallStepExit::Rejected => ("rejected", None),
        SyscallStepExit::Fault(f) => ("fault", Some(format!("{f:?}"))),
        SyscallStepExit::Backend(e) => ("backend_error", Some(format!("{e:?}"))),
    }
}

fn stop_reason(stop: &Stage4Cpl3SyscallStop) -> (&'static str, Option<String>) {
    match stop {
        Stage4Cpl3SyscallStop::Completed => ("roundtrip_complete", None),
        Stage4Cpl3SyscallStop::BudgetExhausted => ("budget_exhausted", None),
        Stage4Cpl3SyscallStop::Step(exit) => step_exit(exit),
        Stage4Cpl3SyscallStop::Error(e) => ("integration_error", Some(format!("{e:?}"))),
    }
}

fn injection_name(i: Stage4Cpl3SyscallInjection) -> &'static str {
    match i {
        Stage4Cpl3SyscallInjection::None => "none",
        Stage4Cpl3SyscallInjection::KernelStoreMisdirected => "kernel_no_write",
        Stage4Cpl3SyscallInjection::SysretStaysCpl0 => "sysret_cpl0",
    }
}

fn parse(args: &[String]) -> Result<(String, u64, Stage4Cpl3SyscallInjection), String> {
    let mut p = None;
    let mut b = None;
    let mut injection = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--stage4-cpl3-syscall-probe" if p.is_none() => {
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
            flag @ ("--inject-kernel-no-write" | "--inject-sysret-cpl0") => {
                if injection.is_some() {
                    return Err("only one injection may be specified".into());
                }
                injection = Some(if flag == "--inject-kernel-no-write" {
                    Stage4Cpl3SyscallInjection::KernelStoreMisdirected
                } else {
                    Stage4Cpl3SyscallInjection::SysretStaysCpl0
                });
                i += 1;
            }
            x => return Err(format!("unexpected option {x}")),
        }
    }
    Ok((
        p.ok_or("--stage4-cpl3-syscall-probe IMAGE is required")?,
        b.unwrap_or(16),
        injection.unwrap_or(Stage4Cpl3SyscallInjection::None),
    ))
}

fn failure(kind: &str, detail: String) -> Value {
    json!({"schema_version":1,"profile":PROFILE,"status":"error","accepted":false,"stop_reason":"error","retired":null,"guest_state":null,"event_trace":[],"error":{"kind":kind,"detail":detail}})
}

fn emit(v: Value, code: u8) -> ExitCode {
    println!("{v}");
    ExitCode::from(code)
}
