use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn run_recorded_argv(argv: &[String], working_directory: &str) -> std::process::Output {
    assert!(argv.len() >= 2, "replay argv is incomplete");
    Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(working_directory)
        .output()
        .expect("recorded replay executable should start")
}

#[test]
fn stack_reducer_records_runnable_original_and_reduced_shape_replays() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir.join("../..");
    let workspace = workspace.canonicalize().unwrap();
    let fixture = workspace.join("tests/fixtures/x86/stage2/s2_17a_push_rax.case.json");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output_relative = format!("work/stage2/s2-42g-stack/replay-test-{nonce}");
    let output_dir = workspace.join(&output_relative);
    let executable = std::path::PathBuf::from(env!("CARGO_BIN_EXE_od-runner"))
        .canonicalize()
        .unwrap();

    let reducer = Command::new(&executable)
        .current_dir(&workspace)
        .args([
            "--reduce-push-pop",
            fixture.to_str().unwrap(),
            "--limit",
            "4",
            "--output-dir",
            &output_relative,
        ])
        .output()
        .unwrap();
    assert!(
        reducer.status.success(),
        "reducer failed: {}",
        String::from_utf8_lossy(&reducer.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(output_dir.join("report.json")).unwrap()).unwrap();
    let replay = &report["replay"];
    let working_directory = replay["working_directory"].as_str().unwrap();
    assert_eq!(
        std::path::Path::new(working_directory)
            .canonicalize()
            .unwrap(),
        workspace
    );
    let argv_original = replay["argv_original"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(std::path::Path::new(&argv_original[0]), executable);
    let original = run_recorded_argv(&argv_original, working_directory);
    assert!(
        original.status.success(),
        "original replay failed: {}",
        String::from_utf8_lossy(&original.stderr)
    );
    let original_result: serde_json::Value = serde_json::from_slice(&original.stdout).unwrap();
    assert_eq!(original_result["case_id"], "s2_17a_push_rax");

    // This fixture is a byte-for-byte copy that exercises the same argv shape
    // emitted for reduced.case.json when the reducer accepts a candidate.
    let reduced_path = output_dir.join("reduced.case.json");
    fs::copy(output_dir.join("original.case.json"), &reduced_path).unwrap();
    let mut reduced_argv = argv_original;
    reduced_argv[2] = reduced_path.display().to_string();
    let reduced = run_recorded_argv(&reduced_argv, working_directory);
    assert!(
        reduced.status.success(),
        "reduced-shape replay failed: {}",
        String::from_utf8_lossy(&reduced.stderr)
    );
    let reduced_result: serde_json::Value = serde_json::from_slice(&reduced.stdout).unwrap();
    assert_eq!(reduced_result["case_id"], "s2_17a_push_rax");

    fs::remove_dir_all(output_dir).unwrap();
}
