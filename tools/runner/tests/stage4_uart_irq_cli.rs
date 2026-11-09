//! Builder -> runner tests for `--stage4-uart-irq-probe` (S4-24e). They build
//! the real freestanding kernel with clang/lld and run it through the CLI.

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

/// Builds the kernel into a unique ignored directory and returns `probe.bin`.
fn build(name: &str, flags: &[&str]) -> PathBuf {
    let root = repo_root();
    let dir = format!("work/stage4/uirq-cli-{}-{name}", std::process::id());
    let _ = fs::remove_dir_all(root.join(&dir));
    let output = Command::new("python3")
        .current_dir(&root)
        .args([
            "tools/build/build_stage4_uart_irq_probe.py",
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
    root.join(dir).join("probe.bin")
}

fn cleanup(image: &Path) {
    let _ = fs::remove_dir_all(image.parent().unwrap());
}

fn run(image: &Path, options: &[&str]) -> (Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-uart-irq-probe")
        .arg(image)
        .args(options)
        .output()
        .expect("runner process must start");
    let json = serde_json::from_slice(&output.stdout).expect("runner stdout must be JSON");
    (output, json)
}

fn events(json: &Value) -> Vec<String> {
    json["trace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn built_kernel_handles_rx_irq_through_ioapic_and_lapic() {
    let image = build("positive", &[]);
    let (output, json) = run(&image, &["--rx", "0x41@1000", "--expect-echo", "0x41"]);
    assert!(output.status.success(), "{json}");
    assert_eq!(json["profile"], "Stage4UartIrqProbeV1");
    assert_eq!(json["status"], "guest_completed");
    assert_eq!(json["exit"], "idle_halt");
    assert_eq!(json["serial_bytes"], serde_json::json!([0x41]));
    assert_eq!(json["counts"]["irr"], 1);
    assert_eq!(json["counts"]["irq_entry"], 1);
    assert_eq!(json["counts"]["eoi"], 1);
    assert_eq!(json["counts"]["iretq"], 1);
    assert_eq!(json["apic"]["irr"], false);
    assert_eq!(json["apic"]["isr"], false);
    assert_eq!(json["activity"], "Halted");
    assert_eq!(json["rsp"], "0x0000000000006ff0");
    assert_eq!(json["clock_ns"], 1000);
    assert!(json["quanta"].as_u64().unwrap() < json["budget"].as_u64().unwrap());
    assert_eq!(json["uart"]["iir"], 1);
    assert_eq!(json["uart"]["interrupt_line"], false);
    let e = events(&json);
    // Setup output (UART programming) precedes STI; the handler's own port
    // accesses follow the interrupt entry.
    let mut cursor = 0;
    for name in [
        "lidt",
        "sti",
        "hlt",
        "idle_advance",
        "rx_delivered",
        "uart_line",
        "irr",
        "irq_entry",
        "port_in",
        "port_in",
        "uart_line",
        "port_out",
        "eoi",
        "iretq",
        "hlt",
    ] {
        let found = e[cursor..]
            .iter()
            .position(|event| event == name)
            .unwrap_or_else(|| panic!("{name} missing after index {cursor} in {e:?}"));
        cursor += found + 1;
    }
    assert_eq!(e.last().map(String::as_str), Some("hlt"));

    // IIR then RBR are read by the handler: RDA (0x04) then the byte itself.
    let ports: Vec<(String, u64, String)> = json["trace"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|ev| ev["event"] == "port_in" || ev["event"] == "port_out")
        .filter(|ev| ev["rip"].as_str().unwrap() >= "0x0000000000004060")
        .map(|ev| {
            (
                ev["event"].as_str().unwrap().into(),
                ev["value"].as_u64().unwrap(),
                ev["port"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        ports,
        [
            ("port_in".to_string(), 4, "0x3fa".to_string()),
            ("port_in".to_string(), 0x41, "0x3f8".to_string()),
            ("port_out".to_string(), 0x41, "0x3f8".to_string()),
        ]
    );
    cleanup(&image);
}

#[test]
fn two_bytes_echo_in_order_and_split_budget_is_reported() {
    let image = build("two", &[]);
    let args = [
        "--rx",
        "0x41@1000",
        "--rx",
        "0x42@2000",
        "--expect-echo",
        "0x41",
        "--expect-echo",
        "0x42",
    ];
    let (output, json) = run(&image, &args);
    assert!(output.status.success(), "{json}");
    assert_eq!(json["serial_bytes"], serde_json::json!([0x41, 0x42]));
    assert_eq!(json["counts"]["irq_entry"], 2);
    assert_eq!(json["counts"]["eoi"], 2);

    // A budget smaller than the run is a work-limit failure with a report.
    let mut limited = args.to_vec();
    limited.extend(["--budget", "5"]);
    let (output, json) = run(&image, &limited);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json["status"], "work_limit");
    assert_eq!(json["exit"], "budget_exhausted");
    assert_eq!(json["quanta"], 5);
    assert_eq!(json["error"]["kind"], "budget_exhausted");

    // STOP is polled before each step.
    let mut cancelled = args.to_vec();
    cancelled.extend(["--cancel-after", "3"]);
    let (output, json) = run(&image, &cancelled);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json["status"], "host_cancelled");
    assert_eq!(json["quanta"], 3);
    cleanup(&image);
}

#[test]
fn ier_zero_guest_gets_no_irq() {
    let image = build("ier0", &["--ier", "0"]);
    let (output, json) = run(&image, &["--rx", "0x41@1000", "--expect-irqs", "0"]);
    assert!(output.status.success(), "{json}");
    assert_eq!(json["status"], "guest_completed");
    assert_eq!(json["counts"]["rx_delivered"], 1);
    assert_eq!(json["counts"]["irq_entry"], 0);
    assert_eq!(json["counts"]["irr"], 0);
    assert_eq!(json["serial_bytes"], serde_json::json!([]));
    assert_eq!(json["uart"]["rbr_pending"], 0x41);
    assert!(!events(&json).contains(&"uart_line".to_string()));
    // The same run does not satisfy the positive expectation.
    let (output, json) = run(&image, &["--rx", "0x41@1000", "--expect-echo", "0x41"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json["status"], "completion_mismatch");
    cleanup(&image);
}

#[test]
fn masked_intin4_edge_is_lost_and_late_unmask_does_not_deliver() {
    let image = build("late", &["--late-unmask"]);
    let (output, json) = run(&image, &["--rx", "0x41@0", "--expect-irqs", "0"]);
    assert!(output.status.success(), "{json}");
    assert_eq!(json["counts"]["irr"], 0);
    assert_eq!(json["counts"]["irq_entry"], 0);
    assert_eq!(json["uart"]["interrupt_line"], true);
    assert_eq!(json["uart"]["rbr_pending"], 0x41);
    let e = events(&json);
    assert!(e.contains(&"uart_line".to_string()));
    assert!(!e.contains(&"irr".to_string()));
    cleanup(&image);
}

#[test]
fn injected_regressions_fail_the_positive_expectation() {
    let args = [
        "--rx",
        "0x41@1000",
        "--rx",
        "0x42@2000",
        "--expect-echo",
        "0x41",
        "--expect-echo",
        "0x42",
    ];
    // No EOI: the second IRQ latches in the IRR but is never delivered.
    let image = build("noeoi", &["--omit-eoi"]);
    let (output, json) = run(&image, &args);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json["status"], "completion_mismatch");
    assert_eq!(json["counts"]["irr"], 2);
    assert_eq!(json["counts"]["irq_entry"], 1);
    assert_eq!(json["counts"]["eoi"], 0);
    assert_eq!(json["apic"]["irr"], true);
    assert_eq!(json["apic"]["isr"], true);
    assert_eq!(json["serial_bytes"], serde_json::json!([0x41]));
    cleanup(&image);

    // No RBR read: the handler echoes IIR and the receive register stays full.
    let image = build("nobr", &["--omit-rbr-read"]);
    let (output, json) = run(&image, &args);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json["status"], "completion_mismatch");
    assert_eq!(json["serial_bytes"], serde_json::json!([4]));
    assert_eq!(json["uart"]["rbr_pending"], 0x41);
    assert_eq!(json["pending_rx"], 1);
    cleanup(&image);
}

#[test]
fn builds_are_reproducible_and_loader_and_arguments_fail_closed() {
    let a = build("repro-a", &[]);
    let b = build("repro-b", &[]);
    let bytes = fs::read(&a).unwrap();
    assert_eq!(bytes, fs::read(&b).unwrap());
    assert_eq!(bytes.len(), 0x6000);
    cleanup(&b);

    let mut corrupted = bytes;
    corrupted[0x1000] ^= 0xff; // GDT
    let bad = a.with_file_name("corrupt.bin");
    fs::write(&bad, corrupted).unwrap();
    let (output, json) = run(&bad, &[]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(json["status"], "loader_error");

    let short = a.with_file_name("short.bin");
    fs::write(&short, [0_u8; 16]).unwrap();
    let (output, json) = run(&short, &[]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(json["status"], "invalid_input");

    for options in [
        vec!["--rx", "0x41"],
        vec!["--rx", "0x141@10"],
        vec!["--budget", "100001"],
        vec!["--bogus", "1"],
        vec!["--budget"],
    ] {
        let (output, json) = run(&a, &options);
        assert_eq!(output.status.code(), Some(3), "{options:?}");
        assert_eq!(json["status"], "invalid_input");
    }
    cleanup(&a);
}
