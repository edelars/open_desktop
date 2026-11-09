use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
/// Bytes 11 22 33 on data page 0 and 44 55 66 77 88 on data page 1.
const A: u64 = 0x8877665544332211;
/// Same first-page bytes, every byte on the second page differs.
const B: u64 = 0xa1b2c3d4e5332211;
const SENTINEL: u64 = 0xdead_beef_cafe_babe;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn build(root: &Path, name: &str, value: u64) -> PathBuf {
    let out = Command::new("python3")
        .current_dir(root)
        .args([
            "tools/build/build_stage4_cross_page_probe.py",
            "--value",
            &format!("0x{value:016x}"),
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
    root.join(format!("work/stage4/{name}/cross_page_probe.bin"))
}
fn run(image: &Path, budget: u16, flags: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    c.arg("--stage4-cross-page-probe")
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
fn rax_per_step(value: u64) -> [u64; 6] {
    let dword = value & 0xffff_ffff;
    [
        (SENTINEL & !0xffff) | (value & 0xffff),
        value,
        dword,
        dword,
        dword,
        dword,
    ]
}

#[test]
fn real_cross_page_artifact_is_reproducible_and_guest_trace_tracks_the_second_page() {
    let r = root();
    let tag = format!("s4-33d-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"), A);
    let a2 = build(&r, &format!("{tag}-a2"), A);
    let b = build(&r, &format!("{tag}-b"), B);
    let bytes = fs::read(&a).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(r.join(format!("work/stage4/{tag}-a/build_manifest.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["profile"], "Stage4CrossPageProbeV1");
    assert_eq!(manifest["first_page_bytes"], "112233");
    assert_eq!(manifest["second_page_bytes"], "4455667788");
    assert_eq!(
        manifest["layout"]["code"]["tail"]["bytes"],
        "03488b038b03668b03eef4"
    );
    let maps = manifest["layout"]["mappings"].as_array().unwrap();
    let gpas: Vec<&str> = maps.iter().map(|m| m["gpa"].as_str().unwrap()).collect();
    assert_eq!(gpas, ["0x7000", "0x9000", "0xb000", "0xd000"]);
    assert_eq!(bytes, fs::read(a2).unwrap());
    assert_ne!(bytes, fs::read(&b).unwrap());
    // Only the second-page bytes (and the control copy) differ between the two images.
    let differing: Vec<usize> = bytes
        .iter()
        .zip(fs::read(&b).unwrap())
        .enumerate()
        .filter_map(|(i, (x, y))| (*x != y).then_some(i))
        .collect();
    assert!(
        differing
            .iter()
            .all(|&i| (0xc000..0xc005).contains(&i) || (0xa800..0xa808).contains(&i))
    );
    assert!(differing.iter().any(|&i| (0xc000..0xc005).contains(&i)));

    // Success with both values; the second page decides RAX from the first read that reaches it.
    let mut finals = Vec::new();
    for (p, value) in [(&a, A), (&b, B)] {
        let out = run(p, 16, &[]);
        assert!(out.status.success());
        let x = json(&out);
        assert_eq!(x["profile"], "Stage4CrossPageProbeV1");
        assert_eq!(x["status"], "completed");
        assert_eq!(x["accepted"], true);
        assert_eq!(x["stop_reason"], "case_complete");
        assert_eq!(
            (x["steps"].clone(), x["retired"].clone()),
            (json!(6), json!(6))
        );
        assert_eq!(x["value_qword"], hex(value));
        assert_eq!(x["handoff"]["cpl"], 0);
        assert_eq!(x["handoff"]["rflags_ac"], false);
        assert_eq!(x["handoff"]["efer_nxe"], true);
        assert_eq!(x["serial_bytes"], json!([0x11]));
        assert_eq!(x["guest_state"]["activity"], "Halted");
        assert_eq!(x["guest_state"]["rip"], hex(0x40_100b));
        assert_eq!(x["guest_state"]["cr2"], Value::Null);
        assert!(failed(&x).is_empty());
        let t = x["event_trace"].as_array().unwrap();
        for (i, expected) in rax_per_step(value).into_iter().enumerate() {
            assert_eq!(t[i]["rax_after"], hex(expected), "step {i}");
            assert_eq!(t[i]["retired"], true);
            assert_eq!(t[i]["cpl"], 0);
            for page in ["code0", "code1", "data0", "data1"] {
                assert_eq!(t[i]["pte_after"][page]["dirty"], false, "step {i} {page}");
            }
        }
        // A by page: step 1 walks both code frames and data page 0; step 2 reaches data page 1.
        assert_eq!(t[0]["pte_after"]["code1"]["accessed"], true);
        assert_eq!(t[0]["pte_after"]["data0"]["accessed"], true);
        assert_eq!(t[0]["pte_after"]["data1"]["accessed"], false);
        assert_eq!(t[1]["pte_after"]["data1"]["accessed"], true);
        assert_eq!(t[1]["committed_writes"][0]["gpa"], "0x5008");
        assert_eq!(
            t[0]["committed_writes"].as_array().unwrap().len(),
            7,
            "step 1 walks PML4E, PDPTE, PDE, two code PTEs, data PDE, data PTE 0"
        );
        finals.push(x["guest_state"]["rax"].clone());
    }
    // Two different second-page values give two different results.
    assert_eq!(
        finals,
        [json!(hex(A & 0xffff_ffff)), json!(hex(B & 0xffff_ffff))]
    );

    // Same-page control: identical values, the second data page is never walked.
    let ctl = run(&a, 16, &["--same-page-control"]);
    assert!(ctl.status.success());
    let x = json(&ctl);
    assert_eq!(x["operand"], "same_page_control");
    assert_eq!(x["observed_ptes"]["data1"]["accessed"], false);
    assert_eq!(x["observed_ptes"]["data0"]["accessed"], true);
    assert_eq!(x["guest_state"]["rbx"], hex(0x80_0800));
    for (i, expected) in rax_per_step(A).into_iter().enumerate() {
        assert_eq!(x["event_trace"][i]["rax_after"], hex(expected));
        assert_eq!(x["event_trace"][i]["pte_after"]["data1"]["accessed"], false);
    }

    // Budgets 0..=6: prefix of the full trace, STOP reason budget_exhausted before the end.
    for budget in 0..=6_u16 {
        let out = run(&a, budget, &[]);
        let x = json(&out);
        assert_eq!(x["steps"], budget);
        assert_eq!(x["event_trace"].as_array().unwrap().len(), budget as usize);
        assert_eq!(x["accepted"], budget == 6);
        if budget == 6 {
            assert!(out.status.success());
        } else {
            assert_eq!(out.status.code(), Some(5));
            assert_eq!(x["stop_reason"], "budget_exhausted");
            assert!(x["failed_checks"].as_array().unwrap().len() >= 1);
        }
        let expected_serial = if budget >= 5 {
            json!([0x11])
        } else {
            json!([])
        };
        assert_eq!(x["serial_bytes"], expected_serial);
    }

    // Injection 1: second data PTE not present -> #PF at the start of the second page, PFEC 0.
    let out = run(&a, 16, &["--inject-data-pte-not-present"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["injection"], "second_data_pte_not_present");
    assert_eq!(x["stop_reason"], "page_fault");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(2), json!(1))
    );
    let pf = &x["event_trace"][1]["page_fault"];
    assert_eq!(
        (pf["vector"].clone(), pf["error_code"].clone()),
        (json!(14), json!(0))
    );
    assert_eq!(pf["cr2"], "0x801000");
    assert_eq!(pf["access"], "Read");
    assert_eq!(x["event_trace"][1]["retired"], false);
    assert_eq!(x["event_trace"][1]["rax_after"], hex(rax_per_step(A)[0]));
    assert_eq!(x["observed_ptes"]["data0"]["accessed"], true);
    assert_eq!(x["observed_ptes"]["data1"]["present"], false);
    assert_eq!(x["guest_state"]["rax"], hex(rax_per_step(A)[0]));

    // Injection 2: NX on the second code PTE -> #PF on the fetch, PFEC 0x11, no operand walk.
    let out = run(&a, 16, &["--inject-code-pte-nx"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["injection"], "second_code_pte_nx");
    assert_eq!(x["stop_reason"], "page_fault");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(1), json!(0))
    );
    let pf = &x["event_trace"][0]["page_fault"];
    assert_eq!(pf["error_code"], 0x11);
    assert_eq!(pf["cr2"], "0x401000");
    assert_eq!(pf["access"], "Fetch");
    assert_eq!(pf["saved_rip"], "0x400ffe");
    assert_eq!(x["observed_ptes"]["data0"]["accessed"], false);
    assert_eq!(x["observed_ptes"]["data1"]["accessed"], false);
    assert_eq!(x["guest_state"]["rax"], hex(SENTINEL));

    // Injection 3: loader swaps the data frames -> every step retires, RAX is wrong, case fails.
    let out = run(&a, 16, &["--inject-swap-data-frames"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(x["status"], "rejected");
    assert_eq!(x["accepted"], false);
    assert_eq!(x["stop_reason"], "case_complete");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(6), json!(6))
    );
    assert_eq!(
        x["event_trace"][0]["rax_after"],
        hex((SENTINEL & !0xffff) | 0x5a5a)
    );
    assert_eq!(x["serial_bytes"], json!([0x5a]));
    assert!(failed(&x).contains(&"rax_follows_architecture_per_step".to_string()));
    assert!(!failed(&x).contains(&"no_page_fault_or_backend_error".to_string()));

    // Injection 4: second data page in a RAM hole -> rejection, no retirement, no #PF.
    let out = run(&a, 16, &["--inject-data-hole"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["stop_reason"], "backend_rejected");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(2), json!(1))
    );
    assert!(x["event_trace"][1]["page_fault"].is_null());
    assert!(
        x["event_trace"][1]["memory_error"]
            .as_str()
            .unwrap()
            .contains("OperandPhysical")
    );
    assert_eq!(x["event_trace"][1]["retired"], false);

    // Argument and image validation.
    let bad = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["--stage4-cross-page-probe"])
        .arg(&a)
        .args(["--inject-data-hole", "--inject-code-pte-nx"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(3));
    let short = r.join(format!("work/stage4/{tag}-a/short.bin"));
    fs::write(&short, [0u8; 16]).unwrap();
    assert_eq!(run(&short, 4, &[]).status.code(), Some(3));
    let mut foreign = bytes.clone();
    foreign[0x4008 - 0x1000] = 0x03; // second code PTE becomes RW: not the fixed profile
    let foreign_path = r.join(format!("work/stage4/{tag}-a/foreign.bin"));
    fs::write(&foreign_path, foreign).unwrap();
    let out = run(&foreign_path, 4, &[]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["error"]["kind"], "loader");
    for n in ["-a", "-a2", "-b"] {
        let _ = fs::remove_dir_all(r.join(format!("work/stage4/{tag}{n}")));
    }
}
