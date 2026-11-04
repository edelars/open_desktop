use std::fs;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

const BASE: &[u8] =
    include_bytes!("../../../tests/fixtures/x86/stage2/s2_11_sub_64_reg_mem.case.json");

fn temp_dir() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "od-runner-s2-43a-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn binary(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(args)
        .output()
        .unwrap()
}

fn make_fixture(code: &str, data_permissions: Value, initial_rax: &str) -> Vec<u8> {
    let mut fixture: Value = serde_json::from_slice(BASE).unwrap();
    fixture["case_id"] = json!("s2_43a_diagnostic_test");
    fixture["initial_state"]["gpr"][0] = json!(initial_rax);
    fixture["initial_state"]["gpr"][3] = json!("0x0000000000200000");
    fixture["stop_rip"] = json!(format!("0x{:016x}", 0x10_0000 + code.len() / 2));
    fixture["instruction_budget"] = json!(32);
    fixture["expected_exit"] = json!("stop_address");
    fixture["memory"] = json!([
        {
            "region_id": "code",
            "guest_address": "0x0000000000100000",
            "length": code.len() / 2,
            "permissions": {"read": true, "write": false, "execute": true},
            "initial_bytes_hex": code,
            "comparison_mask_hex": "ff".repeat(code.len() / 2)
        },
        {
            "region_id": "data",
            "guest_address": "0x0000000000200000",
            "length": 64,
            "permissions": data_permissions,
            "initial_bytes_hex": "0100000000000000".to_owned() + &"00".repeat(56),
            "comparison_mask_hex": "ff".repeat(64)
        }
    ]);
    serde_json::to_vec(&fixture).unwrap()
}

fn diagnostic(bytes: &[u8], options: &[&str]) -> (std::path::PathBuf, Output) {
    let dir = temp_dir();
    let fixture = dir.join("case.json");
    fs::write(&fixture, bytes).unwrap();
    let mut args = vec!["--diagnostic", fixture.to_str().unwrap()];
    args.extend_from_slice(options);
    (dir, binary(&args))
}

