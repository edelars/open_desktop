use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
const A: u64 = 0x1122334455667788;
const B: u64 = 0x8877665544332211;
const CANARY: u64 = 0x6a3591c7d248b0ef;
const MAIN: u64 = 0x40_1000;
const HANDLER: u64 = 0x40_2000;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn build(root: &Path, name: &str, initial: u64, store: u64) -> PathBuf {
    let out = Command::new("python3")
        .current_dir(root)
        .args([
            "tools/build/build_stage4_pf_repair_probe.py",
            "--initial-value",
            &format!("0x{initial:016x}"),
            "--store-value",
            &format!("0x{store:016x}"),
            "--output-dir",
            &format!("work/stage4/{name}"),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    root.join(format!("work/stage4/{name}/pf_repair_probe.bin"))
}
fn run(image: &Path, budget: u16, inject: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    c.arg("--stage4-pf-repair-probe")
        .arg(image)
        .args(["--budget", &budget.to_string()]);
    if let Some(flag) = inject {
        c.arg(flag);
    }
    c.output().unwrap()
}
fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap()
}
fn hex(v: u64) -> String {
    format!("0x{v:016x}")
}
fn names(x: &Value) -> Vec<String> {
    x["event_trace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["name"]
                .as_str()
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn real_pf_repair_artifact_is_reproducible_and_guest_trace_tracks_payload() {
    let r = root();
    let tag = format!("s4-33c-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"), A, B);
    let a2 = build(&r, &format!("{tag}-a2"), A, B);
    let b = build(&r, &format!("{tag}-b"), B, A);
    let bytes = fs::read(&a).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(r.join(format!("work/stage4/{tag}-a/build_manifest.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["profile"], "Stage4PfRepairProbeV1");
    assert_eq!(manifest["layout"]["main_code"]["bytes"], "48890b488b03");
    assert_eq!(
        manifest["layout"]["handler_code"]["bytes"],
        "4889370f013b488d64240848cf"
    );
    assert_eq!(manifest["layout"]["mappings"][0]["access"], "supervisor-ro");
    assert_eq!(manifest["layout"]["repair"]["pte_value"], "0xd023");
    assert_eq!(bytes, fs::read(a2).unwrap());
    assert_ne!(bytes, fs::read(&b).unwrap());

    for (p, initial, store) in [(&a, A, B), (&b, B, A)] {
        for budget in 0..=8_u16 {
            let out = run(p, budget, None);
            let x = json(&out);
            assert_eq!(x["profile"], "Stage4PfRepairProbeV1");
            assert_eq!(x["steps"], budget);
            assert_eq!(x["event_trace"].as_array().unwrap().len(), budget as usize);
            assert_eq!(x["expected_store_qword"], hex(store));
            assert_eq!(
                x["observed_data_qword"],
                hex(if budget >= 7 { store } else { initial })
            );
            assert_eq!(x["canary_qword"], hex(CANARY));
            assert_eq!(x["accepted"], budget == 8);
            // The stack holds the #PF frame between delivery and IRETQ.
            let rsp = match budget {
                2..=4 => 0x70_1ff0,
                5 => 0x70_1ff8,
                _ => 0x0070_2027,
            };
            assert_eq!(x["guest_state"]["rsp"], hex(rsp));
            if budget == 8 {
                assert!(out.status.success());
                assert_eq!(x["stop_reason"], "case_complete");
                assert_eq!(x["status"], "completed");
                assert_eq!(x["retired"], 6);
                assert_eq!(x["observed_pte"], "0xd063");
                assert_eq!(x["guest_state"]["rip"], hex(0x40_1006));
                assert_eq!(x["guest_state"]["rax"], hex(store));
                assert_eq!(x["guest_state"]["cr2"], "0x800000");
                assert_eq!(x["guest_state"]["rf"], false);
                assert_eq!(
                    names(&x),
                    [
                        "fault",
                        "delivery",
                        "repair",
                        "INVLPG [RBX]",
                        "discard",
                        "IRETQ",
                        "retry",
                        "load"
                    ]
                );
                let t = &x["event_trace"];
                assert_eq!(t[0]["page_fault"]["vector"], 14);
                assert_eq!(t[0]["page_fault"]["error_code"], 3);
                assert_eq!(t[0]["page_fault"]["cr2"], "0x800000");
                assert_eq!(t[0]["pte_after"], "0xd021");
                assert_eq!(t[1]["rip_after"], hex(HANDLER));
                assert_eq!(t[1]["error_code_on_stack"], 3);
                assert_eq!(t[2]["pte_after"], "0xd023");
                assert_eq!(t[2]["committed_writes"][1]["gpa"], "0x6000");
                assert_eq!(t[4]["rsp_after"], hex(0x70_1ff8));
                assert_eq!(t[5]["iretq_frame_rip"], format!("0x{MAIN:x}"));
                assert_eq!(t[5]["rf_after"], true);
                assert_eq!(t[6]["rf_after"], false);
                assert_eq!(t[6]["pte_after"], "0xd063");
                assert_eq!(t[6]["committed_writes"][1]["gpa"], "0xd000");
                assert_eq!(t[7]["rax_after"], hex(store));
                let trace = x["committed_write_trace"].as_array().unwrap();
                assert_eq!(trace.len(), 14);
                assert_eq!(trace.iter().filter(|e| e["gpa"] == "0xd000").count(), 1);
                assert!(x["failed_checks"].as_array().unwrap().is_empty());
            } else {
                assert_eq!(out.status.code(), Some(5));
                assert_eq!(x["stop_reason"], "budget_exhausted");
            }
        }
    }

    // Injection 1: the handler stores the RO value; IRETQ returns, the retry faults again.
    let ro = run(&a, 16, Some("--inject-pte-readonly"));
    let x = json(&ro);
    assert_eq!(ro.status.code(), Some(5));
    assert_eq!(x["status"], "error");
    assert_eq!(x["accepted"], false);
    assert_eq!(x["injection"], "pte_stays_readonly");
    assert_eq!(x["stop_reason"], "second_page_fault");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(7), json!(4))
    );
    assert_eq!(x["event_trace"][6]["page_fault"]["error_code"], 3);
    assert_eq!(x["event_trace"][6]["page_fault"]["cr2"], "0x800000");
    assert_eq!(x["observed_pte"], "0xd021");
    assert_eq!(x["observed_data_qword"], hex(A));
    let failed = x["failed_checks"].as_array().unwrap();
    for c in [
        "no_second_fault",
        "retry_store_committed",
        "pte_accessed_dirty",
    ] {
        assert!(failed.contains(&json!(c)), "{c}");
    }
    assert!(!failed.contains(&json!("iretq_returned_to_fault_rip")));

    // Injection 2: no LEA, so IRETQ pops the error code as RIP and does not retire.
    let lea = run(&a, 16, Some("--inject-skip-lea"));
    let x = json(&lea);
    assert_eq!(lea.status.code(), Some(5));
    assert_eq!(x["injection"], "skip_lea");
    assert_eq!(x["stop_reason"], "unsupported");
    assert_eq!(x["stop_detail"], "IRETQ");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(5), json!(2))
    );
    assert_eq!(x["event_trace"][4]["iretq_frame_rip"], "0x3");
    assert_eq!(x["event_trace"][4]["retired"], false);
    assert_eq!(x["guest_state"]["rip"], hex(HANDLER + 6));
    assert_eq!(x["observed_data_qword"], hex(A));
    let failed = x["failed_checks"].as_array().unwrap();
    for c in [
        "error_code_discarded_before_iretq",
        "iretq_returned_to_fault_rip",
    ] {
        assert!(failed.contains(&json!(c)), "{c}");
    }

    // Injection 3: the loader maps the page RW; the store retires and nothing is delivered.
    let rw = run(&a, 16, Some("--inject-pte-writable"));
    let x = json(&rw);
    assert_eq!(rw.status.code(), Some(5));
    assert_eq!(x["injection"], "pte_writable_at_load");
    assert_eq!(x["stop_reason"], "no_page_fault");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(1), json!(1))
    );
    assert!(x["event_trace"][0]["page_fault"].is_null());
    assert_eq!(x["observed_data_qword"], hex(B));
    assert!(
        x["failed_checks"]
            .as_array()
            .unwrap()
            .contains(&json!("fault_raised_on_readonly_store"))
    );
    assert_eq!(x["guest_state"]["rip"], hex(MAIN + 3));

    // Argument and image validation.
    let bad = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["--stage4-pf-repair-probe"])
        .arg(&a)
        .args(["--inject-pte-readonly", "--inject-skip-lea"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(3));
    let short = r.join(format!("work/stage4/{tag}-a/short.bin"));
    fs::write(&short, [0u8; 16]).unwrap();
    assert_eq!(run(&short, 4, None).status.code(), Some(3));
    let mut foreign = bytes.clone();
    foreign[0x5000] = 0x03; // data PTE RW from the start
    let foreign_path = r.join(format!("work/stage4/{tag}-a/foreign.bin"));
    fs::write(&foreign_path, foreign).unwrap();
    let out = run(&foreign_path, 4, None);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["error"]["kind"], "loader");
    for n in ["-a", "-a2", "-b"] {
        let _ = fs::remove_dir_all(r.join(format!("work/stage4/{tag}{n}")));
    }
}
