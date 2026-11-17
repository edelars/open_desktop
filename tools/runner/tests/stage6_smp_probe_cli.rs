//! S6-16f: the `Stage6SmpProbeV2` guest through `od-runner --stage6-smp-probe`. The image comes from
//! the real builder (`tools/build/build_stage6_smp_probe.py`, `python3`; no skip when it is
//! missing). Expectations are the builder's manifest and `expected_uart_n*.txt` (derived from the
//! protocol and the SDM), never the output of a run.
//!
//! Gate of the CLI (the range recorded in `docs/testing/SMP.md`): a fixed quantum, an explicit
//! schedule, and every seed `0..=63`: the same seed twice gives byte-identical JSON, COM1 is the
//! manifest transcript for every seed, the turn counts differ between seeds, the nonce is in the
//! output. The long range `0..=9999` is the `#[ignore]` test at the end (release builds).

use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const NONCE: &str = "cli-nonce-16f";
/// CI range of seeds (inclusive).
const CI_SEEDS: std::ops::RangeInclusive<u64> = 0..=63;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

struct Built {
    dir: PathBuf,
    image: PathBuf,
    manifest: Value,
}

impl Built {
    fn expected(&self, vcpus: usize) -> String {
        fs::read_to_string(self.dir.join(format!("expected_uart_n{vcpus}.txt")))
            .unwrap()
            .replace("{nonce}", NONCE)
    }
}

fn build(name: &str, flags: &[&str]) -> Built {
    let root = root();
    let dir = root.join(format!("work/stage6/smp_probe_cli_{name}"));
    let out = Command::new("python3")
        .current_dir(&root)
        .args(["tools/build/build_stage6_smp_probe.py", "--output-dir"])
        .arg(&dir)
        .args(flags)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Built {
        image: dir.join("probe.bin"),
        manifest: serde_json::from_slice(&fs::read(dir.join("build_manifest.json")).unwrap())
            .unwrap(),
        dir,
    }
}

fn run(image: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage6-smp-probe")
        .arg(image)
        .args(["--nonce", NONCE])
        .args(extra)
        .output()
        .unwrap()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

fn numbers(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_u64().unwrap())
        .collect()
}

