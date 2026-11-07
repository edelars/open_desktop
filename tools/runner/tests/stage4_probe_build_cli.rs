use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PAYLOAD_A: u64 = 0x1122_3344_5566_7788;
const PAYLOAD_B: u64 = 0x8877_6655_4433_2211;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("runner manifest must be two levels below the repository root")
        .to_path_buf()
}

fn build_fixture(root: &Path, output_dir: &str, payload: u64) -> PathBuf {
    let output = Command::new("python3")
        .current_dir(root)
        .args([
            "tools/build/build_stage4_flat_probe.py",
            "--payload",
            &format!("0x{payload:016x}"),
            "--output-dir",
            output_dir,
        ])
        .output()
        .expect("python3 must be available for the freestanding fixture");
    assert!(
        output.status.success(),
        "fixture build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    root.join(output_dir).join("flat_probe.bin")
}

fn run_probe(image: &Path, budget: Option<u64>, nonce: u64) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    command.arg("--stage4-probe").arg(image);
    command.args(["--expect-nonce", &nonce.to_string()]);
    if let Some(budget) = budget {
        command.args(["--budget", &budget.to_string()]);
    }
    command.output().expect("runner process must start")
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("runner stdout must be JSON")
}

#[test]
fn built_artifacts_are_reproducible_payload_driven_and_fail_closed() {
    let root = repo_root();
    let suffix = format!("runner-build-cli-{}", std::process::id());
    let same_a = format!("work/stage4/{suffix}-a");
    let same_b = format!("work/stage4/{suffix}-b");
    let different = format!("work/stage4/{suffix}-different");
    let malformed = format!("work/stage4/{suffix}-malformed.bin");

    let image_a = build_fixture(&root, &same_a, PAYLOAD_A);
    let image_b = build_fixture(&root, &same_b, PAYLOAD_A);
    let image_different = build_fixture(&root, &different, PAYLOAD_B);
    let bytes_a = fs::read(&image_a).expect("first built image must be readable");
    let bytes_b = fs::read(&image_b).expect("second built image must be readable");
    assert_eq!(
        bytes_a, bytes_b,
        "same payload must produce identical raw images"
    );

    let first = run_probe(&image_a, None, PAYLOAD_A);
    let first_json = json(&first);
    assert!(first.status.success());
    assert_eq!(first_json["exit"], "halted");
    assert_eq!(first_json["serial_bytes"], serde_json::json!([0x88]));

    let second = run_probe(&image_different, None, PAYLOAD_B);
    let second_json = json(&second);
    assert!(second.status.success());
    assert_eq!(second_json["exit"], "halted");
    assert_eq!(second_json["serial_bytes"], serde_json::json!([0x11]));
    assert_ne!(first_json["serial_bytes"], second_json["serial_bytes"]);

    let mut corrupted = bytes_a;
    corrupted[0] ^= 0xff;
    let malformed_path = root.join(&malformed);
    fs::write(&malformed_path, corrupted).expect("malformed artifact must be writable");
    let rejected = run_probe(&malformed_path, None, PAYLOAD_A);
    let rejected_json = json(&rejected);
    assert_eq!(rejected.status.code(), Some(4));
    assert_eq!(rejected_json["status"], "loader_error");
    assert_eq!(rejected_json["error"]["kind"], "loader");

    let _ = fs::remove_file(malformed_path);
    for directory in [&same_a, &same_b, &different] {
        let _ = fs::remove_dir_all(root.join(directory));
    }
}

#[test]
fn uart_echo_fixture_is_reproducible_and_runs() {
    let root = repo_root();
    let names = [
        format!("work/stage4/echo-{}-a", std::process::id()),
        format!("work/stage4/echo-{}-b", std::process::id()),
    ];
    for name in &names {
        let output = Command::new("python3")
            .current_dir(&root)
            .args([
                "tools/build/build_stage4_flat_probe.py",
                "--profile",
                "uart-echo-probe",
                "--output-dir",
                name,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let a = root.join(&names[0]).join("flat_probe.bin");
    let b = root.join(&names[1]).join("flat_probe.bin");
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    let a_elf = root.join(&names[0]).join("flat_probe.elf");
    let b_elf = root.join(&names[1]).join("flat_probe.elf");
    assert_eq!(fs::read(&a_elf).unwrap(), fs::read(&b_elf).unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-uart-echo-probe")
        .arg(&a)
        .args(["--rx-byte", "255", "--expect-byte", "255"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(json(&output)["serial_bytes"], serde_json::json!([255]));
    for name in &names {
        let _ = fs::remove_dir_all(root.join(name));
    }
}
