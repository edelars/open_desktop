//! `od-runner display-probe` (S10-08). Expected values come from the builder's manifest and
//! the reference raw frame in the fixtures, never from the runner's own output.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const IMAGE: &str = "tests/fixtures/guest/stage10/display_probe_bars_64x48_seed0.img";
const MANIFEST: &str = "tests/fixtures/guest/stage10/display_probe_bars_64x48_seed0.json";
const REFERENCE: &str = "tests/fixtures/guest/stage10/bars_64x48_seed0.raw";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("runner manifest must be two levels below the repository root")
        .to_path_buf()
}

fn run(args: &[&str]) -> (Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .current_dir(repo_root())
        .arg("display-probe")
        .args(args)
        .output()
        .expect("runner process must start");
    let json = serde_json::from_slice(&output.stdout).expect("runner stdout must be JSON");
    (output, json)
}

fn manifest() -> Value {
    serde_json::from_slice(&fs::read(repo_root().join(MANIFEST)).unwrap()).unwrap()
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("od-display-probe-{}-{name}", std::process::id()))
}

#[test]
fn display_probe_reports_the_manifest_values_and_saves_the_reference_frame() {
    let manifest = manifest();
    let frame_path = temp_path("frame.raw");
    let _ = fs::remove_file(&frame_path);
    let (output, report) = run(&[
        "--image",
        IMAGE,
        "--frame-out",
        frame_path.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{report}");
    assert_eq!(report["status"], "completed");
    assert_eq!(report["outcome"], "halted");
    assert_eq!(report["retired"], manifest["expected_retired"]);
    assert_eq!(report["generation"], manifest["expected_generation"]);
    assert_eq!(report["status_after_commit"], "0x00000001");
    assert_eq!(report["status_after_frame"], "0x00000003");
    assert_eq!(report["mode"]["width"], manifest["width"]);
    assert_eq!(report["mode"]["height"], manifest["height"]);
    assert_eq!(report["mode"]["stride"], manifest["stride"]);
    assert_eq!(report["frame_sha256"], manifest["expected_frame_sha256"]);
    let saved = fs::read(&frame_path).unwrap();
    assert_eq!(saved, fs::read(repo_root().join(REFERENCE)).unwrap());
    let _ = fs::remove_file(&frame_path);
}

#[test]
fn display_probe_without_frame_out_writes_no_file_and_still_reports_the_hash() {
    let (output, report) = run(&["--image", IMAGE]);
    assert_eq!(output.status.code(), Some(0), "{report}");
    assert_eq!(report["frame_out"], Value::Null);
    assert_eq!(report["frame_sha256"], manifest()["expected_frame_sha256"]);
}

#[test]
fn display_probe_with_a_small_budget_reports_an_unfinished_guest() {
    let (output, report) = run(&["--image", IMAGE, "--budget", "100"]);
    assert_eq!(output.status.code(), Some(5), "{report}");
    assert_eq!(report["status"], "error");
    assert_eq!(report["outcome"], "budget_exhausted");
    assert_eq!(report["retired"], 100);
    assert_eq!(report["frame_sha256"], Value::Null);
}

#[test]
fn display_probe_rejects_bad_arguments_and_images() {
    let (output, report) = run(&[]);
    assert_eq!(output.status.code(), Some(3), "{report}");
    let (output, report) = run(&["--image", IMAGE, "--frame-out", IMAGE]);
    assert_eq!(output.status.code(), Some(3), "{report}");
    assert_eq!(report["error"]["kind"], "paths");
    let (output, report) = run(&["--image", "tests/fixtures/guest/stage10/no-such.img"]);
    assert_eq!(output.status.code(), Some(3), "{report}");
    let corrupt = temp_path("corrupt.img");
    let mut bytes = fs::read(repo_root().join(IMAGE)).unwrap();
    bytes[0x41] ^= 0xFF;
    fs::write(&corrupt, bytes).unwrap();
    let (output, report) = run(&["--image", corrupt.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(4), "{report}");
    assert_eq!(report["status"], "loader_error");
    let _ = fs::remove_file(&corrupt);
}
