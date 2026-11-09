//! S4-27i1: the real clang/lld `Stage4InterruptPumpProbeV1` images driven by the
//! event pump through the runner. Expected evidence is read from the builder's
//! manifest (derived from the Intel SDM), not from the runner's own output.

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
            "tools/build/build_stage4_interrupt_pump_probe.py",
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

fn run(image: &Path, extra: &[String]) -> (Output, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-interrupt-pump-probe")
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

fn events(x: &Value) -> Vec<(String, Option<u64>)> {
    x["trace"]
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

/// Run with the request the manifest prescribes and compare every item.
fn check(built: &Built, which: &str) -> Value {
    let expected = &built.manifest["expected"][which];
    let request = &expected["request"];
    let argument = format!(
        "{}@{}",
        request["vector"].as_str().unwrap(),
        request["after_quanta"]
    );
    let (out, x) = run(&built.image, &["--request".into(), argument]);
    assert!(out.status.success(), "{x}");
    assert_eq!(x["profile"], "Stage4InterruptPumpProbeV1");
    assert_eq!(x["status"], "guest_idle");
    assert_eq!(x["exit"], expected["final"]["exit"]);
    assert_eq!(x["waiting"], true);
    let want: Vec<(String, Option<u64>)> = expected["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["event"].as_str().unwrap().to_string(),
                e.get("vector").map(|v| v.as_u64().unwrap()),
            )
        })
        .collect();
    assert_eq!(events(&x), want, "{which}");
    assert_eq!(num(&x["rip"]), num(&expected["final"]["rip"]), "{which}");
    assert_eq!(num(&x["rsp"]), num(&expected["final"]["rsp"]), "{which}");
    assert_eq!(
        num(&x["rflags"]),
        num(&expected["final"]["rflags"]),
        "{which}"
    );
    for (vector, state) in expected["final"]["vectors"].as_object().unwrap() {
        assert_eq!(x["vectors"][vector]["irr"], state[0], "{which} {vector}");
        assert_eq!(x["vectors"][vector]["isr"], state[1], "{which} {vector}");
    }
    let frames = expected["frames"].as_array().unwrap();
    let entries = x["entries"].as_array().unwrap();
    assert_eq!(entries.len(), frames.len(), "{which}");
    for (entry, frame) in entries.iter().zip(frames) {
        assert_eq!(
            entry["frame_low_to_high"],
            serde_json::json!([
                frame["rip"],
                frame["cs"],
                frame["rflags"],
                frame["rsp"],
                frame["ss"]
            ]),
            "{which}"
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
fn real_artifact_is_reproducible_and_every_scenario_matches_the_sdm_expectations() {
    let r = root();
    let tag = format!("s4-27i1-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"), &[]);
    let a2 = build(&r, &format!("{tag}-a2"), &[]);
    assert_eq!(
        fs::read(&a.image).unwrap(),
        fs::read(&a2.image).unwrap(),
        "builds must be byte-identical"
    );
    assert_eq!(a.manifest["profile"], "Stage4InterruptPumpProbeV1");
    assert_eq!(a.manifest["accepted_profile"], true);

    // Scenario a, then the nested case on the same image.
    let x = check(&a, "request_b");
    assert_eq!(x["entries"][0]["source"], "halt_wake");
    assert_eq!(x["retired"], 10);
    let nested = check(&a, "request_a_nested");
    let sources: Vec<_> = nested["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["vector"].as_u64().unwrap(), e["source"].as_str().unwrap()))
        .collect();
    assert_eq!(sources, [(0x51, "halt_wake"), (0x41, "boundary")]);

    let if1 = build(&r, &format!("{tag}-if1"), &["--scenario", "if1_hlt"]);
    let x = check(&if1, "request_b");
    assert_eq!(x["entries"][0]["source"], "boundary");

    let cli = build(&r, &format!("{tag}-cli"), &["--scenario", "cli_hlt"]);
    let x = check(&cli, "request_b");
    assert!(x["entries"].as_array().unwrap().is_empty());
    assert_eq!(x["vectors"]["0x41"]["irr"], true);

    let rep = build(&r, &format!("{tag}-rep"), &["--scenario", "rep"]);
    let x = check(&rep, "request_b");
    // Deviation: the entry follows the completed REP, so RCX is 0 at the HLT.
    assert_eq!(x["rcx"], "0x0000000000000000");
}

#[test]
fn omitting_the_eoi_leaves_the_second_vector_undelivered() {
    let r = root();
    let tag = format!("s4-27i1-eoi-{}", std::process::id());
    let built = build(&r, &tag, &["--omit-eoi"]);
    assert_eq!(built.manifest["accepted_profile"], false);
    let x = check(&built, "request_a_nested");
    assert_eq!(x["vectors"]["0x51"]["isr"], true);
    assert_eq!(x["vectors"]["0x41"]["irr"], true);
    assert_eq!(x["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn injections_are_distinguishable_in_the_evidence() {
    let r = root();
    let tag = format!("s4-27i1-inj-{}", std::process::id());
    let built = build(&r, &tag, &[]);
    let request = ["--request".to_string(), "0x41@3".to_string()];

    let (out, x) = run(
        &built.image,
        &[
            request[0].clone(),
            request[1].clone(),
            "--inject-skip-shadow-check".into(),
        ],
    );
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["status"], "rejected");
    assert_eq!(x["error"]["detail"], "ApicQuantumProfile");
    assert_eq!(x["injection"], "SkipShadowCheck");
    assert!(x["entries"].as_array().unwrap().is_empty());
    assert_eq!(x["waiting"], false);
    assert_eq!(x["vectors"]["0x41"]["irr"], true);
    assert_eq!(x["vectors"]["0x41"]["isr"], false);

    let (out, x) = run(
        &built.image,
        &[
            request[0].clone(),
            request[1].clone(),
            "--inject-hlt-as-ordinary".into(),
        ],
    );
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["status"], "guest_terminal");
    assert!(
        x["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("Unsupported")
    );
    assert_eq!(x["waiting"], false);
    assert!(x["entries"].as_array().unwrap().is_empty());
}

#[test]
fn budget_and_input_errors_are_reported() {
    let r = root();
    let tag = format!("s4-27i1-bud-{}", std::process::id());
    let built = build(&r, &tag, &[]);
    let (out, x) = run(&built.image, &["--budget".into(), "2".into()]);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["status"], "work_limit");
    assert_eq!(
        (x["quanta"].as_u64(), x["retired"].as_u64()),
        (Some(2), Some(2))
    );

    let (out, x) = run(&built.image, &["--request".into(), "0x41".into()]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(x["status"], "invalid_input");

    let (out, x) = run(&built.image, &["--budget".into(), "1000000".into()]);
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
