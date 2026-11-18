//! S6-36: `od-runner --stage6-litmus --parallel`, the corpus in the parallel mode. The reference
//! set is the x86-TSO set of the fixture (formula for a counter), read from the fixture file; a run
//! is not deterministic, so only its structure and the verdict are checked here (the long runs and
//! their numbers are in `docs/reports/data/stage6/s6-36/`).

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/guest/stage6/litmus")
}

fn run(extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage6-litmus")
        .args(extra)
        .output()
        .unwrap()
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

fn fixture(file: &str) -> String {
    corpus().join(file).display().to_string()
}

#[test]
fn a_two_thread_fixture_runs_on_two_and_four_vcpus_and_stays_in_the_x86_tso_set() {
    for vcpus in ["2", "4"] {
        let out = run(&[
            "--fixture",
            &fixture("mp.json"),
            "--parallel",
            "--vcpus",
            vcpus,
            "--iterations",
            "3000",
        ]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let result = json_of(&out);
        assert_eq!(result["schema"], "od.stage6-litmus-parallel-run.v1");
        assert_eq!(result["status"], "ok");
        // The weakening features are off in this build.
        assert_eq!(result["build"]["weak_host_ordering"], false);
        assert_eq!(result["build"]["nonatomic_lock_test"], false);
        let case = &result["cases"][0];
        assert_eq!(case["status"], "run");
        assert_eq!(case["concurrent_threads"], 2);
        assert_eq!(case["vcpus"].as_u64().unwrap().to_string(), vcpus);
        assert_eq!(case["rounds"], 3000);
        assert_eq!(case["verdict"], "allowed_only");
        assert_eq!(case["forbidden_outcomes"].as_array().unwrap().len(), 0);
        assert_eq!(case["late_barrier_rounds"], 0);
        let counted: u64 = case["histogram"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["rounds"].as_u64().unwrap())
            .sum();
        assert_eq!(counted, 3000);
    }
}

#[test]
fn a_four_thread_fixture_is_not_applicable_to_two_vcpus_and_runs_on_four() {
    let args = |vcpus: &'static str| {
        run(&[
            "--fixture",
            &fixture("iriw.json"),
            "--parallel",
            "--vcpus",
            vcpus,
            "--iterations",
            "2000",
        ])
    };
    let two = json_of(&args("2"));
    assert_eq!(two["cases"][0]["status"], "not_applicable");
    assert_eq!(two["summary"]["not_applicable"], 1);
    assert!(
        two["cases"][0].get("histogram").is_none(),
        "no result for two vCPUs"
    );
    let four = json_of(&args("4"));
    assert_eq!(four["cases"][0]["status"], "run");
    assert_eq!(four["cases"][0]["concurrent_threads"], 4);
    assert_eq!(four["cases"][0]["vcpus"], 4);
    assert_eq!(four["cases"][0]["verdict"], "allowed_only");
}

#[test]
fn the_parallel_flag_takes_only_its_own_options() {
    for extra in [
        vec!["--parallel", "--vcpus", "2"],
        vec!["--parallel", "--iterations", "10"],
        vec!["--parallel", "--vcpus", "2", "--iterations", "0"],
        vec!["--parallel", "--vcpus", "9", "--iterations", "10"],
        vec![
            "--parallel",
            "--vcpus",
            "2",
            "--iterations",
            "10",
            "--mode",
            "seed",
        ],
        vec!["--vcpus", "2", "--iterations", "10"],
    ] {
        let mut args = vec!["--fixture".to_string(), fixture("mp.json")];
        args.extend(extra.iter().map(ToString::to_string));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run(&refs);
        assert_eq!(
            out.status.code(),
            Some(3),
            "{extra:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(json_of(&out)["status"], "invalid_input", "{extra:?}");
    }
}
