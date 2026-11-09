//! Builder -> runner tests for `--stage4-pit-calibrate-probe` (S4-29d). They
//! build the real freestanding kernel with clang/lld, run it through the CLI and
//! compare the result with the expected values in the build manifest, which the
//! builder derives from the PIT formula and the port-tick rule.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("runner manifest must be two levels below the repository root")
        .to_path_buf()
}

/// Builds the kernel into a unique ignored directory; returns `probe.bin` and
/// the parsed build manifest.
fn build(name: &str, flags: &[&str]) -> (PathBuf, Value) {
    let root = repo_root();
    let dir = format!("work/stage4/pitcal-cli-{}-{name}", std::process::id());
    let _ = fs::remove_dir_all(root.join(&dir));
    let output = Command::new("python3")
        .current_dir(&root)
        .args([
            "tools/build/build_stage4_pit_calibrate_probe.py",
            "--output-dir",
            &dir,
        ])
        .args(flags)
        .output()
        .expect("python3 must be available for the freestanding fixture");
    assert!(
        output.status.success(),
        "fixture build failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = fs::read(root.join(&dir).join("build_manifest.json")).unwrap();
    (
        root.join(dir).join("probe.bin"),
        serde_json::from_slice(&manifest).unwrap(),
    )
}

fn cleanup(image: &Path) {
    let _ = fs::remove_dir_all(image.parent().unwrap());
}

fn run(image: &Path, options: &[&str]) -> (Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-pit-calibrate-probe")
        .arg(image)
        .args(options)
        .output()
        .expect("runner process must start");
    let json = serde_json::from_slice(&output.stdout).expect("runner stdout must be JSON");
    (output, json)
}

#[test]
fn built_kernel_calibrates_the_tsc_and_matches_the_manifest_expectation() {
    let (image, manifest) = build("positive", &[]);
    let (output, json) = run(&image, &[]);
    assert!(output.status.success(), "{json}");
    assert_eq!(json["profile"], "Stage4PitCalibrateProbeV1");
    assert_eq!(json["status"], "guest_completed");
    assert_eq!(json["exit"], "halted");
    assert_eq!(json["inject_no_port_tick"], false);
    let expected = &manifest["expected"];
    let (a, b) = (&expected["a"], &expected["b"]);
    let record = &json["record"];
    for (got, want) in [
        ("freq_khz_a", &a["freq_khz"]),
        ("delta_tsc_a", &a["delta_tsc"]),
        ("pitcnt", &a["pitcnt"]),
        ("tscmin", &a["tscmin"]),
        ("tscmax", &a["tscmax"]),
        ("freq_khz_b", &b["freq_khz"]),
        ("delta_tsc_b", &b["delta_tsc"]),
        ("stage1_passes", &b["stage1_passes"]),
        ("stage2_passes", &b["stage2_passes"]),
    ] {
        assert_eq!(&record[got], want, "{got}");
    }
    assert_eq!(
        (record["status_a"].clone(), record["status_b"].clone()),
        (0.into(), 0.into())
    );
    assert_eq!(json["port_b_reads"], expected["port_b_reads_total"]);
    assert_eq!(json["clock_ns"], expected["clock_ns"]);
    assert_eq!(json["serial_hex"], expected["serial_hex"]);
    assert_eq!(json["serial_bytes"], expected["serial_bytes"]);
    assert_eq!(json["pit"]["gate2"], true);
    assert_eq!(json["pit"]["channel2_reload"], 0xffff);
    assert!(json["steps"].as_u64().unwrap() < json["budget"].as_u64().unwrap());
    assert_eq!(json["steps"], json["retired"]);
    assert_eq!(json["error"], Value::Null);
    cleanup(&image);
}

#[test]
fn injected_regression_without_port_tick_exhausts_the_budget_and_fails() {
    let (image, manifest) = build("inject", &[]);
    let (ok_output, ok) = run(&image, &[]);
    assert!(ok_output.status.success());
    let (output, json) = run(&image, &["--inject-no-port-tick", "--budget", "50000"]);
    assert_eq!(output.status.code(), Some(5), "{json}");
    assert_eq!(json["status"], "work_limit");
    assert_eq!(json["exit"], "budget_exhausted");
    assert_eq!(json["error"]["kind"], "budget_exhausted");
    assert_eq!(json["inject_no_port_tick"], true);
    assert_eq!(
        (json["steps"].clone(), json["retired"].clone()),
        (50000.into(), 50000.into())
    );
    assert_eq!(json["clock_ns"], 0, "time never moves while polling");
    assert_eq!(json["record"], Value::Null);
    assert_eq!(json["serial_bytes"], serde_json::json!([]));
    // Distinguishable from the accepted run on (stop, steps, retired, reads).
    assert_ne!(json["exit"], ok["exit"]);
    assert_ne!(json["steps"], ok["steps"]);
    assert!(
        json["port_b_reads"].as_u64().unwrap()
            < manifest["expected"]["port_b_reads_total"].as_u64().unwrap()
    );
    cleanup(&image);
}

#[test]
fn the_omit_gate_variant_is_refused_by_the_pit_model() {
    let (image, manifest) = build("omit-gate", &["--omit-gate"]);
    assert_eq!(manifest["accepted_profile"], false);
    let (output, json) = run(&image, &[]);
    assert_eq!(output.status.code(), Some(5), "{json}");
    assert_eq!(json["status"], "guest_terminal");
    assert_eq!(json["exit"], "terminal");
    assert!(
        json["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("port I/O error")
    );
    assert_eq!(json["port_b_reads"], 0);
    assert_eq!(json["pit"]["gate2"], false);
    assert_eq!(json["pit"]["channel2_reload"], Value::Null);
    cleanup(&image);
}

#[test]
fn invalid_input_is_rejected_before_the_guest_runs() {
    let (image, _) = build("invalid", &[]);
    for options in [
        vec!["--budget", "2000001"],
        vec!["--budget", "x"],
        vec!["--bogus", "1"],
        vec!["--inject-no-port-tick", "--inject-no-port-tick"],
    ] {
        let (output, json) = run(&image, &options);
        assert_eq!(output.status.code(), Some(3), "{options:?}: {json}");
        assert_eq!(json["status"], "invalid_input");
    }
    let short = image.with_file_name("short.bin");
    fs::write(&short, [0_u8; 16]).unwrap();
    let (output, json) = run(&short, &[]);
    assert_eq!(output.status.code(), Some(3), "{json}");
    let (output, json) = run(&image.with_file_name("missing.bin"), &[]);
    assert_eq!(output.status.code(), Some(3), "{json}");
    // A well-sized image that breaks the fixed layout is a loader error.
    let mut bad = fs::read(&image).unwrap();
    bad[0x3000 - 0x1000] = 1;
    let bad_path = image.with_file_name("bad.bin");
    fs::write(&bad_path, bad).unwrap();
    let (output, json) = run(&bad_path, &[]);
    assert_eq!(output.status.code(), Some(4), "{json}");
    assert_eq!(json["status"], "loader_error");
    cleanup(&image);
}
