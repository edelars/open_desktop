//! S6-19b: the litmus corpus on our machine in the interleaving mode through
//! `od-runner --stage6-litmus`. The expected sets are read from the fixture files of S6-05
//! (derived from the cited sources), never from a run of the machine.
//!
//! The recorded choices:
//! - a fixture with a `LOCK` form (or `XCHG` with memory) the interpreter does not execute gives
//!   the verdict `unsupported` and exit 0: it is a known gap (S6-20), not an `equal` and not a
//!   failure; the summary counts it apart;
//! - `forbidden`, `incomplete` and (in an exhaustive run) `not_covered` give exit 5; `not_covered`
//!   in a seed or single run is coverage only, exit 0.

use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
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

fn corpus() -> PathBuf {
    root().join("tests/fixtures/guest/stage6/litmus")
}

fn run(extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage6-litmus")
        .args(extra)
        .output()
        .unwrap()
}

fn fixture_arg(file: &str) -> String {
    corpus().join(file).display().to_string()
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

fn set_of(value: &Value) -> BTreeSet<Vec<u64>> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect()
        })
        .collect()
}

/// The `interleaving_instruction_atomic` set straight from the fixture file.
fn fixture_expected(file: &str) -> BTreeSet<Vec<u64>> {
    let doc: Value =
        serde_json::from_str(&fs::read_to_string(corpus().join(file)).unwrap()).unwrap();
    set_of(&doc["outcomes"]["interleaving_instruction_atomic"]["values"])
}

