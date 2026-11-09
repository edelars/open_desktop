//! S4-27i3: the real clang/lld `Stage4InterruptQueueProbeV1` image driven by the
//! event pump through the runner (fixed three-phase caller schedule). Expected
//! evidence is read from the builder's manifest (derived from the queue rules
//! and the Intel SDM), not from the runner's own output.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

struct Built {
    image: PathBuf,
    manifest: Value,
}

fn build(root: &Path, name: &str, flags: &[&str]) -> Built {
    let out = Command::new("python3")
        .current_dir(root)
        .args([
            "tools/build/build_stage4_interrupt_queue_probe.py",
            "--output-dir",
            &format!("work/stage4/{name}"),
        ])
        .args(flags)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = root.join(format!("work/stage4/{name}"));
    Built {
        image: dir.join("probe.bin"),
        manifest: serde_json::from_slice(&fs::read(dir.join("build_manifest.json")).unwrap())
            .unwrap(),
    }
}

fn run(image: &Path, extra: &[&str]) -> (Output, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-interrupt-queue-probe")
        .arg(image)
        .args(extra)
        .output()
        .unwrap();
    let json = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    (out, json)
}

fn num(value: &Value) -> u64 {
    let text = value.as_str().unwrap();
    u64::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

fn events(value: &Value) -> Vec<(String, Option<u64>)> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["event"].as_str().unwrap().to_string(),
                e.get("vector").map(|v| v.as_u64().unwrap()),
            )
        })
        .collect()
}

/// Run and compare every item the manifest prescribes.
fn check(built: &Built) -> Value {
    let expected = &built.manifest["expected"];
    let (out, x) = run(&built.image, &[]);
    assert!(out.status.success(), "{x}");
    assert_eq!(x["profile"], "Stage4InterruptQueueProbeV1");
    assert_eq!(x["status"], "guest_idle");
    assert_eq!(x["exit"], expected["final"]["exit"]);
    assert_eq!(x["waiting"], true);
    assert_eq!(x["injection"], Value::Null);
    // Phase 1: the request fails, the event is kept, the guest has not moved.
    let phase1 = &expected["phase1"];
    assert_eq!(x["phase1"]["exit"], phase1["exit"]);
    assert!(
        x["phase1"]["error"]
            .as_str()
            .unwrap()
            .starts_with(phase1["error"].as_str().unwrap())
    );
    assert_eq!(x["phase1"]["retired"], phase1["retired"]);
    assert_eq!(x["phase1"]["queue_len"], phase1["queue_len"]);
    assert_eq!(num(&x["phase1"]["rip"]), num(&phase1["rip"]));
    assert_eq!(x["counter"], expected["counter"]);
    assert_eq!(x["wakeups"], expected["wakeups"]);
    assert_eq!(x["time_ns"], expected["time_ns"]);
    assert_eq!(x["coalesced"], expected["coalesced"]);
    assert_eq!(x["queue_len"], expected["queue_len"]);
    assert_eq!(events(&x["trace"]), events(&expected["events"]));
    assert_eq!(num(&x["rip"]), num(&expected["final"]["rip"]));
    assert_eq!(num(&x["rsp"]), num(&expected["final"]["rsp"]));
    assert_eq!(num(&x["rflags"]), num(&expected["final"]["rflags"]));
    for (vector, state) in expected["final"]["vector"].as_object().unwrap() {
        assert_eq!(x["vectors"][vector]["irr"], state[0], "{vector}");
        assert_eq!(x["vectors"][vector]["isr"], state[1], "{vector}");
    }
    let entries = x["entries"].as_array().unwrap();
    let wake_times = expected["wake_times_ns"].as_array().unwrap();
    assert_eq!(entries.len(), wake_times.len());
    for (entry, time) in entries.iter().zip(wake_times) {
        assert_eq!(entry["source"], "halt_wake");
        assert_eq!(entry["time_ns"], *time);
        assert_eq!(
            entry["rip_after_entry"],
            format!("0x{:016x}", num(&expected["handler"]))
        );
        let frame = &expected["frame"];
        assert_eq!(
            entry["frame_low_to_high"],
            serde_json::json!([
                frame["rip"],
                frame["cs"],
                frame["rflags"],
                frame["rsp"],
                frame["ss"]
            ])
        );
        assert_eq!(num(&entry["rsp_after_entry"]), num(&frame["address"]));
        assert_eq!(
            num(&entry["rflags_after_entry"]),
            num(&expected["handler_rflags"])
        );
    }
    x
}

