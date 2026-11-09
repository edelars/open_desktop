use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
/// Default builder values: offset 0, offset 0x100000, offset 0x1ffff8 of the 2 MiB page.
const START: u64 = 0x1122334455667788;
const MIDDLE: u64 = 0x99aabbccddeeff01;
const END: u64 = 0x0f1e2d3c4b5a6978;
/// Second set of values: every qword differs.
const START_B: u64 = 0x0102030405060708;
const MIDDLE_B: u64 = 0x1112131415161718;
const END_B: u64 = 0x2122232425262728;
const SENTINEL: u64 = 0xfeedface0badf00d;
const DECOY: u64 = 0xa5a5a5a5a5a5a5a5;
const RAX0: u64 = 0xdead_beef_cafe_babe;
/// RCX of the handoff: the stored value.
const W: u64 = 0xc0de_c0de_1234_56ab;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn build(root: &Path, name: &str, values: Option<[u64; 3]>) -> PathBuf {
    let mut c = Command::new("python3");
    c.current_dir(root)
        .arg("tools/build/build_stage4_huge_pages_probe.py");
    if let Some([s, m, e]) = values {
        c.args([
            "--start-value",
            &format!("0x{s:016x}"),
            "--middle-value",
            &format!("0x{m:016x}"),
            "--end-value",
            &format!("0x{e:016x}"),
        ]);
    }
    let out = c
        .args(["--output-dir", &format!("work/stage4/{name}")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    root.join(format!("work/stage4/{name}/huge_pages_probe.bin"))
}
fn run(image: &Path, budget: u16, flags: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    c.arg("--stage4-huge-pages-probe")
        .arg(image)
        .args(["--budget", &budget.to_string()])
        .args(flags);
    c.output().unwrap()
}
fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap()
}
fn hex(v: u64) -> String {
    format!("0x{v:016x}")
}
fn failed(x: &Value) -> Vec<String> {
    x["failed_checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect()
}
fn rax_per_step(v: [u64; 3]) -> [u64; 7] {
    [v[0], v[1], v[2], v[2], W, W, W]
}

#[test]
fn real_huge_pages_artifact_is_reproducible_and_guest_trace_covers_2m_and_1gib() {
    let r = root();
    let tag = format!("s4-33e-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"), None);
    let a2 = build(&r, &format!("{tag}-a2"), None);
    let b = build(&r, &format!("{tag}-b"), Some([START_B, MIDDLE_B, END_B]));
    let bytes = fs::read(&a).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(r.join(format!("work/stage4/{tag}-a/build_manifest.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["profile"], "Stage4HugePagesProbeV1");
    assert_eq!(manifest["start_value_u64"], hex(START));
    assert_eq!(manifest["middle_value_u64"], hex(MIDDLE));
    assert_eq!(manifest["end_value_u64"], hex(END));
    assert_eq!(
        manifest["layout"]["code"]["bytes"],
        "488b03488b03488b0348890b488b03eef4"
    );
    let maps = manifest["layout"]["mappings"].as_array().unwrap();
    let big = maps.iter().find(|m| m["gva"] == "0x800000").unwrap();
    assert_eq!(big["size"], "2 MiB");
    assert_eq!(big["entry"], "0x8000000000200083");
    assert_eq!(big["ps"], 1);
    let gib = maps.iter().find(|m| m["gva"] == "0x8000000000").unwrap();
    assert_eq!(gib["ps"], 1);
    assert_eq!(bytes, fs::read(a2).unwrap());
    assert_ne!(bytes, fs::read(&b).unwrap());
    // Only the three values differ between the two images.
    let other = fs::read(&b).unwrap();
    let differing: Vec<usize> = bytes
        .iter()
        .zip(&other)
        .enumerate()
        .filter_map(|(i, (x, y))| (*x != *y).then_some(i))
        .collect();
    let in_value = |i: usize| {
        [0x20_0000usize, 0x30_0000, 0x3f_fff8]
            .iter()
            .any(|&g| (g - 0x1000..g - 0x1000 + 8).contains(&i))
    };
    assert!(differing.iter().all(|&i| in_value(i)));

    // Success with both value sets: each offset returns its own value.
    for (p, v) in [(&a, [START, MIDDLE, END]), (&b, [START_B, MIDDLE_B, END_B])] {
        let out = run(p, 16, &[]);
        assert!(out.status.success());
        let x = json(&out);
        assert_eq!(x["profile"], "Stage4HugePagesProbeV1");
        assert_eq!(x["status"], "completed");
        assert_eq!(x["accepted"], true);
        assert_eq!(x["case"], "two_mib");
        assert_eq!(x["injection"], "none");
        assert_eq!(x["stop_reason"], "case_complete");
        assert_eq!(
            (x["steps"].clone(), x["retired"].clone()),
            (json!(7), json!(7))
        );
        assert_eq!(x["values"]["start"], hex(v[0]));
        assert_eq!(x["values"]["middle"], hex(v[1]));
        assert_eq!(x["values"]["end"], hex(v[2]));
        assert_eq!(x["handoff"]["cpl"], 0);
        assert_eq!(x["handoff"]["rflags_ac"], false);
        assert_eq!(x["handoff"]["efer_nxe"], true);
        assert_eq!(x["serial_bytes"], json!([0xab]));
        assert_eq!(x["guest_state"]["activity"], "Halted");
        assert_eq!(x["guest_state"]["rip"], hex(0x40_0011));
        assert_eq!(x["guest_state"]["cr2"], Value::Null);
        assert_eq!(x["store_target_qword"], hex(W));
        assert!(failed(&x).is_empty());
        let t = x["event_trace"].as_array().unwrap();
        for (i, expected) in rax_per_step(v).into_iter().enumerate() {
            assert_eq!(t[i]["rax_after"], hex(expected), "step {i}");
            assert_eq!(t[i]["retired"], true);
            assert_eq!(t[i]["cpl"], 0);
            // PS=1 leaf on the data PDE; D only from the store (step 4).
            assert_eq!(t[i]["entries_after"]["pde_2m"]["ps"], true);
            assert_eq!(t[i]["entries_after"]["pde_2m"]["accessed"], true);
            assert_eq!(t[i]["entries_after"]["pde_2m"]["dirty"], i >= 3, "step {i}");
            assert_eq!(t[i]["entries_after"]["pdpte1_ps"]["accessed"], false);
        }
        assert_eq!(
            t[0]["entries_after"]["pde_2m"]["entry"],
            hex(0x8000_0000_0020_00a3)
        );
        assert_eq!(
            t[3]["entries_after"]["pde_2m"]["entry"],
            hex(0x8000_0000_0020_00e3)
        );
        assert_eq!(t[0]["committed_writes"].as_array().unwrap().len(), 5);
        assert_eq!(t[3]["committed_writes"][0]["gpa"], "0x3020");
        assert_eq!(t[3]["committed_writes"][1]["gpa"], "0x3fff00");
    }

    // Budgets 0..=7: prefix of the full trace, STOP reason budget_exhausted before the end.
    for budget in 0..=7_u16 {
        let out = run(&a, budget, &[]);
        let x = json(&out);
        assert_eq!(x["steps"], budget);
        assert_eq!(x["event_trace"].as_array().unwrap().len(), budget as usize);
        assert_eq!(x["accepted"], budget == 7);
        if budget == 7 {
            assert!(out.status.success());
        } else {
            assert_eq!(out.status.code(), Some(5));
            assert_eq!(x["stop_reason"], "budget_exhausted");
            assert!(!failed(&x).is_empty());
        }
        let expected_serial = if budget >= 6 {
            json!([0xab])
        } else {
            json!([])
        };
        assert_eq!(x["serial_bytes"], expected_serial);
    }

    // Negative: PDPTE.PS=1 is a guest #PF with P|RSVD, A only on PML4E[1], no retirement.
    let out = run(&a, 16, &["--negative-1gib"]);
    let x = json(&out);
    assert!(out.status.success(), "{x}");
    assert_eq!(x["case"], "gib_negative");
    assert_eq!(x["accepted"], true);
    assert_eq!(x["stop_reason"], "page_fault");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(1), json!(0))
    );
    let pf = &x["event_trace"][0]["page_fault"];
    assert_eq!(pf["vector"], 14);
    assert_eq!(pf["error_code"], 0x09);
    assert_eq!(pf["error_code_hex"], "0x09");
    assert_eq!(pf["cr2"], "0x8000000000");
    assert_eq!(pf["saved_rip"], "0x400000");
    assert_eq!(pf["access"], "Read");
    assert_eq!(x["expected_pfec"], "0x09");
    assert_eq!(x["event_trace"][0]["retired"], false);
    assert!(x["event_trace"][0]["memory_error"].is_null());
    assert_eq!(x["guest_state"]["rip"], hex(0x40_0000));
    assert_eq!(x["guest_state"]["rax"], hex(RAX0));
    assert_eq!(x["guest_state"]["cr2"], Value::Null);
    let e = &x["observed_entries"];
    assert_eq!(e["pml4e1"]["entry"], hex(0x5023));
    assert_eq!(e["pml4e1"]["accessed"], true);
    assert_eq!(e["pdpte1_ps"]["entry"], hex(0x4000_0083));
    assert_eq!(e["pdpte1_ps"]["accessed"], false);
    assert_eq!(e["pdpte1_ps"]["dirty"], false);
    assert_eq!(e["pde_2m"]["accessed"], false);
    assert_eq!(x["serial_bytes"], json!([]));
    assert!(failed(&x).is_empty());

    // Injection 1: PDE.PS replaced with a 4 KiB table -> the outcome differs: the first load
    // returns the decoy and the second is a #PF with PFEC 0 on the not-present PTE.
    let out = run(&a, 16, &["--inject-pde-4k-table"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["injection"], "pde_ps_cleared");
    assert_eq!(x["stop_reason"], "page_fault");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(2), json!(1))
    );
    assert_eq!(x["event_trace"][0]["rax_after"], hex(DECOY));
    let pf = &x["event_trace"][1]["page_fault"];
    assert_eq!(
        (pf["vector"].clone(), pf["error_code"].clone()),
        (json!(14), json!(0))
    );
    assert_eq!(pf["cr2"], "0x900000");
    assert_eq!(x["observed_entries"]["pde_2m"]["ps"], false);
    assert!(failed(&x).contains(&"per_step_rip_rax_rbx_and_entries".to_string()));

    // Injection 2: PDPTE.PS path ends in the host error UnsupportedPageSize, not a guest #PF.
    let out = run(&a, 16, &["--negative-1gib", "--inject-pdpte-host-error"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["injection"], "pdpte_ps_host_error");
    assert_eq!(x["stop_reason"], "backend_rejected");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(1), json!(0))
    );
    assert!(x["event_trace"][0]["page_fault"].is_null());
    assert!(
        x["event_trace"][0]["memory_error"]
            .as_str()
            .unwrap()
            .contains("UnsupportedPageSize")
    );
    assert_eq!(x["accepted"], false);
    assert!(failed(&x).contains(&"page_fault_stop".to_string()));
    assert!(failed(&x).contains(&"vector_14_pfec_p_rsvd_read_cpl0".to_string()));

    // Injection 3: an offset one byte past the 2 MiB page reads another physical frame.
    let out = run(&a, 16, &["--inject-offset-past-page"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(x["status"], "rejected");
    assert_eq!(x["injection"], "offset_past_page");
    assert_eq!(x["stop_reason"], "case_complete");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(7), json!(7))
    );
    assert_eq!(x["event_trace"][2]["rax_after"], hex(SENTINEL));
    assert_eq!(x["event_trace"][2]["rbx"], hex(0xa0_0000));
    assert!(failed(&x).contains(&"rax_follows_architecture_per_step".to_string()));
    assert!(!failed(&x).contains(&"no_page_fault_or_backend_error".to_string()));

    // Argument and image validation.
    let run_args = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_od-runner"))
            .args(["--stage4-huge-pages-probe"])
            .arg(&a)
            .args(extra)
            .output()
            .unwrap()
    };
    assert_eq!(
        run_args(&["--inject-pde-4k-table", "--inject-offset-past-page"])
            .status
            .code(),
        Some(3)
    );
    // An injection that belongs to the other case is refused.
    assert_eq!(
        run_args(&["--inject-pdpte-host-error"]).status.code(),
        Some(3)
    );
    assert_eq!(
        run_args(&["--negative-1gib", "--inject-offset-past-page"])
            .status
            .code(),
        Some(3)
    );
    let short = r.join(format!("work/stage4/{tag}-a/short.bin"));
    fs::write(&short, [0u8; 16]).unwrap();
    assert_eq!(run(&short, 4, &[]).status.code(), Some(3));
    let mut foreign = bytes.clone();
    foreign[0x3020 - 0x1000] = 0x03; // the 2 MiB PDE loses PS: not the fixed profile
    let foreign_path = r.join(format!("work/stage4/{tag}-a/foreign.bin"));
    fs::write(&foreign_path, foreign).unwrap();
    let out = run(&foreign_path, 4, &[]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["error"]["kind"], "loader");
    for n in ["-a", "-a2", "-b"] {
        let _ = fs::remove_dir_all(r.join(format!("work/stage4/{tag}{n}")));
    }
}
