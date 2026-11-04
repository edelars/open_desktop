use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn literal_fixture_reports_no_initial_mismatch_and_saved_replay_uses_fresh_output() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let relative = format!("work/stage2/s2-42o-sqrtss-fault/replay-test-{run_id}");
    let input = repository.join("tests/fixtures/x86/stage2/s2_42o_sqrtss_snan_fault.case.json");
    let runner = env!("CARGO_BIN_EXE_od-runner");
    let created = Command::new(runner)
        .current_dir(&repository)
        .args([
            "--reduce-sqrtss-fault",
            input.to_str().unwrap(),
            "--limit",
            "48",
            "--output-dir",
            &relative,
        ])
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "reducer failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let output = repository.join(&relative);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["stop_reason"], "no_initial_mismatch");
    assert_eq!(report["actual_callbacks"], 2);
    assert_eq!(report["native_execution"], "not_run");
    let cwd = report["replay"]["working_directory"].as_str().unwrap();
    let argv = report["replay"]["argv_original"].as_array().unwrap();
    let replay = Command::new(argv[0].as_str().unwrap())
        .current_dir(cwd)
        .args(argv[1..].iter().map(|v| v.as_str().unwrap()))
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "fixture replay failed: {}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replayed: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    let first: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("callbacks/run-000001.result.json")).unwrap())
            .unwrap();
    assert_eq!(replayed["input_sha256"], report["source_fixture_sha256"]);
    assert_eq!(replayed["outcome"], first["outcome"]);
    assert_eq!(replayed["snapshot"], first["snapshot"]);
    let argv = report["replay"]["argv_reducer_fresh_output"]
        .as_array()
        .unwrap();
    let fresh = Command::new(argv[0].as_str().unwrap())
        .current_dir(cwd)
        .args(argv[1..].iter().map(|v| v.as_str().unwrap()))
        .output()
        .unwrap();
    assert!(
        fresh.status.success(),
        "fresh reducer replay failed: {}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    let fresh_path = repository.join(
        report["replay"]["fresh_output_destination"]
            .as_str()
            .unwrap(),
    );
    let fresh_report: serde_json::Value =
        serde_json::from_slice(&fs::read(fresh_path.join("report.json")).unwrap()).unwrap();
    assert_eq!(fresh_report["stop_reason"], "no_initial_mismatch");
    fs::remove_dir_all(fresh_path).unwrap();
    fs::remove_dir_all(output).unwrap();
}
