use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn recorded_fixture_replay_runs_from_its_recorded_working_directory() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let output_relative = format!("work/stage2/s2-42k-branch/replay-test-{run_id}");
    let input = repository.join("tests/fixtures/x86/stage2/s2_42k_branch_jz_taken_nop.case.json");
    let runner = env!("CARGO_BIN_EXE_od-runner");
    let recorded_cwd = std::env::temp_dir().canonicalize().unwrap();
    let reducer = Command::new(runner)
        .current_dir(&recorded_cwd)
        .args([
            "--reduce-branch-nops",
            input.to_str().unwrap(),
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

    let output_dir = repository.join(&output_relative);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(output_dir.join("report.json")).unwrap()).unwrap();
    let replay = &report["replay"];
    let cwd = replay["working_directory"].as_str().unwrap();
    assert_eq!(cwd, recorded_cwd.display().to_string());
    let source_revision = report["source_revision"].as_str().unwrap();
    assert_ne!(source_revision, "unavailable");
    assert!(source_revision.starts_with(&git_revision(&repository)));
    let replay_original = &replay["original"]["argv"];
    let argv = replay_original.as_array().unwrap();
    let executable = argv[0].as_str().unwrap();
    let replay_result = Command::new(executable)
        .current_dir(cwd)
        .args(argv[1..].iter().map(|value| value.as_str().unwrap()))
        .output()
        .unwrap();
    assert!(
        replay_result.status.success(),
        "saved replay failed: {}",
        String::from_utf8_lossy(&replay_result.stderr)
    );
    let replayed: serde_json::Value = serde_json::from_slice(&replay_result.stdout).unwrap();
    let callback: serde_json::Value =
        serde_json::from_slice(&fs::read(output_dir.join("initial-1.result.json")).unwrap())
            .unwrap();
    assert_eq!(
        replayed["input_sha256"],
        replay["original"]["fixture_sha256"]
    );
    assert_eq!(replayed["outcome"], callback["outcome"]);
    assert_eq!(
        replayed["instructions_executed"],
        callback["instructions_executed"]
    );
    assert_eq!(replayed["snapshot"], callback["snapshot"]);

    let reducer_argv = replay["reducer_argv"].as_array().unwrap();
    assert_eq!(reducer_argv[1], "--reduce-branch-nops");
    assert_eq!(report["reducer_version"], "s2-42k-branch-nop-v1");
    assert_eq!(report["oracle_version"], "s2-42k-jcc-full-state-oracle-v1");
    fs::remove_dir_all(output_dir).unwrap();
}

fn git_revision(repository: &std::path::Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