/// What the manifest says an `n2` run must leave in the cells of the protocol.
fn check_cells(built: &Built, x: &Value) {
    let memory = &built.manifest["expected"]["n2"]["memory"];
    for (cell, key) in [
        ("lock_entered", "lock_entered"),
        ("flag_ap_up", "flag_ap_up"),
        ("start_count", "start_count"),
        ("ap_rx", "ap_rx"),
        ("bsp_rx", "bsp_rx"),
        ("nmi_cell", "nmi_cell"),
    ] {
        let want = u64::from_str_radix(
            memory[key]["value"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
            16,
        )
        .unwrap();
        assert_eq!(x["cells"][cell].as_u64().unwrap(), want, "{cell}");
    }
    // The cells of the shootdown scenario (S6-17) are in the manifest of that scenario only.
    if built.manifest["scenario"] == "shootdown" {
        for cell in ["sd_ack", "sd_post", "sd_seen_handler", "sd_seen_main"] {
            let want = u64::from_str_radix(
                memory[cell]["value"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap();
            assert_eq!(x["cells"][cell].as_u64().unwrap(), want, "{cell}");
        }
    }
}

#[test]
fn a_fixed_quantum_run_reports_every_field_of_the_exchange() {
    let built = build("fixed", &["--scenario", "exchange"]);
    let out = run(
        &built.image,
        &["--quantum", "1024", "--scenario", "exchange"],
    );
    assert!(out.status.success(), "{}", json(&out));
    let x = json(&out);
    assert_eq!(x["profile"], "Stage6SmpProbeV2");
    assert_eq!(x["status"], "guest_idle");
    assert_eq!(x["exit"], "all_idle");
    assert_eq!(x["scenario"], "exchange");
    assert_eq!(x["vcpus"], 2);
    assert_eq!(x["nonce"], NONCE);
    assert_eq!(x["com1_text"].as_str().unwrap(), built.expected(2));
    assert!(x["com1_text"].as_str().unwrap().contains(NONCE));
    let hex = x["com1_hex"].as_str().unwrap();
    assert_eq!(hex.len(), 2 * built.expected(2).len());
    // The quantum policy, the hash of the image (the manifest's), the counters per vCPU.
    assert_eq!(x["quantum_policy"]["policy"], "fixed");
    assert_eq!(x["quantum_policy"]["quantum"], 1024);
    assert_eq!(
        x["image"]["sha256"],
        built.manifest["artifacts"]["probe.bin"]["sha256"]
    );
    assert_eq!(x["image"]["bytes"], 0xB000);
    assert_eq!(numbers(&x["retired"]).len(), 2);
    assert!(numbers(&x["retired"]).iter().all(|&n| n > 0));
    assert_eq!(
        x["retired_total"].as_u64().unwrap(),
        numbers(&x["retired"]).iter().sum::<u64>()
    );
    assert_eq!(numbers(&x["turns_per_cpu"]).len(), 2);
    assert_eq!(
        x["turns"].as_u64().unwrap(),
        numbers(&x["turns_per_cpu"]).iter().sum::<u64>()
    );
    let expected = &built.manifest["expected"]["n2"];
    assert_eq!(
        numbers(&x["init_applied"]),
        [0, expected["ap_init_applied"].as_u64().unwrap()]
    );
    assert_eq!(
        numbers(&x["started"]),
        [0, expected["ap_started"].as_u64().unwrap()]
    );
    assert_eq!(x["activity"], serde_json::json!(["Halted", "Halted"]));
    assert_eq!(x["lapic_vectors"], serde_json::json!([[], []]));
    check_cells(&built, &x);
    assert!(x["error"].is_null());
}

#[test]
fn an_explicit_schedule_replays_a_recorded_run_and_ends_with_schedule_end() {
    let built = build("schedule", &["--scenario", "exchange"]);
    // One step per turn: the recorded schedule is exactly the cutting of the run, so replaying it
    // is the same run (a longer turn would apply an ICR event later than one step would).
    let recorded = json(&run(&built.image, &["--quantum", "1", "--record-schedule"]));
    let runs: Vec<String> = recorded["schedule"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| format!("{}:{}", run[0], run[1]))
        .collect();
    assert!(runs.len() > 10);
    let schedule = runs.join(",");
    let out = run(&built.image, &["--schedule", &schedule]);
    let x = json(&out);
    assert!(out.status.success(), "{x}");
    assert_eq!(x["status"], "schedule_end");
    assert_eq!(x["exit"], "schedule_end");
    assert_eq!(x["quantum_policy"]["policy"], "schedule");
    assert_eq!(x["com1_text"].as_str().unwrap(), built.expected(2));
    assert_eq!(x["retired"], recorded["retired"]);
    assert_eq!(x["started"], recorded["started"]);
    check_cells(&built, &x);
    // The same schedule twice: the same bytes.
    assert_eq!(
        out.stdout,
        run(&built.image, &["--schedule", &schedule]).stdout
    );
    // A schedule that names a vCPU that is not running at that step is refused, not skipped.
    let bad = run(&built.image, &["--schedule", "1:1"]);
    assert!(!bad.status.success());
    assert_eq!(json(&bad)["status"], "scheduler_error");
}

#[test]
fn every_seed_gives_the_same_transcript_and_cells_and_the_same_seed_the_same_bytes() {
    let built = build("seeds", &["--scenario", "exchange"]);
    let expected = built.expected(2);
    let mut turns = BTreeSet::new();
    let mut retired_bsp = BTreeSet::new();
    for seed in CI_SEEDS {
        let seed_text = seed.to_string();
        let out = run(&built.image, &["--seed", &seed_text]);
        assert!(out.status.success(), "seed {seed}: {}", json(&out));
        let x = json(&out);
        assert_eq!(x["com1_text"].as_str().unwrap(), expected, "seed {seed}");
        assert!(x["com1_text"].as_str().unwrap().contains(NONCE));
        assert_eq!(x["quantum_policy"]["seed"], seed);
        assert_eq!(x["quantum_policy"]["policy"], "seeded");
        assert_eq!(
            x["lapic_vectors"],
            serde_json::json!([[], []]),
            "seed {seed}"
        );
        assert_eq!(numbers(&x["started"]), [0, 1], "seed {seed}");
        check_cells(&built, &x);
        turns.insert(x["turns"].as_u64().unwrap());
        retired_bsp.insert(numbers(&x["retired"])[0]);
        // The same seed again: the same bytes of JSON.
        assert_eq!(
            out.stdout,
            run(&built.image, &["--seed", &seed_text]).stdout,
            "seed {seed}"
        );
    }
    assert!(
        turns.len() >= 2,
        "the seeds cut the run into different numbers of turns: {turns:?}"
    );
    assert!(retired_bsp.len() >= 2, "{retired_bsp:?}");
}

#[test]
fn the_shootdown_scenario_ends_ok_under_every_seed_and_leaves_the_cells_of_the_manifest() {
    let built = build("shootdown", &["--scenario", "shootdown"]);
    let expected = built.expected(2);
    assert!(
        expected.contains("SMP PROBE SHOOTDOWN OK K=16 "),
        "{expected}"
    );
    for seed in CI_SEEDS {
        let out = run(&built.image, &["--seed", &seed.to_string()]);
        assert!(out.status.success(), "seed {seed}: {}", json(&out));
        let x = json(&out);
        assert_eq!(x["scenario"], "shootdown");
        assert_eq!(x["com1_text"].as_str().unwrap(), expected, "seed {seed}");
        assert_eq!(
            x["lapic_vectors"],
            serde_json::json!([[], []]),
            "seed {seed}"
        );
        assert_eq!(numbers(&x["started"]), [0, 1], "seed {seed}");
        check_cells(&built, &x);
    }
}

#[test]
fn the_negative_scenarios_end_as_the_manifest_says() {
    // One processor: AP ABSENT in every scenario, exit 0.
    let exchange = build("one", &["--scenario", "exchange"]);
    let out = run(&exchange.image, &["--vcpus", "1"]);
    let x = json(&out);
    assert!(out.status.success(), "{x}");
    assert_eq!(x["com1_text"].as_str().unwrap(), exchange.expected(1));
    assert!(x["com1_text"].as_str().unwrap().contains("AP ABSENT"));
    assert!(!x["com1_text"].as_str().unwrap().contains("OK"));
    // An IPI to a missing APIC ID: a typed board error after the marker, exit 5, nothing set.
    let bad = build("badipi", &["--scenario", "bad-ipi"]);
    let out = run(&bad.image, &["--quantum", "7"]);
    let x = json(&out);
    assert_eq!(out.status.code(), Some(5), "{x}");
    assert_eq!(x["status"], "guest_error");
    assert_eq!(x["exit"], "board_error");
    assert_eq!(x["error"]["kind"], "board_error");
    assert_eq!(
        bad.manifest["expected"]["n2"]["run_error"],
        "LapicDeviceError"
    );
    assert!(
        x["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("UnsupportedConfiguration"),
        "{x}"
    );
    assert_eq!(x["com1_text"].as_str().unwrap(), bad.expected(2));
    assert!(
        x["com1_text"]
            .as_str()
            .unwrap()
            .ends_with(&format!("BAD-IPI {NONCE}\n"))
    );
    assert_eq!(x["lapic_vectors"], serde_json::json!([[], []]));
    // A second SIPI to the running AP: started stays 1.
    let resipi = build("resipi", &["--scenario", "resipi"]);
    let out = run(&resipi.image, &["--seed", "9"]);
    let x = json(&out);
    // Exit 0 with `guest_idle` is the run's end, not the verdict: the verdict is `com1_text`.
    assert!(out.status.success(), "{x}");
    assert_eq!(x["status"], "guest_idle");
    assert_eq!(x["com1_text"].as_str().unwrap(), resipi.expected(2));
    assert_eq!(numbers(&x["started"]), [0, 1]);
    assert_eq!(x["cells"]["start_count"], 1);
    // NMI to the AP on the board's tables: handler of vector 2 ran once.
    let nmi = build("nmi", &["--scenario", "nmi"]);
    let out = run(&nmi.image, &["--quantum", "100"]);
    let x = json(&out);
    assert!(out.status.success(), "{x}");
    assert_eq!(x["status"], "guest_idle");
    assert_eq!(x["com1_text"].as_str().unwrap(), nmi.expected(2));
    assert_eq!(numbers(&x["nmi_delivered"]), [0, 1]);
    assert_eq!(x["cells"]["nmi_cell"], 1);
}

#[test]
fn the_cli_refuses_bad_arguments_with_exit_3_and_a_loader_error_with_exit_4() {
    let built = build("args", &["--scenario", "exchange"]);
    for extra in [
        vec!["--vcpus", "0"],
        vec!["--vcpus", "5"],
        vec!["--quantum", "0"],
        vec!["--quantum", "4", "--seed", "4"],
        vec!["--seed", "1", "--schedule", "0:3"],
        vec!["--budget", "0"],
        vec!["--scenario", "resipi"], // the image was built as exchange
        vec!["--scenario", "nope"],
        vec!["--schedule", "9:3"],
        vec!["--schedule", "0:1,0:0xFFFFFFFFFFFFFFFF"], // the sum must not overflow
        vec!["--schedule", "0:10000001"],
        vec!["--schedule", "0:5000000,1:5000001"],
        vec!["--frobnicate", "1"],
    ] {
        let out = run(&built.image, &extra);
        assert_eq!(out.status.code(), Some(3), "{extra:?}: {}", json(&out));
        assert_eq!(json(&out)["status"], "invalid_input");
    }
    let no_nonce = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage6-smp-probe")
        .arg(&built.image)
        .output()
        .unwrap();
    assert_eq!(no_nonce.status.code(), Some(3));
    let missing = run(&built.dir.join("nope.bin"), &[]);
    assert_eq!(missing.status.code(), Some(3));
    // A nonce the loader refuses (a space).
    let out = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage6-smp-probe")
        .arg(&built.image)
        .args(["--nonce", "has space"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4), "{}", json(&out));
    assert_eq!(json(&out)["status"], "loader_error");
}

/// Proof run over the long range of seeds (release build):
///
/// ```text
/// OD_S6_SMP_SEEDS=0..=9999 OD_S6_SMP_SEEDS_OUT=docs/reports/data/stage6/<file>.json \
///   [OD_S6_SMP_SCENARIO=shootdown] cargo test --release -p od-runner --offline --test stage6_smp_probe_cli -- --ignored seed_proof
/// ```
///
/// It checks, for every seed, the same things as the CI range and writes one summary (range,
/// elapsed seconds, the distinct transcripts, the spread of turns and retired counts, the image
/// hash). A seed that fails stops the run and is named.
#[test]
#[ignore = "long range of seeds: run in release, see the doc comment"]
fn seed_proof() {
    let range = std::env::var("OD_S6_SMP_SEEDS").unwrap_or_else(|_| "0..=9999".to_string());
    let (first, last) = range.split_once("..=").expect("OD_S6_SMP_SEEDS=A..=B");
    let (first, last): (u64, u64) = (first.parse().unwrap(), last.parse().unwrap());
    // `OD_S6_SMP_SCENARIO=shootdown` proves the S6-17 scenario over the same range.
    let scenario = std::env::var("OD_S6_SMP_SCENARIO").unwrap_or_else(|_| "exchange".to_string());
    let built = build("proof", &["--scenario", &scenario]);
    let expected = built.expected(2);
    let started = std::time::Instant::now();
    let (mut turns, mut retired) = (BTreeSet::new(), BTreeSet::new());
    let (mut turn_min, mut turn_max) = (u64::MAX, 0);
    for seed in first..=last {
        let out = run(&built.image, &["--seed", &seed.to_string()]);
        let x = json(&out);
        assert!(out.status.success(), "seed {seed}: {x}");
        assert_eq!(x["com1_text"].as_str().unwrap(), expected, "seed {seed}");
        assert_eq!(
            x["lapic_vectors"],
            serde_json::json!([[], []]),
            "seed {seed}"
        );
        assert_eq!(numbers(&x["started"]), [0, 1], "seed {seed}");
        check_cells(&built, &x);
        let t = x["turns"].as_u64().unwrap();
        (turn_min, turn_max) = (turn_min.min(t), turn_max.max(t));
        turns.insert(t);
        retired.insert(numbers(&x["retired"]));
    }
    let elapsed = started.elapsed().as_secs_f64();
    let summary = serde_json::json!({
        "schema": "stage6-smp-probe-seed-proof-v1",
        "scenario": scenario,
        "vcpus": 2,
        "seeds": {"first": first, "last": last, "count": last - first + 1},
        "nonce": NONCE,
        "image_sha256": built.manifest["artifacts"]["probe.bin"]["sha256"],
        "elapsed_seconds": elapsed,
        "per_run_seconds": elapsed / (last - first + 1) as f64,
        "distinct_transcripts": 1,
        "transcript": expected,
        "distinct_turn_counts": turns.len(),
        "turns_min": turn_min,
        "turns_max": turn_max,
        "distinct_retired_pairs": retired.len(),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    });
    if let Ok(path) = std::env::var("OD_S6_SMP_SEEDS_OUT") {
        let path = root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&summary).unwrap() + "\n").unwrap();
    }
    println!("{summary}");
}
