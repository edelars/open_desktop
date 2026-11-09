use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
const A: u64 = 0x1122334455667788;
const B: u64 = 0x8877665544332211;
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
            "tools/build/build_stage4_cpl3_syscall_probe.py",
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
    root.join(format!("work/stage4/{name}/cpl3_syscall_probe.bin"))
}
fn run(image: &Path, budget: u8, inject: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    c.arg("--stage4-cpl3-syscall-probe")
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

#[test]
fn real_cpl3_syscall_artifact_is_reproducible_and_guest_trace_tracks_payload() {
    let r = root();
    let tag = format!("s4-33b-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"), A, B);
    let a2 = build(&r, &format!("{tag}-a2"), A, B);
    let b = build(&r, &format!("{tag}-b"), B, A);
    let bytes = fs::read(&a).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(r.join(format!("work/stage4/{tag}-a/build_manifest.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["profile"], "Stage4Cpl3SyscallProbeV1");
    assert_eq!(manifest["layout"]["user_code"]["bytes"], "0f05488b03");
    assert_eq!(manifest["layout"]["kernel_code"]["bytes"], "4c8917480f07");
    assert_eq!(manifest["layout"]["lstar_gva"], "0x600000");
    assert_eq!(manifest["layout"]["mappings"][0]["gpa"], "0x9000");
    assert_eq!(manifest["layout"]["mappings"][1]["gpa"], "0x9000");
    assert_eq!(manifest["layout"]["mappings"][0]["access"], "supervisor-rw");
    assert_eq!(manifest["layout"]["mappings"][1]["access"], "user-ro");
    assert_eq!(
        u64::from_le_bytes(bytes[0x8000..0x8008].try_into().unwrap()),
        A
    );
    assert_eq!(
        u64::from_le_bytes(bytes[0x8008..0x8010].try_into().unwrap()),
        B
    );
    assert_eq!(bytes, fs::read(a2).unwrap());
    assert_ne!(bytes, fs::read(&b).unwrap());

    for (p, initial, store) in [(&a, A, B), (&b, B, A)] {
        for budget in 0..=4_u8 {
            let out = run(p, budget, None);
            let x = json(&out);
            assert_eq!(x["profile"], "Stage4Cpl3SyscallProbeV1");
            assert_eq!(x["retired"], budget);
            assert_eq!(x["expected_store_qword"], hex(store));
            assert_eq!(
                x["observed_kernel_qword"],
                hex(if budget >= 2 { store } else { initial })
            );
            assert_eq!(
                x["guest_state"]["rax"],
                hex(if budget == 4 {
                    store
                } else {
                    0x0123456789abcdef
                })
            );
            assert_eq!(
                x["guest_state"]["cpl"],
                if matches!(budget, 1 | 2) { 0 } else { 3 }
            );
            assert_eq!(x["guest_state"]["rsp"], hex(0x0070_2027));
            assert_eq!(x["event_trace"].as_array().unwrap().len(), budget as usize);
            assert_eq!(x["accepted"], budget == 4);
            if budget == 4 {
                assert!(out.status.success());
                assert_eq!(x["stop_reason"], "roundtrip_complete");
                assert_eq!(x["status"], "completed");
                assert_eq!(x["guest_state"]["rip"], hex(0x400005));
                assert_eq!(x["guest_state"]["cs"], "0x23");
                assert_eq!(x["guest_state"]["rcx"], hex(0x400002));
                let cpls: Vec<_> = x["event_trace"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["cpl_after"].clone())
                    .collect();
                assert_eq!(cpls, [json!(0), json!(0), json!(3), json!(3)]);
                let trace = x["committed_write_trace"].as_array().unwrap();
                assert_eq!(trace.iter().filter(|e| e["gpa"] == "0x9000").count(), 1);
                assert_eq!(x["event_trace"][1]["committed_writes"][5]["gpa"], "0x9000");
            } else {
                assert_eq!(out.status.code(), Some(5));
                assert_eq!(x["stop_reason"], "budget_exhausted");
            }
        }
    }

    // Injection 1: the kernel misses the qword. The roundtrip completes in CPL3,
    // so only the value evidence differs.
    let no_write = run(&a, 4, Some("--inject-kernel-no-write"));
    let x = json(&no_write);
    assert_eq!(no_write.status.code(), Some(7));
    assert_eq!(x["status"], "rejected");
    assert_eq!(x["accepted"], false);
    assert_eq!(x["injection"], "kernel_no_write");
    assert_eq!(x["stop_reason"], "roundtrip_complete");
    assert_eq!(x["guest_state"]["cpl"], 3);
    assert_eq!(x["observed_kernel_qword"], hex(A));
    assert_eq!(x["guest_state"]["rax"], hex(A));
    assert_eq!(
        x["failed_checks"],
        json!(["kernel_qword_written", "user_load_observed_kernel_qword"])
    );
    let writes: Vec<_> = x["committed_write_trace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["gpa"].clone())
        .collect();
    assert!(writes.contains(&json!("0x9ff8")));
    assert!(!writes.contains(&json!("0x9000")));

    // Injection 2: SYSRETQ leaves the guest in CPL0. The profile refuses the user
    // load and the case stops with CPL 0 visible in the final state.
    let cpl0 = run(&a, 4, Some("--inject-sysret-cpl0"));
    let x = json(&cpl0);
    assert_eq!(cpl0.status.code(), Some(5));
    assert_eq!(x["status"], "error");
    assert_eq!(x["accepted"], false);
    assert_eq!(x["injection"], "sysret_cpl0");
    assert_eq!(x["stop_reason"], "unsupported");
    assert_eq!(x["retired"], 3);
    assert_eq!(x["guest_state"]["cpl"], 0);
    assert_eq!(x["guest_state"]["cs"], "0x8");
    assert_eq!(x["observed_kernel_qword"], hex(B));
    assert_eq!(x["event_trace"][3]["exit"], "unsupported");
    assert!(
        x["failed_checks"]
            .as_array()
            .unwrap()
            .contains(&json!("returned_to_cpl3"))
    );

    // Argument and image validation.
    let bad = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["--stage4-cpl3-syscall-probe"])
        .arg(&a)
        .args(["--inject-kernel-no-write", "--inject-sysret-cpl0"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(3));
    for n in ["-a", "-a2", "-b"] {
        let _ = fs::remove_dir_all(r.join(format!("work/stage4/{tag}{n}")));
    }
}