fn output_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn traces_load_add_store_and_carries_definedness_between_steps() {
    let fixture = make_fixture(
        "488b034883c00148894308",
        json!({"read": true, "write": true, "execute": false}),
        "0x0000000000000000",
    );
    let (dir, output) = diagnostic(&fixture, &["--steps", "3"]);
    let trace = output_json(&output);
    let records = trace["result"]["records"].as_array().unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(
        records[1]["rflags_defined_before"],
        records[0]["rflags_defined_after"]
    );
    assert_eq!(records[2]["memory_effects"][0]["region_id"], "data");
    assert_eq!(
        records[2]["memory_effects"][0]["address"],
        "0x0000000000200008"
    );
    assert_eq!(records[2]["memory_effects"][0]["after"], "02");
    fs::remove_dir_all(dir).unwrap();

    let mul_then_branch = make_fixture(
        "48f7e374029090",
        json!({"read": true, "write": true, "execute": false}),
        "0x0000000000000003",
    );
    let (dir, output) = diagnostic(&mul_then_branch, &["--steps", "2"]);
    let trace = output_json(&output);
    let records = trace["result"]["records"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0]["rflags_defined_after"], "0x0000000000000cd7");
    assert_eq!(
        records[1]["rflags_defined_before"],
        records[0]["rflags_defined_after"]
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn records_taken_branch_and_faulting_fetch_and_store_without_commit() {
    let writable = json!({"read": true, "write": true, "execute": false});
    let branch = make_fixture("74029090", writable.clone(), "0x0000000000000000");
    let (branch_dir, branch_output) = diagnostic(&branch, &["--steps", "1"]);
    let branch_trace = output_json(&branch_output);
    assert_eq!(
        branch_trace["result"]["records"][0]["after"]["rip"],
        "0x0000000000100004"
    );
    fs::remove_dir_all(branch_dir).unwrap();

    let fetch_fault = make_fixture("90", writable.clone(), "0x0000000000000000");
    let mut fetch_fault: Value = serde_json::from_slice(&fetch_fault).unwrap();
    fetch_fault["memory"][0]["permissions"]["execute"] = json!(false);
    let (fetch_dir, fetch_output) = diagnostic(
        &serde_json::to_vec(&fetch_fault).unwrap(),
        &["--steps", "1"],
    );
    let fetch_trace = output_json(&fetch_output);
    let fetch_record = &fetch_trace["result"]["records"][0];
    assert_eq!(fetch_record["faulting_rip"], "0x0000000000100000");
    assert_eq!(fetch_record["before"], fetch_record["after"]);
    fs::remove_dir_all(fetch_dir).unwrap();

    let store_fault = make_fixture(
        "488903",
        json!({"read": true, "write": false, "execute": false}),
        "0x0000000000000007",
    );
    let (store_dir, store_output) = diagnostic(&store_fault, &["--steps", "1"]);
    let store_trace = output_json(&store_output);
    let store_record = &store_trace["result"]["records"][0];
    assert_eq!(store_record["faulting_rip"], "0x0000000000100000");
    assert_eq!(store_record["memory_effects"], json!([]));
    fs::remove_dir_all(store_dir).unwrap();
}

#[test]
fn zero_and_exact_limits_replay_and_reject_tampering() {
    let fixture = make_fixture(
        "488903",
        json!({"read": true, "write": true, "execute": false}),
        "0x0000000000000007",
    );
    let (zero_dir, zero_output) = diagnostic(&fixture, &["--steps", "0", "--max-event-bytes", "0"]);
    let zero = output_json(&zero_output);
    assert_eq!(zero["result"]["records"], json!([]));
    assert_eq!(zero["result"]["limits"]["max_event_bytes"], 0);
    fs::remove_dir_all(zero_dir).unwrap();

    let (zero_event_dir, zero_event_output) =
        diagnostic(&fixture, &["--steps", "1", "--max-event-bytes", "0"]);
    let zero_event = output_json(&zero_event_output);
    let zero_event_record = &zero_event["result"]["records"][0];
    assert_eq!(zero_event["result"]["event_bytes"], 0);
    assert_eq!(zero_event_record["memory_effects"], json!([]));
    assert_eq!(zero_event_record["memory_effects_truncated"], true);
    fs::remove_dir_all(zero_event_dir).unwrap();

    let (dir, output) = diagnostic(&fixture, &["--steps", "1", "--max-event-bytes", "2"]);
    let trace = output_json(&output);
    assert_eq!(trace["result"]["records"].as_array().unwrap().len(), 1);
    assert_eq!(trace["result"]["event_bytes"], 2);
    let replay = dir.join("trace.json");
    fs::write(&replay, &output.stdout).unwrap();
    let replayed = binary(&["--diagnostic-replay", replay.to_str().unwrap()]);
    assert!(
        replayed.status.success(),
        "{}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    let mut tampered: Value = serde_json::from_slice(&output.stdout).unwrap();
    tampered["result"]["records"][0]["retired"] = json!(9);
    fs::write(&replay, serde_json::to_vec(&tampered).unwrap()).unwrap();
    let rejected = binary(&["--diagnostic-replay", replay.to_str().unwrap()]);
    assert!(!rejected.status.success());
    fs::write(&replay, b"{not json").unwrap();
    let malformed = binary(&["--diagnostic-replay", replay.to_str().unwrap()]);
    assert!(!malformed.status.success());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn denies_other_profiles_and_jit_without_changing_regular_fixture_path() {
    let fixture = make_fixture(
        "90",
        json!({"read": true, "write": true, "execute": false}),
        "0",
    );
    let (dir, diagnostic_output) = diagnostic(&fixture, &["--engine", "jit"]);
    assert!(!diagnostic_output.status.success());
    fs::remove_dir_all(dir).unwrap();

    let dir = temp_dir();
    let path = dir.join("case.json");
    fs::write(&path, BASE).unwrap();
    let regular = binary(&[
        "--fixture",
        path.to_str().unwrap(),
        "--engine",
        "interpreter",
        "--json",
    ]);
    assert!(
        regular.status.success(),
        "{}",
        String::from_utf8_lossy(&regular.stderr)
    );
    assert!(serde_json::from_slice::<Value>(&regular.stdout).is_ok());
    let (diagnostic_dir, diagnostic_output) = diagnostic(BASE, &["--steps", "1"]);
    let mut other_profile: Value = serde_json::from_slice(BASE).unwrap();
    other_profile["profile"] = json!("s2-long-cli-v1");
    let altered = serde_json::to_vec(&other_profile).unwrap();
    let path2 = dir.join("other.json");
    fs::write(&path2, altered).unwrap();
    let denied = binary(&["--diagnostic", path2.to_str().unwrap()]);
    assert!(!denied.status.success());
    let _ = diagnostic_output;
    fs::remove_dir_all(diagnostic_dir).unwrap();
    fs::remove_dir_all(dir).unwrap();
}
