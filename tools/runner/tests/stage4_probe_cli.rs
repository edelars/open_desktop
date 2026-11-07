use od_isa::INTEGER_FLAGS;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn put_entry(image: &mut [u8], guest_physical_address: u64, value: u64) {
    let offset = usize::try_from(guest_physical_address - 0x1000).unwrap();
    image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn valid_image(payload: u64) -> Vec<u8> {
    let mut image = vec![0; 0x9000];
    for (address, value) in [
        (0x1000, 0x2003),
        (0x2000, 0x3003),
        (0x3010, 0x4003),
        (0x3020, 0x5003),
        (0x3028, 0x6003),
        (0x4000, 0x7001),
        (0x5000, 0x9003 | (1 << 63)),
        (0x6000, 0xa003 | (1 << 63)),
    ] {
        put_entry(&mut image, address, value);
    }
    let code_offset = usize::try_from(0x7000_u64 - 0x1000).unwrap();
    image[code_offset..code_offset + 5].copy_from_slice(&[0x48, 0x8b, 0x03, 0xee, 0xf4]);
    let data_offset = usize::try_from(0x9000_u64 - 0x1000).unwrap();
    image[data_offset..data_offset + 8].copy_from_slice(&payload.to_le_bytes());
    image
}

fn invoke(image: &[u8], options: &[&str]) -> (Output, PathBuf) {
    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "od-runner-stage4-probe-{}-{id}.raw",
        std::process::id()
    ));
    fs::write(&path, image).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-probe")
        .arg(&path)
        .args(options)
        .output()
        .unwrap();
    (output, path)
}

fn result(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn valid_image_runs_to_hlt_and_reports_guest_uart_output() {
    let (output, path) = invoke(
        &valid_image(0x1122_3344_5566_77a5),
        &["--expect-nonce", "0x11223344556677a5"],
    );
    let json = result(&output);
    let _ = fs::remove_file(path);

    assert!(output.status.success());
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["profile"], "Stage4FlatProbeV1");
    assert_eq!(json["status"], "guest_completed");
    assert_eq!(json["exit"], "halted");
    assert_eq!(json["retired"], 3);
    assert_eq!(json["rip"], "0x0000000000400005");
    assert_eq!(json["activity"], "Halted");
    assert_eq!(json["rflags_defined"], format!("0x{INTEGER_FLAGS:016x}"));
    assert_eq!(json["observed_data_qword"], "0x11223344556677a5");
    assert_eq!(json["serial_bytes"], serde_json::json!([0xa5]));
    assert!(json["error"].is_null());
}

#[test]
fn output_byte_changes_with_guest_payload() {
    let (first, first_path) = invoke(&valid_image(0x11), &["--expect-nonce", "17"]);
    let first_json = result(&first);
    let _ = fs::remove_file(first_path);
    let (second, second_path) = invoke(&valid_image(0x92), &["--expect-nonce", "0x92"]);
    let second_json = result(&second);
    let _ = fs::remove_file(second_path);

    assert!(first.status.success() && second.status.success());
    assert_eq!(first_json["serial_bytes"], serde_json::json!([0x11]));
    assert_eq!(second_json["serial_bytes"], serde_json::json!([0x92]));
    assert_ne!(first_json["serial_bytes"], second_json["serial_bytes"]);
    assert_ne!(second_json["serial_bytes"], serde_json::json!([0x11]));
}

#[test]
fn budget_exhaustion_returns_nonzero_json_with_partial_state() {
    let (output, path) = invoke(&valid_image(0x5a), &["--budget", "1"]);
    let json = result(&output);
    let _ = fs::remove_file(path);

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json["status"], "work_limit");
    assert_eq!(json["exit"], "budget_exhausted");
    assert_eq!(json["retired"], 1);
    assert_eq!(json["serial_bytes"], serde_json::json!([]));
    assert_eq!(json["error"]["kind"], "budget_exhausted");
    assert_eq!(json["rflags_defined"], format!("0x{INTEGER_FLAGS:016x}"));
}

#[test]
fn invalid_size_budget_and_loader_layout_have_distinct_results() {
    let (size_output, size_path) = invoke(&[0; 3], &[]);
    let size_json = result(&size_output);
    let _ = fs::remove_file(size_path);
    assert_eq!(size_output.status.code(), Some(3));
    assert_eq!(size_json["error"]["kind"], "image_size");

    let (budget_output, budget_path) = invoke(&valid_image(0), &["--budget", "1025"]);
    let budget_json = result(&budget_output);
    let _ = fs::remove_file(budget_path);
    assert_eq!(budget_output.status.code(), Some(3));
    assert_eq!(budget_json["error"]["kind"], "arguments");

    let mut malformed_layout = valid_image(0);
    malformed_layout[0] = 0xff;
    let (loader_output, loader_path) = invoke(&malformed_layout, &[]);
    let loader_json = result(&loader_output);
    let _ = fs::remove_file(loader_path);
    assert_eq!(loader_output.status.code(), Some(4));
    assert_eq!(loader_json["status"], "loader_error");
    assert_eq!(loader_json["error"]["kind"], "loader");
}

#[test]
fn missing_file_returns_json_error_without_panicking() {
    let path = std::env::temp_dir().join(format!(
        "od-runner-stage4-probe-missing-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-probe")
        .arg(path)
        .output()
        .unwrap();
    let json = result(&output);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(json["status"], "invalid_input");
    assert_eq!(json["error"]["kind"], "file");
}

#[test]
fn completion_requires_matching_expectation() {
    for (args, status) in [
        (&[][..], "guest_halted"),
        (&["--expect-nonce", "0x42"][..], "completion_mismatch"),
    ] {
        let (output, path) = invoke(&valid_image(0x41), args);
        let json = result(&output);
        let _ = fs::remove_file(path);
        assert_eq!(output.status.code(), Some(5));
        assert_eq!(json["status"], status);
        assert_eq!(json["exit"], "halted");
        assert_eq!(json["serial_bytes"], serde_json::json!([0x41]));
    }
}

#[test]
fn cancellation_preserves_committed_prefix() {
    for (limit, retired, serial, observed) in [
        ("0", 0, serde_json::json!([]), Value::Null),
        (
            "1",
            1,
            serde_json::json!([]),
            serde_json::json!("0x0000000000000041"),
        ),
        (
            "2",
            2,
            serde_json::json!([0x41]),
            serde_json::json!("0x0000000000000041"),
        ),
    ] {
        let (output, path) = invoke(
            &valid_image(0x41),
            &["--cancel-after", limit, "--expect-nonce", "65"],
        );
        let json = result(&output);
        let _ = fs::remove_file(path);
        assert_eq!(output.status.code(), Some(5));
        assert_eq!(json["status"], "host_cancelled");
        assert_eq!(json["exit"], "stopped");
        assert_eq!(json["retired"], retired);
        assert_eq!(json["serial_bytes"], serial);
        assert_eq!(json["observed_data_qword"], observed);
    }
}

#[test]
fn invalid_expectation_and_cancellation_are_invalid_input() {
    for args in [
        &["--expect-nonce", "0x10000000000000000"][..],
        &["--cancel-after", "-1"][..],
        &["--expect-nonce", "1", "--expect-nonce", "2"][..],
    ] {
        let (output, path) = invoke(&valid_image(0), args);
        let _ = fs::remove_file(path);
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(result(&output)["status"], "invalid_input");
    }
}
