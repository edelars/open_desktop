use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const INITIAL: u64 = 0x0102_0304_0506_0708;
const VALUE_A: u64 = 0x1122_3344_5566_7788;
const VALUE_B: u64 = 0x8877_6655_4433_2211;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn build(root: &Path, name: &str, initial: u64, value: u64) -> PathBuf {
    let out = Command::new("python3")
        .current_dir(root)
        .args([
            "tools/build/build_stage4_flat_probe.py",
            "--profile",
            "store-load-probe",
            "--initial-value",
            &format!("0x{initial:016x}"),
            "--store-value",
            &format!("0x{value:016x}"),
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
fn run(image: &Path, budget: Option<u64>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    command.arg("--stage4-store-load-probe").arg(image);
    if let Some(b) = budget {
        command.args(["--budget", &b.to_string()]);
    }
    command.output().unwrap()
}

fn run_with_args(image: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-store-load-probe")
        .arg(image)
        .args(args)
        .output()
        .unwrap()
}
fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("JSON result")
}

#[test]
fn built_store_load_artifacts_are_reproducible_and_drive_guest_results() {
    let root = root();
    let suffix = format!("s4-32b-{}", std::process::id());
    let a = build(&root, &format!("{suffix}-a"), INITIAL, VALUE_A);
    let a2 = build(&root, &format!("{suffix}-a2"), INITIAL, VALUE_A);
    let b = build(&root, &format!("{suffix}-b"), INITIAL, VALUE_B);
    let bytes = fs::read(&a).unwrap();
    assert_eq!(
        bytes,
        fs::read(&a2).unwrap(),
        "same inputs must reproduce raw artifact"
    );
    assert_ne!(bytes, fs::read(&b).unwrap());
    for (path, value) in [(&a, VALUE_A), (&b, VALUE_B)] {
        let output = run(path, None);
        let result = json(&output);
        assert!(output.status.success());
        assert_eq!(result["profile"], "Stage4StoreLoadProbeV1");
        assert_eq!(result["exit"], "halted");
        assert_eq!(result["retired"], 4);
        assert_eq!(result["rip"], "0x0000000000400008");
        assert_eq!(result["activity"], "Halted");
        assert_eq!(result["rflags_defined"], "0x0000000000000cd7");
        assert_eq!(result["observed_data_qword"], format!("0x{value:016x}"));
        assert_eq!(result["observed_rax"], format!("0x{value:016x}"));
        assert_eq!(result["serial_bytes"], serde_json::json!([value as u8]));
    }
    for budget in 1..=3 {
        let output = run(&a, Some(budget));
        let result = json(&output);
        assert_eq!(output.status.code(), Some(5));
        assert_eq!(result["exit"], "budget_exhausted");
        assert_eq!(result["retired"], budget);
        assert_eq!(
            result["observed_data_qword"],
            format!("0x{:016x}", if budget >= 1 { VALUE_A } else { INITIAL })
        );
        assert_eq!(
            result["observed_rax"],
            format!("0x{:016x}", if budget >= 2 { VALUE_A } else { 0 })
        );
        assert_eq!(
            result["serial_bytes"],
            if budget >= 3 {
                serde_json::json!([VALUE_A as u8])
            } else {
                serde_json::json!([])
            }
        );
    }
    let duplicate_budget = run_with_args(&a, &["--budget", "1", "--budget", "16"]);
    assert_eq!(duplicate_budget.status.code(), Some(3));
    assert_eq!(json(&duplicate_budget)["error"]["kind"], "arguments");
    let malformed = root.join(format!("work/stage4/{suffix}-corrupted.bin"));
    let mut damaged = bytes.clone();
    damaged[0x6000] ^= 1;
    fs::write(&malformed, damaged).unwrap();
    assert_eq!(run(&malformed, None).status.code(), Some(4));
    let equal = build(&root, &format!("{suffix}-invalid",), INITIAL, INITIAL);
    assert_eq!(run(&equal, None).status.code(), Some(4));
    let _ = fs::remove_file(malformed);
    for tail in ["-a", "-a2", "-b", "-invalid"] {
        let _ = fs::remove_dir_all(root.join(format!("work/stage4/{suffix}{tail}")));
    }
}