fn observed(case: &Value) -> BTreeSet<Vec<u64>> {
    case["histogram"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            set_of(&json!([entry["outcome"]]))
                .into_iter()
                .next()
                .unwrap()
        })
        .collect()
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("od_s6_litmus_cli_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A copy of a fixture whose `interleaving_instruction_atomic` set is replaced.
fn with_expected(scratch: &Scratch, file: &str, set: &[[u64; 4]]) -> PathBuf {
    let mut doc: Value =
        serde_json::from_str(&fs::read_to_string(corpus().join(file)).unwrap()).unwrap();
    doc["outcomes"]["interleaving_instruction_atomic"]["values"] = json!(set);
    let path = scratch.path(file);
    fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    path
}

fn only_case(out: &Output) -> Value {
    let doc = json_of(out);
    assert_eq!(doc["schema"], "od.stage6-litmus-run.v1");
    assert_eq!(doc["cases"].as_array().unwrap().len(), 1);
    doc["cases"][0].clone()
}

#[test]
fn sb_and_mp_exhaustive_equal_the_fixture_sets_and_exit_0() {
    for (file, id) in [("sb.json", "SB"), ("mp.json", "MP")] {
        let out = run(&["--fixture", &fixture_arg(file)]);
        assert_eq!(out.status.code(), Some(0), "{file}: {}", json_of(&out));
        let doc = json_of(&out);
        assert_eq!(doc["status"], "ok");
        assert_eq!(doc["exploration"]["mode"], "exhaustive");
        assert_eq!(doc["summary"]["equal"], 1);
        let case = &doc["cases"][0];
        assert_eq!(case["id"], id);
        assert_eq!(case["verdict"], "equal");
        assert_eq!(case["vcpus"], 2);
        assert_eq!(case["complete_schedules"], "6");
        assert_eq!(case["runs"], 6);
        assert_eq!(case["instructions_cut"], 0);
        assert_eq!(case["incomplete_runs"], 0);
        assert_eq!(case["forbidden_outcomes"], json!([]));
        assert_eq!(case["not_covered_by_exploration"], json!([]));
        assert_eq!(case["unsupported"], Value::Null);
        assert_eq!(observed(case), fixture_expected(file), "{file}");
        assert_eq!(
            set_of(&case["expected_interleaving_instruction_atomic"]),
            fixture_expected(file)
        );
        let runs: u64 = case["histogram"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["runs"].as_u64().unwrap())
            .sum();
        assert_eq!(runs, 6, "every schedule is in the histogram");
        assert!(case["histogram"][0]["first"]["schedule"].is_string());
    }
    // SB: both-zero is x86-TSO only and is never reached here.
    let sb = only_case(&run(&["--fixture", &fixture_arg("sb.json")]));
    assert!(!observed(&sb).contains(&vec![0, 0, 1, 1]));
    assert_eq!(sb["x86_tso_only_outcomes"], json!([[0, 0, 1, 1]]));
}

#[test]
fn the_corpus_runs_eight_fixtures_equal_and_names_the_seven_the_interpreter_refuses() {
    let out = run(&["--corpus", &corpus().display().to_string()]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let doc = json_of(&out);
    assert_eq!(doc["status"], "ok");
    assert_eq!(doc["summary"]["cases"], 15);
    assert_eq!(doc["summary"]["equal"], 8);
    assert_eq!(doc["summary"]["unsupported"], 7);
    assert_eq!(doc["summary"]["forbidden"], 0);
    let mut unsupported = BTreeSet::new();
    for case in doc["cases"].as_array().unwrap() {
        match case["verdict"].as_str().unwrap() {
            "equal" => {
                let file = Path::new(case["source"].as_str().unwrap())
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string();
                assert_eq!(observed(case), fixture_expected(&file), "{file}");
            }
            "unsupported" => {
                assert_eq!(case["runs"], 0);
                assert_eq!(case["histogram"], json!([]));
                unsupported.insert(case["id"].as_str().unwrap().to_string());
            }
            other => panic!("{}: {other}", case["id"]),
        }
    }
    let expected: BTreeSet<String> = [
        "LOCK-CMPXCHG-MUTEX",
        "LOCK-XADD",
        "SB-LOCK",
        "SB-LOCK-LOCAL",
        "SB-LOCK-XADD",
        "SB-XCHG",
        "XCHG-MUTEX",
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(unsupported, expected);
}

#[test]
fn a_lock_form_is_unsupported_with_the_instruction_and_its_bytes_and_is_not_equal() {
    // Recorded choice: verdict "unsupported", exit 0 (a known gap of the interpreter, S6-20).
    let out = run(&["--fixture", &fixture_arg("lock-xadd.json")]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let doc = json_of(&out);
    assert_eq!(doc["status"], "ok");
    assert_eq!(doc["summary"]["equal"], 0);
    assert_eq!(doc["summary"]["unsupported"], 1);
    let case = &doc["cases"][0];
    assert_eq!(case["verdict"], "unsupported");
    assert_eq!(case["runs"], 0);
    let unsupported = &case["unsupported"];
    assert!(
        unsupported["instruction"]
            .as_str()
            .unwrap()
            .starts_with("lock xadd")
    );
    assert!(
        unsupported["bytes"]
            .as_str()
            .unwrap()
            .starts_with("f0 0f c1")
    );
    assert_eq!(unsupported["cpu"], 0);
    assert!(
        unsupported["reason"]
            .as_str()
            .unwrap()
            .contains("not execute")
    );
    // The expected set is still the fixture's, for the day the form runs.
    assert_eq!(
        set_of(&case["expected_interleaving_instruction_atomic"]),
        fixture_expected("lock-xadd.json")
    );

    // The LOCK-INC counter of the formula file: unsupported too, never replaced by another body.
    let formula = fixture_arg("counters-formula.json");
    let out = run(&[
        "--formula",
        &formula,
        "--counter",
        "LOCK-INC",
        "--threads",
        "2",
        "--rounds",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let case = only_case(&out);
    assert_eq!(case["id"], "LOCK-INC(N=2,K=1)");
    assert_eq!(case["verdict"], "unsupported");
    assert!(
        case["unsupported"]["instruction"]
            .as_str()
            .unwrap()
            .starts_with("lock inc")
    );
    assert_eq!(
        case["expected_interleaving_instruction_atomic"],
        json!([[2]])
    );
}

#[test]
fn a_formula_counter_loses_an_update_between_instructions() {
    let formula = fixture_arg("counters-formula.json");
    let out = run(&[
        "--formula",
        &formula,
        "--counter",
        "THREE-INSN-COUNTER",
        "--threads",
        "2",
        "--rounds",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let case = only_case(&out);
    assert_eq!(case["verdict"], "equal");
    assert_eq!(observed(&case), BTreeSet::from([vec![1], vec![2]]));
    // UNLOCKED-INC is exact in this mode (an instruction is indivisible).
    let out = run(&[
        "--formula",
        &formula,
        "--counter",
        "UNLOCKED-INC",
        "--threads",
        "3",
        "--rounds",
        "2",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    assert_eq!(observed(&only_case(&out)), BTreeSet::from([vec![6]]));
}

#[test]
fn bad_arguments_exit_3_with_invalid_input_and_a_missing_or_broken_fixture_exits_4() {
    let sb = fixture_arg("sb.json");
    let formula = fixture_arg("counters-formula.json");
    for extra in [
        vec![],
        vec!["--mode", "exhaustive"],
        vec!["--fixture"],
        vec!["--fixture", &sb, "--corpus", "x"],
        vec!["--fixture", &sb, "--fixture", &sb],
        vec!["--fixture", &sb, "--mode", "random"],
        vec!["--fixture", &sb, "--mode", "seed", "--seeds", "5"],
        vec!["--fixture", &sb, "--mode", "seed", "--seeds", "9..=3"],
        vec!["--fixture", &sb, "--seeds", "0..=3"], // seeds without --mode seed
        vec!["--fixture", &sb, "--mode", "seed", "--budget", "5"],
        vec!["--fixture", &sb, "--budget", "0"],
        vec!["--fixture", &sb, "--budget", "5"], // 6 schedules
        vec!["--fixture", &sb, "--quantum", "0"],
        vec!["--fixture", &sb, "--quantum", "2", "--schedule", "0:4"],
        vec!["--fixture", &sb, "--quantum", "2", "--mode", "seed"],
        vec!["--fixture", &sb, "--schedule", "0:2"], // a thread is left unfinished
        vec!["--fixture", &sb, "--schedule", "0:2,7:2"], // no such vCPU
        vec!["--fixture", &sb, "--schedule", "0:0"],
        vec!["--fixture", &sb, "--schedule", "0:10000001"],
        vec!["--fixture", &sb, "--schedule", "0:1,0:0xFFFFFFFFFFFFFFFF"],
        vec!["--fixture", &sb, "--schedule", "zero"],
        vec!["--corpus", "x", "--quantum", "1"],
        vec!["--fixture", &sb, "--counter", "LOCK-INC"],
        vec!["--formula", &formula],
        vec![
            "--formula",
            &formula,
            "--counter",
            "LOCK-INC",
            "--threads",
            "2",
            "--rounds",
            "1",
            "--mode",
            "seed",
            "--budget",
            "1",
        ],
        vec!["--fixture", &sb, "--frobnicate", "1"],
        vec!["--fixture", &sb, "--budget", "+6"], // a leading `+` is not a number
        vec!["--fixture", &sb, "--budget", "0x+6"],
        vec!["--fixture", &sb, "--mode", "seed", "--seeds", "+1..=5"],
        vec!["--fixture", &sb, "--schedule", "0:+4,1:2"],
    ] {
        let out = run(&extra);
        assert_eq!(
            out.status.code(),
            Some(3),
            "{extra:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let doc = json_of(&out);
        assert_eq!(doc["status"], "invalid_input", "{extra:?}");
        assert_eq!(doc["error"]["kind"], "arguments");
    }
    // Above the budget: 17,153,136 schedules.
    let out = run(&[
        "--formula",
        &formula,
        "--counter",
        "THREE-INSN-COUNTER",
        "--threads",
        "3",
        "--rounds",
        "2",
    ]);
    assert_eq!(out.status.code(), Some(3));
    // Loader errors.
    let scratch = Scratch::new("load");
    fs::write(scratch.path("broken.json"), "{not json").unwrap();
    fs::write(
        scratch.path("other.json"),
        r#"{"schema": "something-else"}"#,
    )
    .unwrap();
    fs::write(
        scratch.path("noname.json"),
        r#"{"schema": "od.smp-litmus.v1"}"#,
    )
    .unwrap();
    for target in [
        scratch.path("absent.json"),
        scratch.path("broken.json"),
        scratch.path("other.json"),
        scratch.path("noname.json"),
    ] {
        let out = run(&["--fixture", &target.display().to_string()]);
        assert_eq!(out.status.code(), Some(4), "{}", target.display());
        let doc = json_of(&out);
        assert_eq!(doc["status"], "loader_error");
    }
    // Addresses at the top of the 64-bit space overflow `start + size` and `address + 4`: the
    // loader answers with an error (exit 4), it does not panic (exit 101).
    for (what, pointer) in [
        ("code region start", "/memory/code_regions/0/start"),
        ("cell address", "/memory/cells/0/address"),
    ] {
        for value in [
            "0xfffffffffffffffe",
            "0xffffffffffffffff",
            "0xfffffffffffffffc",
        ] {
            let mut doc: Value =
                serde_json::from_str(&fs::read_to_string(corpus().join("sb.json")).unwrap())
                    .unwrap();
            *doc.pointer_mut(pointer).unwrap() = json!(value);
            let path = scratch.path("overflow.json");
            fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
            let out = run(&["--fixture", &path.display().to_string()]);
            assert_eq!(out.status.code(), Some(4), "{what} {value}");
            assert_eq!(json_of(&out)["status"], "loader_error", "{what} {value}");
        }
    }
    let out = run(&["--corpus", &scratch.path("nowhere").display().to_string()]);
    assert_eq!(out.status.code(), Some(4));
    let out = run(&[
        "--formula",
        &formula,
        "--counter",
        "NOPE",
        "--threads",
        "2",
        "--rounds",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(4));
    let out = run(&[
        "--formula",
        &formula,
        "--counter",
        "LOCK-INC",
        "--threads",
        "99",
        "--rounds",
        "1",
    ]);
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn every_case_carries_the_sha256_of_its_source_file() {
    let sb = fixture_arg("sb.json");
    let expected = od_testkit::sha256_hex(&fs::read(corpus().join("sb.json")).unwrap());
    let out = run(&["--fixture", &sb]);
    assert_eq!(only_case(&out)["fixture_sha256"], json!(expected));
    let formula = fixture_arg("counters-formula.json");
    let expected =
        od_testkit::sha256_hex(&fs::read(corpus().join("counters-formula.json")).unwrap());
    let out = run(&[
        "--formula",
        &formula,
        "--counter",
        "UNLOCKED-INC",
        "--threads",
        "2",
        "--rounds",
        "1",
    ]);
    assert_eq!(only_case(&out)["fixture_sha256"], json!(expected));
}

#[test]
fn hex_numbers_accept_both_prefixes() {
    let sb = fixture_arg("sb.json");
    for budget in ["0x10", "0X10", "16"] {
        let out = run(&["--fixture", &sb, "--budget", budget]);
        assert_eq!(out.status.code(), Some(0), "{budget}");
    }
}

#[test]
fn seeds_0_to_99_give_byte_identical_json_and_a_subset_of_the_fixture_set() {
    let sb = fixture_arg("sb.json");
    let args = ["--fixture", &sb, "--mode", "seed", "--seeds", "0..=99"];
    let first = run(&args);
    let again = run(&args);
    assert_eq!(first.status.code(), Some(0), "{}", json_of(&first));
    assert_eq!(
        first.stdout, again.stdout,
        "the same seeds give the same bytes"
    );
    let case = only_case(&first);
    assert_eq!(case["runs"], 100);
    assert_eq!(case["verdict"], "equal");
    let expected = fixture_expected("sb.json");
    assert!(observed(&case).is_subset(&expected));
    let mut total = 0;
    let mut frequency = 0.0;
    for entry in case["histogram"].as_array().unwrap() {
        total += entry["runs"].as_u64().unwrap();
        frequency += entry["frequency"].as_f64().unwrap();
        assert!(entry["first"]["seed"].as_u64().unwrap() <= 99);
        assert_eq!(entry["in_expected_set"], true);
    }
    assert_eq!(total, 100);
    assert!((frequency - 1.0).abs() < 1e-9);
    // A later thread may go first: [1, 0, 1, 1] needs cpu1 before cpu0.
    assert!(observed(&case).contains(&vec![1, 0, 1, 1]));
    let doc = json_of(&first);
    assert_eq!(doc["exploration"]["seeds"], json!([0, 99]));
    // A different range of seeds gives different bytes.
    let other = run(&["--fixture", &sb, "--mode", "seed", "--seeds", "100..=199"]);
    assert_ne!(first.stdout, other.stdout);
    let single = run(&["--fixture", &sb, "--mode", "seed", "--seeds", "7..=7"]);
    assert_eq!(only_case(&single)["runs"], 1);
}

#[test]
fn single_runs_by_quantum_and_by_explicit_schedule() {
    let sb = fixture_arg("sb.json");
    // cpu0 runs both steps, then cpu1 both: cpu0.eax reads y = 0, cpu1.ebx reads x = 1.
    let out = run(&["--fixture", &sb, "--schedule", "0:2,1:2"]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let case = only_case(&out);
    assert_eq!(case["runs"], 1);
    assert_eq!(observed(&case), BTreeSet::from([vec![0, 1, 1, 1]]));
    assert_eq!(case["histogram"][0]["first"]["schedule"], "0011");
    // One run cannot cover the set: coverage only, exit 0.
    assert_eq!(case["verdict"], "not_covered");
    let out = run(&["--fixture", &sb, "--quantum", "1"]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let case = only_case(&out);
    assert_eq!(case["runs"], 1);
    assert_eq!(case["incomplete_runs"], 0);
    assert_eq!(case["histogram"][0]["first"]["quantum"], 1);
    assert_eq!(observed(&case), BTreeSet::from([vec![1, 1, 1, 1]]));
}

#[test]
fn a_forbidden_outcome_exits_5_and_so_does_a_set_the_machine_cannot_reach_in_an_exhaustive_run() {
    let scratch = Scratch::new("forbidden");
    // The expected set without [1,1,1,1], which the machine does reach: forbidden.
    let tampered = with_expected(&scratch, "sb.json", &[[0, 1, 1, 1], [1, 0, 1, 1]]);
    let target = tampered.display().to_string();
    for args in [
        vec!["--fixture", &target],
        vec!["--fixture", &target, "--mode", "seed", "--seeds", "0..=99"],
    ] {
        let out = run(&args);
        assert_eq!(out.status.code(), Some(5), "{args:?}: {}", json_of(&out));
        let doc = json_of(&out);
        assert_eq!(doc["status"], "failed");
        assert_eq!(doc["summary"]["forbidden"], 1);
        let case = &doc["cases"][0];
        assert_eq!(case["verdict"], "forbidden");
        assert_eq!(case["forbidden_outcomes"], json!([[1, 1, 1, 1]]));
        let flagged: Vec<&Value> = case["histogram"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["in_expected_set"] == false)
            .collect();
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0]["outcome"], json!([1, 1, 1, 1]));
    }

    // The TSO-only outcome [0,0,1,1] added to the set: never reached. Exhaustive: the sets must be
    // equal, exit 5; seeds only need a subset, exit 0, with the gap named.
    let widened = with_expected(
        &scratch,
        "sb.json",
        &[[0, 1, 1, 1], [1, 0, 1, 1], [1, 1, 1, 1], [0, 0, 1, 1]],
    );
    let target = widened.display().to_string();
    let out = run(&["--fixture", &target]);
    assert_eq!(out.status.code(), Some(5), "{}", json_of(&out));
    let case = only_case(&out);
    assert_eq!(case["verdict"], "not_covered");
    assert_eq!(case["not_covered_by_exploration"], json!([[0, 0, 1, 1]]));
    let out = run(&["--fixture", &target, "--mode", "seed", "--seeds", "0..=99"]);
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    assert_eq!(only_case(&out)["verdict"], "not_covered");
}

#[test]
fn the_exhaustive_histograms_equal_the_s6_19_proof_data() {
    // The data file of S6-19 was written by the driver test; the runner is the same driver.
    let data: Value = serde_json::from_str(
        &fs::read_to_string(root().join("docs/reports/data/stage6/s6-19-litmus-interleaving.json"))
            .unwrap(),
    )
    .unwrap();
    let out = run(&["--corpus", &corpus().display().to_string()]);
    let doc = json_of(&out);
    let histogram = |case: &Value| -> Vec<(Vec<u64>, u64)> {
        case["histogram"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    set_of(&json!([entry["outcome"]]))
                        .into_iter()
                        .next()
                        .unwrap(),
                    entry["runs"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    let mut compared = 0;
    for record in data["cases"].as_array().unwrap() {
        if record["mode"] != "exhaustive" || record["id"].as_str().unwrap().contains('(') {
            continue;
        }
        let ours = doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == record["id"])
            .unwrap_or_else(|| panic!("{} is not in the corpus run", record["id"]));
        assert_eq!(histogram(ours), histogram(record), "{}", record["id"]);
        assert_eq!(ours["runs"], record["runs"], "{}", record["id"]);
        compared += 1;
    }
    assert_eq!(compared, 8);
}

/// `(outcome, runs)` of every histogram entry, in the order the runner writes them.
fn histogram_of(case: &Value) -> Vec<(Vec<u64>, u64, Value)> {
    case["histogram"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                set_of(&json!([entry["outcome"]]))
                    .into_iter()
                    .next()
                    .unwrap(),
                entry["runs"].as_u64().unwrap(),
                entry["first"].clone(),
            )
        })
        .collect()
}

fn proof_data() -> Value {
    serde_json::from_str(
        &fs::read_to_string(root().join("docs/reports/data/stage6/s6-19-litmus-interleaving.json"))
            .unwrap(),
    )
    .unwrap()
}

/// `UNLOCKED-INC(N=3,K=2)` as the arguments of a formula run.
fn formula_args(id: &str, formula: &str) -> Vec<String> {
    let (name, rest) = id.split_once('(').unwrap();
    let rest = rest.trim_end_matches(')');
    let (threads, rounds) = rest.split_once(',').unwrap();
    vec![
        "--formula".to_string(),
        formula.to_string(),
        "--counter".to_string(),
        name.to_string(),
        "--threads".to_string(),
        threads.trim_start_matches("N=").to_string(),
        "--rounds".to_string(),
        rounds.trim_start_matches("K=").to_string(),
    ]
}

#[test]
fn the_formula_counters_equal_the_s6_19_proof_data_by_exhaustion_and_by_seeds() {
    // UNLOCKED-INC and THREE-INSN-COUNTER (not LOCK-INC: the interpreter refuses it) at every
    // size of the data file whose schedule count is at most 100,000 (a few seconds in all).
    let data = proof_data();
    let formula = fixture_arg("counters-formula.json");
    let mut exhaustive = 0;
    let mut seeded = 0;
    for record in data["cases"].as_array().unwrap() {
        let id = record["id"].as_str().unwrap();
        let mode = record["mode"].as_str().unwrap();
        let counter = id.starts_with("UNLOCKED-INC(") || id.starts_with("THREE-INSN-COUNTER(");
        let small = record["runs"].as_u64().is_some_and(|runs| runs > 0)
            && record["complete_schedules"]
                .as_str()
                .is_some_and(|n| n.parse::<u128>().unwrap() <= 100_000);
        if !counter || !small {
            continue;
        }
        let mut args = formula_args(id, &formula);
        if mode == "seeds (cross-check)" {
            args.extend(["--mode", "seed", "--seeds", "0..=9999"].map(String::from));
            seeded += 1;
        } else {
            assert_eq!(mode, "exhaustive", "{id}");
            exhaustive += 1;
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run(&refs);
        assert_eq!(out.status.code(), Some(0), "{id}: {}", json_of(&out));
        let ours = only_case(&out);
        assert_eq!(ours["runs"], record["runs"], "{id} {mode}");
        assert_eq!(histogram_of(&ours), histogram_of(record), "{id} {mode}");
    }
    // 2 + 2 + 2 + ... counted from the data file: both counters at six and five sizes.
    assert!(exhaustive >= 10, "{exhaustive} exhaustive records compared");
    assert!(seeded >= 10, "{seeded} seed records compared");
}

#[test]
fn the_fixture_seed_runs_equal_the_s6_19_seed_records() {
    // Seeds 0..=9999 on the finite fixtures: the same random step order as the S6-19 driver, so
    // the histograms (outcome, runs, first seed) are equal to the cross-check records.
    let data = proof_data();
    let out = run(&[
        "--corpus",
        &corpus().display().to_string(),
        "--mode",
        "seed",
        "--seeds",
        "0..=9999",
    ]);
    let doc = json_of(&out);
    let mut compared = 0;
    for record in data["cases"].as_array().unwrap() {
        if record["mode"] != "seeds (cross-check)" || record["id"].as_str().unwrap().contains('(') {
            continue;
        }
        let ours = doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == record["id"])
            .unwrap_or_else(|| panic!("{} is not in the corpus run", record["id"]));
        assert_eq!(ours["runs"], record["runs"], "{}", record["id"]);
        assert_eq!(histogram_of(ours), histogram_of(record), "{}", record["id"]);
        compared += 1;
    }
    assert_eq!(compared, 8);
}

#[test]
fn an_unfinished_run_is_incomplete_and_not_forbidden_exit_5() {
    // Thread 0 ends in `jmp $` (EB FE): it never reaches its end. The outcome [0,0,1,1] it leaves
    // is not in the expected set, yet an unfinished run is not a forbidden outcome.
    let scratch = Scratch::new("incomplete");
    let mut doc: Value =
        serde_json::from_str(&fs::read_to_string(corpus().join("sb.json")).unwrap()).unwrap();
    doc["threads"][0]["instructions"][1] = json!({
        "asm": "jmp $",
        "bytes": "eb fe",
        "op": "jmp",
    });
    doc["memory"]["code_regions"][0]["size_bytes"] = json!(13);
    let path = scratch.path("jmp-self.json");
    fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    let out = run(&["--fixture", &path.display().to_string()]);
    assert_eq!(out.status.code(), Some(5), "{}", json_of(&out));
    let case = only_case(&out);
    assert_eq!(case["verdict"], "incomplete", "{case}");
    assert!(case["incomplete_runs"].as_u64().unwrap() > 0);
    assert_eq!(json_of(&out)["summary"]["forbidden"], 0);
}
