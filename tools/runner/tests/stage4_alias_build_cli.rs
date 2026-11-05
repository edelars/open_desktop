use serde_json::Value;
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
            "tools/build/build_stage4_flat_probe.py",
            "--profile",
            "alias-probe",
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
    root.join(format!("work/stage4/{name}/flat_probe.bin"))
}
fn run(image: &Path, budget: u8, inject: bool) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    c.arg("--stage4-alias-probe")
        .arg(image)
        .args(["--budget", &budget.to_string()]);
    if inject {
        c.arg("--inject-break-alias");
    }
    c.output().unwrap()
}
fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap()
}
#[test]
fn real_alias_artifact_is_reproducible_and_guest_result_tracks_payload() {
    let r = root();
    let tag = format!("s4-33a-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"), A, B);
    let a2 = build(&r, &format!("{tag}-a2"), A, B);
    let b = build(&r, &format!("{tag}-b"), B, A);
    let bytes = fs::read(&a).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(r.join(format!("work/stage4/{tag}-a/build_manifest.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["layout"]["mappings"][0]["gpa"], "0x9000");
    assert_eq!(manifest["layout"]["mappings"][1]["gpa"], "0x9000");
    assert_eq!(manifest["layout"]["mappings"][0]["raw_offset"], "0x8000");
    assert_eq!(manifest["layout"]["mappings"][1]["raw_offset"], "0x8000");
    assert_eq!(manifest["layout"]["stack"]["gva_start"], "0xa00000");
    assert_eq!(manifest["layout"]["stack"]["gva_end"], "0xa01000");
    assert_eq!(manifest["layout"]["stack"]["gpa_start"], "0x9000");
    assert_eq!(manifest["layout"]["stack"]["gpa_end"], "0xa000");
    assert_eq!(manifest["layout"]["stack"]["in_raw_image"], true);
    assert_eq!(manifest["layout"]["stack"]["raw_offset"], "0x8000");
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
        for budget in 0..=4 {
            let out = run(p, budget, false);
            let x = json(&out);
            assert_eq!(x["profile"], "Stage4AliasProbeV1");
            assert_eq!(x["retired"], budget);
            assert_eq!(x["expected_store_qword"], format!("0x{store:016x}"));
            assert_eq!(
                x["observed_alias_qword"],
                format!("0x{:016x}", if budget >= 1 { store } else { initial })
            );
            assert_eq!(
                x["observed_rax"],
                format!("0x{:016x}", if budget >= 2 { store } else { 0 })
            );
            assert_eq!(
                x["observed_rsp"],
                format!("0x{:016x}", if budget >= 2 { 0xa00008 } else { 0xa00000 })
            );
            assert_eq!(x["accepted"], budget == 4);
            if budget == 0 {
                assert_eq!(x["observed_alias_qword"], format!("0x{initial:016x}"));
            }
            if budget == 4 {
                assert!(out.status.success());
                assert_eq!(x["exit"], "halted");
                assert_eq!(x["serial_bytes"], serde_json::json!([store as u8]));
                let trace = x["committed_write_trace"].as_array().unwrap();
                assert_eq!(
                    trace
                        .iter()
                        .filter(|entry| entry["gpa"] == "0x9000")
                        .count(),
                    1
                );
            } else {
                assert_eq!(out.status.code(), Some(5));
            }
        }
    }
    let injected = run(&a, 4, true);
    let injected_json = json(&injected);
    assert_eq!(injected.status.code(), Some(7));
    assert_eq!(injected_json["exit"], "halted");
    assert_eq!(injected_json["activity"], "Halted");
    assert_eq!(injected_json["accepted"], false);
    assert_eq!(injected_json["status"], "rejected");
    assert_eq!(injected_json["observed_alias_qword"], format!("0x{B:016x}"));
    assert_eq!(injected_json["observed_rax"], "0x0000000000000000");
    for n in ["-a", "-a2", "-b"] {
        let _ = fs::remove_dir_all(r.join(format!("work/stage4/{tag}{n}")));
    }
}
