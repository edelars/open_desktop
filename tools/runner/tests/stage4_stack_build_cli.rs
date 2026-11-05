use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PAYLOAD_A: u64 = 0x1122_3344_5566_7788;
const PAYLOAD_B: u64 = 0x8877_6655_4433_2211;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn build(root: &Path, name: &str, payload: u64) -> PathBuf {
    let out = Command::new("python3")
        .current_dir(root)
        .args([
            "tools/build/build_stage4_flat_probe.py",
            "--profile",
            "stack-probe",
            "--payload",
            &format!("0x{payload:016x}"),
            "--output-dir",
            &format!("work/stage4/{name}"),
        ])
        .output()
        .expect("fixture builder starts");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root.join(format!("work/stage4/{name}/flat_probe.bin"))
}

fn run(image: &Path, budget: u64) -> Output {
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-stack-probe")
        .arg(image)
        .args(["--budget", &budget.to_string()])
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("JSON result")
}

#[test]
fn actual_stack_probe_artifact_is_reproducible_and_respects_all_budgets() {
    let root = root();
    let suffix = format!("s4-32c-{}", std::process::id());
    let a = build(&root, &format!("{suffix}-a"), PAYLOAD_A);
    let a2 = build(&root, &format!("{suffix}-a2"), PAYLOAD_A);
    let b = build(&root, &format!("{suffix}-b"), PAYLOAD_B);
    let bytes = fs::read(&a).unwrap();
    assert_eq!(bytes, fs::read(&a2).unwrap());
    assert_ne!(bytes, fs::read(&b).unwrap());
    for (image, payload) in [(&a, PAYLOAD_A), (&b, PAYLOAD_B)] {
        for budget in 0..=4 {
            let output = run(image, budget);
            let result = json(&output);
            assert_eq!(result["profile"], "Stage4StackProbeV1");
            assert_eq!(result["retired"], budget);
            assert_eq!(result["rip"], format!("0x{:016x}", 0x0040_0000 + budget));
            assert_eq!(
                result["observed_stack_qword"],
                if budget >= 1 {
                    format!("0x{payload:016x}")
                } else {
                    "0x0000000000000000".to_string()
                }
            );
            assert_eq!(
                result["observed_rax"],
                format!("0x{:016x}", if budget >= 2 { payload } else { 0 })
            );
            assert_eq!(
                result["observed_rsp"],
                format!(
                    "0x{:016x}",
                    if budget == 1 {
                        0x00a0_0ff8
                    } else {
                        0x00a0_1000
                    }
                )
            );
            if budget == 4 {
                assert!(output.status.success());
                assert_eq!(result["exit"], "halted");
                assert_eq!(result["activity"], "Halted");
            } else {
                assert_eq!(output.status.code(), Some(5));
                assert_eq!(result["exit"], "budget_exhausted");
            }
            assert_eq!(
                result["serial_bytes"],
                if budget >= 3 {
                    serde_json::json!([payload as u8])
                } else {
                    serde_json::json!([])
                }
            );
        }
    }

    let malformed = root.join(format!("work/stage4/{suffix}-corrupt-code.bin"));
    let mut damaged = bytes.clone();
    damaged[0x6000] ^= 1;
    fs::write(&malformed, damaged).unwrap();
    assert_eq!(run(&malformed, 4).status.code(), Some(4));
    let mut damaged = bytes;
    damaged[0x5000] ^= 1;
    fs::write(&malformed, damaged).unwrap();
    assert_eq!(run(&malformed, 4).status.code(), Some(4));
    let _ = fs::remove_file(malformed);
    for tail in ["-a", "-a2", "-b"] {
        let _ = fs::remove_dir_all(root.join(format!("work/stage4/{suffix}{tail}")));
    }
}

#[test]
fn duplicate_budget_is_rejected_before_loading_the_image() {
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args([
            "--stage4-stack-probe",
            "missing.bin",
            "--budget",
            "1",
            "--budget",
            "2",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let result = json(&output);
    assert_eq!(result["profile"], "Stage4StackProbeV1");
    assert_eq!(result["error"]["kind"], "arguments");
}
