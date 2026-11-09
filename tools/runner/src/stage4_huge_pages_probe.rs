//! S4-33e CLI: `Stage4HugePagesProbeV1`, a 2 MiB RAM page (positive) and a 1 GiB PDPTE.PS=1 (negative).
use od_isa::Register;
use od_machine::{
    STAGE4_HUGE_PAGES_END_GVA, STAGE4_HUGE_PAGES_ENTRIES, STAGE4_HUGE_PAGES_ENTRY_NAMES,
    STAGE4_HUGE_PAGES_FINAL_RIP, STAGE4_HUGE_PAGES_GIB_GVA, STAGE4_HUGE_PAGES_GIB_STEPS,
    STAGE4_HUGE_PAGES_IMAGE_BYTES, STAGE4_HUGE_PAGES_INITIAL_RAX, STAGE4_HUGE_PAGES_INSTRUCTIONS,
    STAGE4_HUGE_PAGES_MIDDLE_GVA, STAGE4_HUGE_PAGES_PAST_PAGE_GVA, STAGE4_HUGE_PAGES_START_GVA,
    STAGE4_HUGE_PAGES_STEP_RIPS, STAGE4_HUGE_PAGES_STEPS, STAGE4_HUGE_PAGES_STORE_GPA,
    STAGE4_HUGE_PAGES_STORE_GVA, STAGE4_HUGE_PAGES_STORE_VALUE, Stage4HugePagesCase,
    Stage4HugePagesEvent, Stage4HugePagesInjection, Stage4HugePagesStep, Stage4HugePagesStop,
    load_stage4_huge_pages_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4HugePagesProbeV1";
const A: u64 = 1 << 5;
const D: u64 = 1 << 6;
const PS: u64 = 1 << 7;
/// PFEC of a CPL0 data read of a present entry with a reserved bit set (SDM Vol. 3A 4.7):
/// P (bit 0) | RSVD (bit 3).
const PFEC_P_RSVD_READ: u32 = 0x09;

struct Options {
    path: String,
    budget: u64,
    case: Stage4HugePagesCase,
    injection: Stage4HugePagesInjection,
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
        .take((STAGE4_HUGE_PAGES_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
    {
        return emit(failure("file", e.to_string()), 3);
    }
    if image.len() != STAGE4_HUGE_PAGES_IMAGE_BYTES {
        return emit(
            failure(
                "image_size",
                format!(
                    "expected {}, got {}",
                    STAGE4_HUGE_PAGES_IMAGE_BYTES,
                    image.len()
                ),
            ),
            3,
        );
    }
    let mut machine = match load_stage4_huge_pages_probe(&image) {
        Ok(m) => m,
        Err(e) => return emit(failure("loader", format!("{e:?}")), 4),
    };
    machine.select_case(options.case);
    if let Err(e) = machine.inject(options.injection) {
        return emit(failure("arguments", format!("{e:?}")), 3);
    }
    let (cpl, ac, nxe, efer) = (machine.cpl(), machine.ac(), machine.nxe(), machine.efer());
    let values = machine.values();
    let mut write_trace = Vec::new();
    let report = machine.run(options.budget, |range| {
        write_trace.push(json!({"gpa":format!("0x{:x}",range.start().get()),"length":range.len()}))
    });
    let entries = machine.entries();
    let store_qword = machine.physical_qword(STAGE4_HUGE_PAGES_STORE_GPA);
    let s = machine.state();
    let rax = s.gpr[Register::Rax as usize];
    let (stop_reason, stop_detail) = stop_reason(&report.stop);
    let negative = options.case == Stage4HugePagesCase::GibNegative;

    // Expected values come from the architecture (Intel SDM Vol. 3A, chapter 4), not from the
    // machine: a little-endian qword load puts the bytes at the operand's physical address in RAX;
    // GPA = 2 MiB frame base + bits 20:0 of the linear address (4.5, Table 4-18).
    let initial = STAGE4_HUGE_PAGES_ENTRIES;
    // Entries after the first fetch and the first data access: A on every entry used in the
    // translation (PML4E, PDPTE, PD[2], code PTE, the 2 MiB PDE). D only after the store, and only
    // in the entry that maps the page (the PDE for a 2 MiB page) (4.8).
    let after_reads = {
        let mut e = initial;
        for i in [0, 1, 2, 3, 4] {
            e[i] |= A;
        }
        e
    };
    let after_store = {
        let mut e = after_reads;
        e[4] |= D;
        e
    };
    let expected_rax = [
        values[0],
        values[1],
        values[2],
        values[2],
        STAGE4_HUGE_PAGES_STORE_VALUE,
        STAGE4_HUGE_PAGES_STORE_VALUE,
        STAGE4_HUGE_PAGES_STORE_VALUE,
    ];
    let expected_rbx = [
        STAGE4_HUGE_PAGES_START_GVA,
        STAGE4_HUGE_PAGES_MIDDLE_GVA,
        STAGE4_HUGE_PAGES_END_GVA,
        STAGE4_HUGE_PAGES_STORE_GVA,
        STAGE4_HUGE_PAGES_STORE_GVA,
        STAGE4_HUGE_PAGES_STORE_GVA,
        STAGE4_HUGE_PAGES_STORE_GVA,
    ];
    let expected_rips_after = {
        let mut r = [0u64; 7];
        for i in 0..7 {
            r[i] = STAGE4_HUGE_PAGES_STEP_RIPS
                .get(i + 1)
                .copied()
                .unwrap_or(STAGE4_HUGE_PAGES_FINAL_RIP);
        }
        r
    };
    let event_ok = |i: usize, e: &Stage4HugePagesEvent| {
        e.retired
            && e.rip == expected_rips_after[i]
            && e.rax == expected_rax[i]
            && (i >= 5 || e.rbx == expected_rbx[i])
            && e.cpl == 0
            && e.page_fault.is_none()
            && e.memory_error.is_none()
            && e.entries == if i >= 3 { after_store } else { after_reads }
    };
    let complete = report.stop == Stage4HugePagesStop::Completed;
    let checks: Vec<(&str, bool)> = if !negative {
        let steps_ok = report.events.len() == STAGE4_HUGE_PAGES_STEPS as usize
            && report
                .events
                .iter()
                .enumerate()
                .all(|(i, e)| event_ok(i, e));
        vec![
            ("case_complete", complete),
            (
                "all_steps_executed",
                report.steps == STAGE4_HUGE_PAGES_STEPS
                    && report.retired == STAGE4_HUGE_PAGES_INSTRUCTIONS,
            ),
            ("handoff_cpl0_ac_clear_nxe_set", cpl == 0 && !ac && nxe),
            ("per_step_rip_rax_rbx_and_entries", steps_ok),
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
                "values_differ_per_offset",
                values[0] != values[1] && values[0] != values[2] && values[1] != values[2],
            ),
            (
                "pde_is_a_2m_leaf_with_a_and_d",
                entries[4] & PS != 0
                    && entries[4] & A != 0
                    && entries[4] & D != 0
                    && entries[4] == after_store[4],
            ),
            (
                "d_on_pde_only_after_store",
                report
                    .events
                    .iter()
                    .enumerate()
                    .all(|(i, e)| (e.entries[4] & D != 0) == (i >= 3)),
            ),
            (
                "ancestors_have_a_and_no_d",
                entries[..4].iter().all(|v| v & A != 0 && v & D == 0),
            ),
            ("unused_entries_untouched", entries[5..] == initial[5..]),
            (
                "store_landed_in_the_2m_frame",
                store_qword == STAGE4_HUGE_PAGES_STORE_VALUE,
            ),
            (
                "no_page_fault_or_backend_error",
                report
                    .events
                    .iter()
                    .all(|e| e.page_fault.is_none() && e.memory_error.is_none()),
            ),
            (
                "uart_byte_is_low_byte_of_stored_value",
                report.serial == [(STAGE4_HUGE_PAGES_STORE_VALUE & 0xff) as u8],
            ),
            ("final_rax", rax == STAGE4_HUGE_PAGES_STORE_VALUE),
            ("final_rip", s.rip == STAGE4_HUGE_PAGES_FINAL_RIP),
            ("halted", s.activity == Some(od_isa::CpuActivity::Halted)),
        ]
    } else {
        let pf = report.events.first().and_then(|e| e.page_fault.as_ref());
        let e0 = report.events.first();
        let mut expected_entries = initial;
        // Fetch path (PML4E[0], PDPT0[0], PD[2], code PTE) and the data route's PML4E[1] get A.
        // The PDPT1 entry with PS=1 is not accessed (it fails first), no D anywhere.
        for i in [0, 1, 2, 3, 6] {
            expected_entries[i] |= A;
        }
        vec![
            (
                "page_fault_stop",
                report.stop == Stage4HugePagesStop::PageFault(Stage4HugePagesStep::GibRead),
            ),
            (
                "one_step_no_retirement",
                report.steps == STAGE4_HUGE_PAGES_GIB_STEPS && report.retired == 0,
            ),
            ("handoff_cpl0_ac_clear_nxe_set", cpl == 0 && !ac && nxe),
            (
                "vector_14_pfec_p_rsvd_read_cpl0",
                pf.is_some_and(|f| {
                    f.vector == 14 && f.error_code == Some(PFEC_P_RSVD_READ) && f.access == "Read"
                }),
            ),
            (
                "cr2_is_the_access_address",
                pf.is_some_and(|f| f.cr2 == STAGE4_HUGE_PAGES_GIB_GVA),
            ),
            (
                "saved_rip_is_the_faulting_instruction",
                pf.is_some_and(|f| f.saved_rip == STAGE4_HUGE_PAGES_STEP_RIPS[0]),
            ),
            (
                "rip_rax_unchanged",
                s.rip == STAGE4_HUGE_PAGES_STEP_RIPS[0] && rax == STAGE4_HUGE_PAGES_INITIAL_RAX,
            ),
            (
                "a_on_pml4e_only_ps_entry_untouched",
                e0.is_some_and(|e| e.entries == expected_entries),
            ),
            (
                "data_walk_commits_only_pml4e_a",
                e0.is_some_and(|e| {
                    e.committed_writes
                        .last()
                        .is_some_and(|r| r.start().get() == 0x1008)
                        && e.committed_writes.len() == 5
                }),
            ),
            (
                "no_backend_error",
                report.events.iter().all(|e| e.memory_error.is_none()),
            ),
            ("no_uart_output", report.serial.is_empty()),
        ]
    };
    let failed: Vec<&str> = checks
        .iter()
        .filter_map(|&(name, pass)| (!pass).then_some(name))
        .collect();
    let ok = failed.is_empty();
    // For the negative the expected stop is the page fault; a completed run means a wrong result.
    let finished = if negative {
        matches!(report.stop, Stage4HugePagesStop::PageFault(_)) && report.retired == 0
    } else {
        complete
    };
    let events: Vec<Value> = report
        .events
        .iter()
        .enumerate()
        .map(|(index, e)| event_json(index, e))
        .collect();
    let expected_steps = if negative {
        STAGE4_HUGE_PAGES_GIB_STEPS
    } else {
        STAGE4_HUGE_PAGES_STEPS
    };
    emit(
        json!({
            "schema_version": 1,
            "profile": PROFILE,
            "status": if ok { "completed" } else if complete { "rejected" } else { "error" },
            "accepted": ok,
            "case": if negative { "gib_negative" } else { "two_mib" },
            "injection": injection_name(options.injection),
            "budget": options.budget,
            "stop_reason": stop_reason,
            "stop_detail": stop_detail,
            "steps": report.steps,
            "retired": report.retired,
            "expected_steps": expected_steps,
            "expected_instructions": if negative { 0 } else { STAGE4_HUGE_PAGES_INSTRUCTIONS },
            "values": {
                "start": format!("0x{:016x}", values[0]),
                "middle": format!("0x{:016x}", values[1]),
                "end": format!("0x{:016x}", values[2]),
                "stored": format!("0x{STAGE4_HUGE_PAGES_STORE_VALUE:016x}"),
            },
            "operands": {
                "start_gva": hex(STAGE4_HUGE_PAGES_START_GVA),
                "middle_gva": hex(STAGE4_HUGE_PAGES_MIDDLE_GVA),
                "end_gva": hex(STAGE4_HUGE_PAGES_END_GVA),
                "store_gva": hex(STAGE4_HUGE_PAGES_STORE_GVA),
                "past_page_gva": hex(STAGE4_HUGE_PAGES_PAST_PAGE_GVA),
                "gib_gva": hex(STAGE4_HUGE_PAGES_GIB_GVA),
            },
            "expected_rax_per_step": if negative { Vec::new() } else { expected_rax.iter().map(|v| format!("0x{v:016x}")).collect::<Vec<_>>() },
            "expected_step_rips": STAGE4_HUGE_PAGES_STEP_RIPS.iter().map(|v| format!("0x{v:x}")).collect::<Vec<_>>(),
            "expected_pfec": if negative { json!(format!("0x{PFEC_P_RSVD_READ:02x}")) } else { Value::Null },
            "handoff": {
                "cpl": cpl,
                "rflags_ac": ac,
                "efer": format!("0x{efer:x}"),
                "efer_nxe": nxe,
                "initial_rax": format!("0x{STAGE4_HUGE_PAGES_INITIAL_RAX:016x}"),
                "rcx_store_value": format!("0x{STAGE4_HUGE_PAGES_STORE_VALUE:016x}"),
            },
            "guest_state": {
                "rip": format!("0x{:016x}", s.rip),
                "rflags": format!("0x{:x}", s.rflags),
                "cr2": s.cr2.map(|v| format!("0x{v:x}")),
                "cr3": s.cr3.map(|v| format!("0x{v:x}")),
                "rax": format!("0x{rax:016x}"),
                "rbx": format!("0x{:016x}", s.gpr[Register::Rbx as usize]),
                "rcx": format!("0x{:016x}", s.gpr[Register::Rcx as usize]),
                "rdx": format!("0x{:016x}", s.gpr[Register::Rdx as usize]),
                "activity": s.activity.map(|a| format!("{a:?}")),
                "cpl": cpl,
            },
            "observed_entries": entries_json(&entries),
            "store_target_qword": format!("0x{store_qword:016x}"),
            "serial_bytes": report.serial,
            "event_trace": events,
            "committed_write_trace": write_trace,
            "checks": checks.iter().map(|&(n, p)| (n.to_string(), Value::Bool(p))).collect::<serde_json::Map<_, _>>(),
            "failed_checks": failed,
            "error": if ok { Value::Null } else { json!({
                "kind": if finished && complete { "huge_page_result_mismatch" } else { stop_reason },
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

fn entries_json(entries: &[u64; 8]) -> Value {
    let mut map = serde_json::Map::new();
    for (name, &entry) in STAGE4_HUGE_PAGES_ENTRY_NAMES.iter().zip(entries) {
        map.insert(
            (*name).to_string(),
            json!({
                "entry": format!("0x{entry:016x}"),
                "present": entry & 1 != 0,
                "ps": entry & PS != 0,
                "accessed": entry & A != 0,
                "dirty": entry & D != 0,
                "nx": entry >> 63 != 0,
            }),
        );
    }
    Value::Object(map)
}

fn event_json(index: usize, e: &Stage4HugePagesEvent) -> Value {
    json!({
        "step": index + 1,
        "name": step_name(e.step),
        "retired": e.retired,
        "exit": e.exit,
        "detail": e.detail,
        "rip_after": format!("0x{:016x}", e.rip),
        "rax_after": format!("0x{:016x}", e.rax),
        "rbx": format!("0x{:016x}", e.rbx),
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
        "entries_after": entries_json(&e.entries),
        "committed_writes": e.committed_writes.iter().map(|r| json!({
            "gpa": hex(r.start().get()),
            "length": r.len(),
        })).collect::<Vec<_>>(),
    })
}

fn step_name(step: Stage4HugePagesStep) -> &'static str {
    match step {
        Stage4HugePagesStep::ReadStart => "load at offset 0 of the 2 MiB page: 48 8B 03",
        Stage4HugePagesStep::ReadMiddle => "load at offset 0x100000: 48 8B 03",
        Stage4HugePagesStep::ReadEnd => "load at offset 0x1ffff8: 48 8B 03",
        Stage4HugePagesStep::Store => "store at offset 0x1fff00: 48 89 0B",
        Stage4HugePagesStep::ReadBack => "load back at offset 0x1fff00: 48 8B 03",
        Stage4HugePagesStep::Out => "OUT DX,AL",
        Stage4HugePagesStep::Halt => "HLT",
        Stage4HugePagesStep::GibRead => "load through PDPTE.PS=1: 48 8B 03",
    }
}

fn stop_reason(stop: &Stage4HugePagesStop) -> (&'static str, Option<String>) {
    match stop {
        Stage4HugePagesStop::Completed => ("case_complete", None),
        Stage4HugePagesStop::BudgetExhausted => ("budget_exhausted", None),
        Stage4HugePagesStop::PageFault(step) => ("page_fault", Some(step_name(*step).to_string())),
        Stage4HugePagesStop::Backend(step) => {
            ("backend_rejected", Some(step_name(*step).to_string()))
        }
        Stage4HugePagesStop::Step(step, exit) => (exit, Some(step_name(*step).to_string())),
        Stage4HugePagesStop::Error(e) => ("integration_error", Some(e.clone())),
    }
}

fn injection_name(i: Stage4HugePagesInjection) -> &'static str {
    match i {
        Stage4HugePagesInjection::None => "none",
        Stage4HugePagesInjection::PdePsCleared => "pde_ps_cleared",
        Stage4HugePagesInjection::PdptePsHostError => "pdpte_ps_host_error",
        Stage4HugePagesInjection::OffsetPastPage => "offset_past_page",
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut path = None;
    let mut budget = None;
    let mut injection = None;
    let mut case = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--stage4-huge-pages-probe" if path.is_none() => {
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
            "--negative-1gib" if case.is_none() => {
                case = Some(Stage4HugePagesCase::GibNegative);
                i += 1;
            }
            "--negative-1gib" => return Err("--negative-1gib may be specified only once".into()),
            flag @ ("--inject-pde-4k-table"
            | "--inject-pdpte-host-error"
            | "--inject-offset-past-page") => {
                if injection.is_some() {
                    return Err("only one injection may be specified".into());
                }
                injection = Some(match flag {
                    "--inject-pde-4k-table" => Stage4HugePagesInjection::PdePsCleared,
                    "--inject-pdpte-host-error" => Stage4HugePagesInjection::PdptePsHostError,
                    _ => Stage4HugePagesInjection::OffsetPastPage,
                });
                i += 1;
            }
            x => return Err(format!("unexpected option {x}")),
        }
    }
    Ok(Options {
        path: path.ok_or("--stage4-huge-pages-probe IMAGE is required")?,
        budget: budget.unwrap_or(16),
        case: case.unwrap_or(Stage4HugePagesCase::TwoMiB),
        injection: injection.unwrap_or(Stage4HugePagesInjection::None),
    })
}

fn failure(kind: &str, detail: String) -> Value {
    json!({"schema_version":1,"profile":PROFILE,"status":"error","accepted":false,"stop_reason":"error","retired":null,"guest_state":null,"event_trace":[],"error":{"kind":kind,"detail":detail}})
}

fn emit(v: Value, code: u8) -> ExitCode {
    println!("{v}");
    ExitCode::from(code)
}
