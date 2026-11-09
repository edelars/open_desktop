//! S4-27i4a CLI: `Stage4PagedStiHltProbeV1` (S4-27h layout, main `STI; HLT; HLT`): the exact paged
//! STI and HLT steps, a wake of the halted CPU by APIC vector 0x41 through the paged IDT, the
//! handler EOI store, IRETQ and the second HLT. Only the wake scenario runs here; the boundary
//! scenario is covered by the machine test.
use od_machine::{
    STAGE4_PAGED_IRQ_HANDLER_GVA, STAGE4_PAGED_IRQ_INITIAL_RSP, STAGE4_PAGED_IRQ_PTE_GDT,
    STAGE4_PAGED_IRQ_PTE_HANDLER, STAGE4_PAGED_IRQ_PTE_IDT, STAGE4_PAGED_IRQ_PTE_LAPIC,
    STAGE4_PAGED_IRQ_PTE_MAIN, STAGE4_PAGED_IRQ_PTE_STACK, STAGE4_PAGED_IRQ_VECTOR,
    STAGE4_PAGED_STI_HLT_FIRST_HLT_RIP, STAGE4_PAGED_STI_HLT_IMAGE_BYTES,
    STAGE4_PAGED_STI_HLT_SECOND_HLT_RIP, STAGE4_PAGED_STI_HLT_WAKE_FINAL_RIP,
    STAGE4_PAGED_STI_HLT_WAKE_INSTRUCTIONS, STAGE4_PAGED_STI_HLT_WAKE_STEPS, Stage4PagedIrqPtes,
    Stage4PagedStiHltEvent, Stage4PagedStiHltInjection, Stage4PagedStiHltScenario,
    Stage4PagedStiHltStep, Stage4PagedStiHltStop, load_stage4_paged_sti_hlt_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4PagedStiHltProbeV1";
const IF: u64 = 1 << 9;
/// RSP aligned down to 16 (0x700ff0) minus five qwords (SDM Vol. 3A 6.14.2).
const FRAME_RSP: u64 = 0x0070_0fc8;

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
        .take((STAGE4_PAGED_STI_HLT_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
    {
        return emit(failure("file", e.to_string()), 3);
    }
    if image.len() != STAGE4_PAGED_STI_HLT_IMAGE_BYTES {
        return emit(
            failure(
                "image_size",
                format!(
                    "expected {}, got {}",
                    STAGE4_PAGED_STI_HLT_IMAGE_BYTES,
                    image.len()
                ),
            ),
            3,
        );
    }
    let mut machine =
        match load_stage4_paged_sti_hlt_probe(&image, Stage4PagedStiHltScenario::WakeFromHlt) {
            Ok(m) => m,
            Err(e) => return emit(failure("loader", format!("{e:?}")), 4),
        };
    machine.inject(injection);
    let mut write_trace = Vec::new();
    let report = machine.run(budget, |range| {
        write_trace.push(json!({"gpa":format!("0x{:x}",range.start().get()),"length":range.len()}))
    });
    let (irr, isr) = machine.vector_state();
    let ptes = machine.ptes();
    let s = machine.state();
    let rip = s.rip;
    let rsp = s.gpr[od_isa::Register::Rsp as usize];
    let rflags = s.rflags;
    let cs = s.cs_selector.unwrap_or(0);
    let cr2 = s.cr2;
    let halted = s.activity == Some(od_isa::CpuActivity::Halted);
    let eligible = machine.apic().eligible();
    let (stop_reason, stop_detail) = stop_reason(&report.stop);
    let complete = report.stop == Stage4PagedStiHltStop::Completed;
    let event = |step| report.events.iter().find(|e| e.step == step);
    let checks = [
        ("case_complete", complete),
        (
            "all_steps_executed",
            report.steps == STAGE4_PAGED_STI_HLT_WAKE_STEPS
                && report.retired == STAGE4_PAGED_STI_HLT_WAKE_INSTRUCTIONS,
        ),
        (
            "sti_set_if_and_shadow",
            event(Stage4PagedStiHltStep::Sti).is_some_and(|e| {
                e.retired
                    && e.rip == STAGE4_PAGED_STI_HLT_FIRST_HLT_RIP
                    && e.rflags == 0x202
                    && e.shadow_blocking
            }),
        ),
        (
            "hlt_halted_and_ended_shadow",
            event(Stage4PagedStiHltStep::Hlt).is_some_and(|e| {
                e.retired
                    && e.halted
                    && !e.shadow_blocking
                    && e.rip == STAGE4_PAGED_STI_HLT_SECOND_HLT_RIP
            }),
        ),
        (
            "idle_poll_keeps_waiting",
            event(Stage4PagedStiHltStep::IdlePoll)
                .is_some_and(|e| e.exit == "idle" && e.halted && !e.irr && !e.isr),
        ),
        (
            "wake_entered_handler",
            event(Stage4PagedStiHltStep::Wake).is_some_and(|e| {
                e.exit == "entered"
                    && !e.halted
                    && e.rip == STAGE4_PAGED_IRQ_HANDLER_GVA
                    && e.cs == 0x08
                    && e.rsp == FRAME_RSP
                    && e.rflags & IF == 0
            }),
        ),
        (
            "frame_exact",
            event(Stage4PagedStiHltStep::Wake).is_some_and(|e| {
                e.frame
                    == Some([
                        STAGE4_PAGED_STI_HLT_SECOND_HLT_RIP,
                        0x08,
                        0x202,
                        STAGE4_PAGED_IRQ_INITIAL_RSP,
                        0x10,
                    ])
            }),
        ),
        (
            "vector_in_service_during_handler",
            event(Stage4PagedStiHltStep::Wake).is_some_and(|e| !e.irr && e.isr),
        ),
        (
            "eoi_cleared_isr",
            event(Stage4PagedStiHltStep::Eoi).is_some_and(|e| e.retired && !e.irr && !e.isr),
        ),
        (
            "iretq_returned_after_hlt",
            event(Stage4PagedStiHltStep::Iretq).is_some_and(|e| {
                e.retired
                    && e.rip == STAGE4_PAGED_STI_HLT_SECOND_HLT_RIP
                    && e.rsp == STAGE4_PAGED_IRQ_INITIAL_RSP
                    && e.rflags == 0x202
            }),
        ),
        (
            "final_state_halted",
            rip == STAGE4_PAGED_STI_HLT_WAKE_FINAL_RIP
                && halted
                && rsp == STAGE4_PAGED_IRQ_INITIAL_RSP,
        ),
        ("irq_state_clear", !irr && !isr && eligible.is_none()),
        (
            "ptes_accessed_dirty",
            ptes.main == (STAGE4_PAGED_IRQ_PTE_MAIN | 0x20)
                && ptes.handler == (STAGE4_PAGED_IRQ_PTE_HANDLER | 0x20)
                && ptes.idt == (STAGE4_PAGED_IRQ_PTE_IDT | 0x20)
                && ptes.gdt == (STAGE4_PAGED_IRQ_PTE_GDT | 0x20)
                && ptes.lapic == (STAGE4_PAGED_IRQ_PTE_LAPIC | 0x60)
                && ptes.stack == (STAGE4_PAGED_IRQ_PTE_STACK | 0x60),
        ),
        ("no_cr2_change", cr2.is_none()),
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
            "scenario": "wake_from_hlt",
            "status": if ok { "completed" } else if complete { "rejected" } else { "error" },
            "accepted": ok,
            "injection": injection_name(injection),
            "budget": budget,
            "vector": hex(u64::from(STAGE4_PAGED_IRQ_VECTOR)),
            "stop_reason": stop_reason,
            "stop_detail": stop_detail,
            "steps": report.steps,
            "retired": report.retired,
            "expected_steps": STAGE4_PAGED_STI_HLT_WAKE_STEPS,
            "expected_instructions": STAGE4_PAGED_STI_HLT_WAKE_INSTRUCTIONS,
            "guest_state": {
                "rip": format!("0x{rip:016x}"),
                "rsp": format!("0x{rsp:016x}"),
                "rflags": hex(rflags),
                "cs": hex(u64::from(cs)),
                "cr2": cr2.map(hex),
                "cr3": s.cr3.map(hex),
                "halted": halted,
                "irr": irr,
                "isr": isr,
                "eligible": eligible.map(|v| hex(u64::from(v))),
            },
            "ptes": ptes_json(&ptes),
            "event_trace": events,
            "committed_write_trace": write_trace,
            "model_note": "delivery refusals and aborts are terminal; hardware outcome is sequential #PF/#NP delivery (S4-33); memory type of the APIC page is not modelled",
            "checks": checks.iter().map(|&(n, p)| (n.to_string(), Value::Bool(p))).collect::<serde_json::Map<_, _>>(),
            "failed_checks": failed,
            "error": if ok { Value::Null } else { json!({
                "kind": if complete { "paged_sti_hlt_result_mismatch" } else { stop_reason },
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

fn ptes_json(p: &Stage4PagedIrqPtes) -> Value {
    json!({
        "main": hex(p.main),
        "handler": hex(p.handler),
        "idt": hex(p.idt),
        "gdt": hex(p.gdt),
        "lapic": hex(p.lapic),
        "stack": hex(p.stack),
    })
}

fn event_json(index: usize, e: &Stage4PagedStiHltEvent) -> Value {
    json!({
        "step": index + 1,
        "name": step_name(e.step),
        "retired": e.retired,
        "exit": e.exit,
        "detail": e.detail,
        "rip_after": format!("0x{:016x}", e.rip),
        "rsp_after": format!("0x{:016x}", e.rsp),
        "rflags_after": hex(e.rflags),
        "if_after": e.rflags & IF != 0,
        "cs_after": hex(u64::from(e.cs)),
        "cr2_after": e.cr2.map(hex),
        "halted_after": e.halted,
        "shadow_after": e.shadow_blocking,
        "irr_after": e.irr,
        "isr_after": e.isr,
        "ptes_after": ptes_json(&e.ptes),
        "page_fault": e.page_fault.map(|f| json!({
            "vector": f.vector,
            "error_code": f.error_code,
            "linear_address": f.linear_address.map(hex),
            "cr2": hex(f.cr2),
        })),
        "frame_low_to_high": e.frame.map(|f| f.iter().map(|v| hex(*v)).collect::<Vec<_>>()),
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

fn step_name(step: Stage4PagedStiHltStep) -> &'static str {
    match step {
        Stage4PagedStiHltStep::Sti => "sti: STI",
        Stage4PagedStiHltStep::Hlt => "hlt: HLT",
        Stage4PagedStiHltStep::IdlePoll => "idle poll: nothing requested",
        Stage4PagedStiHltStep::Wake => "wake: vector 0x41 through the paged IDT",
        Stage4PagedStiHltStep::Entry => "entry: vector 0x41 at the boundary",
        Stage4PagedStiHltStep::Eoi => "eoi: MOV [RBX],EAX",
        Stage4PagedStiHltStep::Iretq => "IRETQ",
        Stage4PagedStiHltStep::ResumeHlt => "resume: HLT",
    }
}

fn stop_reason(stop: &Stage4PagedStiHltStop) -> (&'static str, Option<String>) {
    match stop {
        Stage4PagedStiHltStop::Completed => ("case_complete", None),
        Stage4PagedStiHltStop::BudgetExhausted => ("budget_exhausted", None),
        Stage4PagedStiHltStop::DeliveryOutsideProfile(reason) => {
            ("delivery_outside_profile", Some(format!("{reason:?}")))
        }
        Stage4PagedStiHltStop::DeliveryAborted { phase, cause } => {
            ("delivery_aborted", Some(format!("{phase:?}: {cause:?}")))
        }
        Stage4PagedStiHltStop::SelectionChanged => ("selection_changed", None),
        Stage4PagedStiHltStop::Step(step, exit) => (exit, Some(step_name(*step).to_string())),
        Stage4PagedStiHltStop::Error(e) => ("integration_error", Some(e.clone())),
    }
}

fn injection_name(i: Stage4PagedStiHltInjection) -> &'static str {
    match i {
        Stage4PagedStiHltInjection::None => "none",
        Stage4PagedStiHltInjection::IdtGateNotPresent => "idt_not_present",
        Stage4PagedStiHltInjection::StackReadOnly => "stack_read_only",
    }
}

fn parse(args: &[String]) -> Result<(String, u64, Stage4PagedStiHltInjection), String> {
    let mut p = None;
    let mut b = None;
    let mut injection = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--stage4-paged-sti-hlt-probe" if p.is_none() => {
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
            flag @ ("--inject-idt-not-present" | "--inject-stack-read-only") => {
                if injection.is_some() {
                    return Err("only one injection may be specified".into());
                }
                injection = Some(if flag == "--inject-idt-not-present" {
                    Stage4PagedStiHltInjection::IdtGateNotPresent
                } else {
                    Stage4PagedStiHltInjection::StackReadOnly
                });
                i += 1;
            }
            x => return Err(format!("unexpected option {x}")),
        }
    }
    Ok((
        p.ok_or("--stage4-paged-sti-hlt-probe IMAGE is required")?,
        b.unwrap_or(16),
        injection.unwrap_or(Stage4PagedStiHltInjection::None),
    ))
}

fn failure(kind: &str, detail: String) -> Value {
    json!({"schema_version":1,"profile":PROFILE,"status":"error","accepted":false,"stop_reason":"error","retired":null,"guest_state":null,"event_trace":[],"error":{"kind":kind,"detail":detail}})
}

fn emit(v: Value, code: u8) -> ExitCode {
    println!("{v}");
    ExitCode::from(code)
}