#[test]
fn real_artifact_is_reproducible_and_matches_the_queue_rules() {
    let r = root();
    let tag = format!("s4-27i3-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"), &[]);
    let a2 = build(&r, &format!("{tag}-a2"), &[]);
    assert_eq!(
        fs::read(&a.image).unwrap(),
        fs::read(&a2.image).unwrap(),
        "builds must be byte-identical"
    );
    assert_eq!(a.manifest["profile"], "Stage4InterruptQueueProbeV1");
    assert_eq!(a.manifest["accepted_profile"], true);
    let x = check(&a);
    // Three wake-ups (E1, F, the merged pair), counter 3, time 250000 ns, one merge.
    assert_eq!(
        (
            x["counter"].as_u64(),
            x["wakeups"].as_u64(),
            x["time_ns"].as_u64(),
            x["coalesced"].as_u64()
        ),
        (Some(3), Some(3), Some(250_000), Some(1))
    );
}

#[test]
fn the_pop_before_request_injection_is_distinguishable_in_the_evidence() {
    let r = root();
    let tag = format!("s4-27i3-inj-{}", std::process::id());
    let built = build(&r, &tag, &[]);
    let (out, accepted) = run(&built.image, &[]);
    assert!(out.status.success());
    let (out, injected) = run(&built.image, &["--inject-pop-before-request"]);
    // The injected pump is idle too (a successful exit) but lost the due event.
    assert!(out.status.success(), "{injected}");
    let expected = &built.manifest["expected"]["injection_pop_before_request"];
    assert_eq!(injected["injection"], "PopBeforeRequest");
    assert_eq!(injected["exit"], "idle_halt");
    assert_eq!(injected["phase1"]["exit"], "rejected");
    assert_eq!(
        injected["phase1"]["queue_len"],
        expected["phase1_queue_len"]
    );
    assert_eq!(injected["wakeups"], expected["wakeups"]);
    assert_eq!(injected["counter"], expected["counter"]);
    assert_eq!(injected["time_ns"], expected["time_ns"]);
    assert_eq!(injected["coalesced"], expected["coalesced"]);
    assert_ne!(
        (accepted["counter"].clone(), accepted["wakeups"].clone()),
        (injected["counter"].clone(), injected["wakeups"].clone())
    );
}

#[test]
fn budget_and_input_errors_are_reported() {
    let r = root();
    let tag = format!("s4-27i3-bud-{}", std::process::id());
    let built = build(&r, &tag, &[]);
    // Budget 2: phase 1 fails at once (no quantum), phase 2 runs two quanta.
    let (out, x) = run(&built.image, &["--budget", "2"]);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["status"], "work_limit");
    assert_eq!(
        (x["quanta"].as_u64(), x["retired"].as_u64()),
        (Some(2), Some(2))
    );

    let (out, x) = run(&built.image, &["--request", "0x41@3"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(x["status"], "invalid_input");

    let (out, x) = run(&built.image, &["--budget", "1000000"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(x["status"], "invalid_input");

    let (out, x) = run(&r.join("work/stage4/does-not-exist.bin"), &[]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(x["error"]["kind"], "file");

    let mut bytes = fs::read(&built.image).unwrap();
    bytes[0x2008 - 0x1000] ^= 1; // corrupt the GDT code descriptor
    let bad = r.join(format!("work/stage4/{tag}/bad.bin"));
    fs::write(&bad, bytes).unwrap();
    let (out, x) = run(&bad, &[]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(x["status"], "loader_error");
}
