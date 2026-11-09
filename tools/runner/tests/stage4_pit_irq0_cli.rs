//! Builder -> runner tests for `--stage4-pit-irq0-probe` (S4-29f). They build the
//! real freestanding kernel with clang/lld and run it through the CLI. Expected
//! values are read from the builder's manifest (derived from the PIT formula and
//! the bridge rules), never from the runner's output.

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

/// Builds the kernel into a unique ignored directory; returns `probe.bin` and the
/// builder's manifest.
fn build(name: &str, flags: &[&str]) -> (PathBuf, Value) {
    let root = repo_root();
    let dir = format!("work/stage4/pit0-cli-{}-{name}", std::process::id());
    let _ = fs::remove_dir_all(root.join(&dir));
    let output = Command::new("python3")
        .current_dir(&root)
        .args([
            "tools/build/build_stage4_pit_irq0_probe.py",
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
        .arg("--stage4-pit-irq0-probe")
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

/// The pump events of the interrupt path, without owner events.
fn core_events(json: &Value) -> Vec<String> {
    const CORE: [&str; 7] = ["sti", "hlt", "irr", "entry", "eoi", "iretq", "coalesced"];
    events(json)
        .into_iter()
        .filter(|name| CORE.contains(&name.as_str()))
        .collect()
}

fn expected_events(manifest: &Value) -> Vec<String> {
    manifest["expected"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_string())
        .collect()
}

fn hex(value: &str) -> String {
    format!("0x{:016x}", u64::from_str_radix(&value[2..], 16).unwrap())
}

#[test]
fn built_kernel_gets_n_irq0_through_gsi2_and_the_run_matches_the_pit_formula() {
    let (image, manifest) = build("accept", &[]);
    let expected = &manifest["expected"];
    let (output, json) = run(&image, &[]);
    assert_eq!(output.status.code(), Some(0), "{json}");
    assert_eq!(json["status"], "guest_idle");
    assert_eq!(json["exit"], "idle_halt");
    assert_eq!(json["profile"], "Stage4PitIrq0ProbeV1");
    // N interrupts through INTIN2, each with its own EOI; counter and time from the formula.
    assert_eq!(json["wakeups"], expected["wakeups"]);
    assert_eq!(json["counter"], expected["counter"]);
    assert_eq!(json["clock_ns"], expected["time_ns"]);
    assert_eq!(json["entry_times_ns"], expected["wake_times_ns"]);
    assert_eq!(json["counts"]["irq_entry"], expected["wakeups"]);
    assert_eq!(json["counts"]["eoi"], expected["wakeups"]);
    assert_eq!(json["counts"]["iretq"], expected["wakeups"]);
    assert_eq!(core_events(&json), expected_events(&manifest));
    // Quiescent end state: vector clear, halted after the HLT, stack back at the top.
    assert_eq!(json["apic"]["vector"], 0x60);
    assert_eq!(json["apic"]["irr"], false);
    assert_eq!(json["apic"]["isr"], false);
    assert_eq!(json["rip"], hex(expected["final"]["rip"].as_str().unwrap()));
    assert_eq!(json["rsp"], hex(expected["final"]["rsp"].as_str().unwrap()));
    assert_eq!(
        json["rflags"],
        hex(expected["final"]["rflags"].as_str().unwrap())
    );
    // Bridge: generations, dropped events, edges.
    for key in ["edges", "scheduled", "generation_changes", "cancelled"] {
        assert_eq!(json["bridge"][key], expected["bridge"][key], "{key}");
    }
    assert_eq!(json["bridge"]["pending_events"], 0);
    assert_eq!(json["pit"]["channel0_count"], Value::Null, "shut down");
    // PIC IR0 got nothing: no vector 0x30 event, IRR read 0.
    assert_eq!(json["pic"]["events"], 0);
    assert_eq!(json["pic"]["irr"], false);
    assert_eq!(json["pic"]["isr"], false);
    assert_eq!(json["pic"]["irr_reads"], serde_json::json!([0]));
    assert_eq!(json["pic"]["master_phase"], "Initialized");
    assert_eq!(json["pic"]["master_imr"], 0xff);
    // The first program's edge is never delivered.
    let old = &expected["pit"]["first_program_would_be_due_ns"];
    assert!(
        !json["entry_times_ns"].as_array().unwrap().contains(old),
        "the replaced program's edge was delivered"
    );
    cleanup(&image);
}

#[test]
fn mode0_variant_gives_one_irq0() {
    let (image, manifest) = build("mode0", &["--control", "0x30", "--interrupts", "1"]);
    let expected = &manifest["expected"];
    let (output, json) = run(&image, &[]);
    assert_eq!(output.status.code(), Some(0), "{json}");
    assert_eq!(json["status"], "guest_idle");
    assert_eq!(json["wakeups"], 1);
    assert_eq!(json["counter"], 1);
    assert_eq!(json["clock_ns"], expected["time_ns"]);
    assert_eq!(json["entry_times_ns"], expected["wake_times_ns"]);
    for key in ["edges", "scheduled", "generation_changes", "cancelled"] {
        assert_eq!(json["bridge"][key], expected["bridge"][key], "{key}");
    }
    cleanup(&image);
}

#[test]
fn injected_regressions_are_distinguishable_by_stop_wakeups_counter_and_time() {
    let (image, manifest) = build("inject", &[]);
    let injections = &manifest["injections"];
    let budget = injections["budget"].as_u64().unwrap().to_string();
    let accepted = (
        "idle_halt".to_string(),
        manifest["expected"]["wakeups"].clone(),
        manifest["expected"]["counter"].clone(),
        manifest["expected"]["time_ns"].clone(),
    );
    for (flag, name) in [
        ("--inject-masked-entry2", "masked_entry2"),
        ("--inject-no-eoi", "no_eoi"),
        ("--inject-no-generation", "no_generation"),
    ] {
        let want = &injections[name];
        let (output, json) = run(&image, &["--budget", &budget, flag]);
        let stop = if json["exit"] == "budget_exhausted" {
            "budget"
        } else {
            json["exit"].as_str().unwrap()
        };
        assert_eq!(stop, want["stop"].as_str().unwrap(), "{name}: {json}");
        assert_eq!(json["wakeups"], want["wakeups"], "{name}");
        assert_eq!(json["counter"], want["counter"], "{name}");
        assert_eq!(json["clock_ns"], want["time_ns"], "{name}");
        if let Some(times) = want.get("wake_times_ns") {
            assert_eq!(&json["entry_times_ns"], times, "{name}");
        }
        if let Some(edges) = want.get("edges") {
            assert_eq!(&json["bridge"]["edges"], edges, "{name}");
        }
        if let Some(state) = want.get("vector_state") {
            assert_eq!(
                serde_json::json!([json["apic"]["irr"], json["apic"]["isr"]]),
                *state,
                "{name}"
            );
        }
        // The regression is visible: it differs from the accepted run.
        let seen = (
            stop.to_string(),
            json["wakeups"].clone(),
            json["counter"].clone(),
            json["clock_ns"].clone(),
        );
        assert_ne!(seen, accepted, "{name} must differ from the accepted run");
        assert_ne!(output.status.code(), Some(1));
        assert!(json["injection"].is_string());
    }
    cleanup(&image);
}

#[test]
fn builds_are_reproducible_and_loader_and_arguments_fail_closed() {
    let (a, _) = build("repro-a", &[]);
    let (b, _) = build("repro-b", &[]);
    let bytes = fs::read(&a).unwrap();
    assert_eq!(bytes, fs::read(&b).unwrap());
    assert_eq!(bytes.len(), 0x4000);
    cleanup(&b);

    let mut corrupted = bytes.clone();
    corrupted[0x1000] ^= 0xff; // GDT
    let bad = a.with_file_name("corrupt.bin");
    fs::write(&bad, corrupted).unwrap();
    let (output, json) = run(&bad, &[]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(json["status"], "loader_error");

    // An injection needs its patch site: a moved EOI store is a loader error.
    let mut moved = bytes;
    moved[0x40b1 - 0x1000 + 2] = 0xb1;
    let moved_path = a.with_file_name("moved.bin");
    fs::write(&moved_path, moved).unwrap();
    let (output, json) = run(&moved_path, &["--inject-no-eoi"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(json["status"], "loader_error");

    let short = a.with_file_name("short.bin");
    fs::write(&short, [0_u8; 16]).unwrap();
    let (output, json) = run(&short, &[]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(json["status"], "invalid_input");

    let (output, json) = run(&a.with_file_name("missing.bin"), &[]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(json["status"], "invalid_input");

    for options in [
        vec!["--inject-no-eoi", "--inject-masked-entry2"],
        vec!["--inject-no-eoi", "--inject-no-eoi"],
        vec!["--budget", "100001"],
        vec!["--budget", "x"],
        vec!["--bogus", "1"],
        vec!["--budget"],
    ] {
        let (output, json) = run(&a, &options);
        assert_eq!(output.status.code(), Some(3), "{options:?}");
        assert_eq!(json["status"], "invalid_input", "{options:?}");
    }
    cleanup(&a);
}
