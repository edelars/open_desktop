//! S6-41e: the S6-19/S6-36 litmus corpus with the JIT in both modes (the corpus run).
//!
//! `#[ignore]`: the run is long and is made in release (`cargo test --release -p od-host --test
//! s6_41e_litmus_jit -- --ignored --nocapture`). Environment:
//!
//! * `OD_S6_41E_ROUNDS` rounds per test and configuration in the parallel mode (default 20 000);
//! * `OD_S6_41E_OUT` path of the JSON result (default: none, the log only);
//! * `OD_S6_41E_MODES` `parallel`, `interleaved` or `both` (default `both`);
//! * `OD_S6_41E_VCPUS` `2`, `4` or `all` (default `all`): the parallel configurations;
//! * `OD_S6_41E_PROCESS` a label for this process, copied into the JSON.
//!
//! Exit: the test fails when any run has a forbidden outcome, an interleaved run that is not equal
//! to the instruction-atomic set, or a run in which the JIT did not execute every JIT form of every
//! round (a run without the JIT is not counted, S6-41e).
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::path::{Path, PathBuf};
use std::time::Instant;

use od_host::litmus_jit::{
    JitForm, JitInstruction, JitLitmusCounters, jit_classification, run_litmus_jit_interleaved,
    run_litmus_jit_parallel,
};
use od_machine::{LitmusCase, LitmusInterleavedVerdict, LitmusParallelVerdict};
use serde_json::{Value, json};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/guest/stage6/litmus")
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.replace('_', "").parse().ok())
        .unwrap_or(default)
}

fn counters_json(counters: &JitLitmusCounters) -> Value {
    json!({
        "jit_retired": counters.jit_retired,
        "jit_calls": counters.jit_calls,
        "blocks_admitted": counters.blocks_admitted,
        "blocks_reused": counters.blocks_reused,
        "helper_calls": counters.helper_calls,
        "ordinary_declined": counters.ordinary_declined,
        "helper_declined": counters.helper_declined,
        "jit_forms_per_round": counters.jit_forms_per_round,
        "rounds_or_runs": counters.rounds,
        "jit_expected": counters.jit_expected(),
        "fallback_of_jit_forms": counters.fallback(),
        "counted": counters.counted(),
    })
}

fn classification_json(classification: &[JitInstruction]) -> Value {
    Value::Array(
        classification
            .iter()
            .map(|instruction| {
                let (via, reason) = match &instruction.form {
                    JitForm::Jit => ("jit", Value::Null),
                    JitForm::Interpreter(reason) => ("interpreter", json!(reason)),
                };
                json!({
                    "thread": instruction.thread,
                    "index": instruction.index,
                    "asm": instruction.asm,
                    "bytes": instruction.bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    "via": via,
                    "reason": reason,
                })
            })
            .collect(),
    )
}

fn parallel_run(case: &LitmusCase, vcpus: usize, rounds: u64, failures: &mut Vec<String>) -> Value {
    let started = Instant::now();
    let run = run_litmus_jit_parallel(case, vcpus, rounds)
        .unwrap_or_else(|error| panic!("{} on {vcpus} vCPUs: {error:?}", case.id()));
    let seconds = started.elapsed().as_secs_f64();
    let report = &run.litmus;
    let forbidden = report.forbidden.len();
    eprintln!(
        "parallel {:<22} vcpus {vcpus} rounds {rounds} {:>7.1}s {:>8.0} r/s  outcomes {} forbidden {forbidden}  jit {} calls {} admitted {} reused {} fallback {}  late_barrier {}  {}",
        report.id,
        seconds,
        rounds as f64 / seconds,
        report.histogram.len(),
        run.jit.jit_retired,
        run.jit.jit_calls,
        run.jit.blocks_admitted,
        run.jit.blocks_reused,
        run.jit.fallback(),
        report.late_barrier_rounds,
        report.verdict.name(),
    );
    if report.verdict != LitmusParallelVerdict::Allowed || forbidden > 0 {
        failures.push(format!(
            "{} parallel {vcpus}: forbidden {:?}",
            report.id, report.forbidden
        ));
    }
    if !run.jit.counted() {
        failures.push(format!(
            "{} parallel {vcpus}: the JIT run is not counted: {:?}",
            report.id, run.jit
        ));
    }
    json!({
        "mode": "parallel",
        "id": report.id,
        "vcpus": vcpus,
        "threads": report.threads,
        "placements": report.placements,
        "rounds": report.rounds,
        "elapsed_s": seconds,
        "rounds_per_s": rounds as f64 / seconds,
        "verdict": report.verdict.name(),
        "forbidden": report.forbidden,
        "observe": report.observe,
        "histogram": report.histogram.iter().map(|o| json!({
            "values": o.values, "rounds": o.rounds, "in_x86_tso": o.in_x86_tso,
            "first_round": o.first_round, "first_placement": o.first_placement,
        })).collect::<Vec<_>>(),
        "x86_tso": report.x86_tso,
        "not_observed": report.not_observed,
        "late_barrier_rounds": report.late_barrier_rounds,
        "hooked_retired": report.hooked_retired,
        "interpreted_retired": report.interpreted_retired,
        "jit": counters_json(&run.jit),
        "classification": classification_json(&run.classification),
    })
}

fn interleaved_run(case: &LitmusCase, failures: &mut Vec<String>) -> Value {
    let started = Instant::now();
    // 100 000 schedules at most exhaustively; the corpus has at most a few thousand; seeds would
    // be 0..=9999 (S6-19) beyond that.
    let run = run_litmus_jit_interleaved(case, 100_000, 0..=9_999)
        .unwrap_or_else(|error| panic!("{} interleaved: {error:?}", case.id()));
    let seconds = started.elapsed().as_secs_f64();
    let report = &run.litmus;
    eprintln!(
        "interleaved {:<20} vcpus {} {} runs {:>6} {:>7.2}s outcomes {} forbidden {} not_covered {} incomplete {}  jit {} calls {} admitted {} reused {} fallback {}  {}",
        report.id,
        report.vcpus,
        report.policy,
        report.runs,
        seconds,
        report.histogram.len(),
        report.forbidden.len(),
        report.not_covered.len(),
        report.incomplete_runs,
        run.jit.jit_retired,
        run.jit.jit_calls,
        run.jit.blocks_admitted,
        run.jit.blocks_reused,
        run.jit.fallback(),
        report.verdict.name(),
    );
    let exhaustive = report.policy == "exhaustive";
    if report.verdict == LitmusInterleavedVerdict::Forbidden
        || (exhaustive && report.verdict != LitmusInterleavedVerdict::Equal)
        || report.incomplete_runs != 0
    {
        failures.push(format!(
            "{} interleaved: verdict {} forbidden {:?} not_covered {:?} incomplete {}",
            report.id,
            report.verdict.name(),
            report.forbidden,
            report.not_covered,
            report.incomplete_runs
        ));
    }
    if !run.jit.counted() {
        failures.push(format!(
            "{} interleaved: the JIT run is not counted: {:?}",
            report.id, run.jit
        ));
    }
    json!({
        "mode": "interleaved",
        "id": report.id,
        "vcpus": report.vcpus,
        "policy": report.policy,
        "runs": report.runs,
        "elapsed_s": seconds,
        "verdict": report.verdict.name(),
        "forbidden": report.forbidden,
        "not_covered": report.not_covered,
        "incomplete_runs": report.incomplete_runs,
        "observe": report.observe,
        "histogram": report.histogram.iter().map(|o| json!({
            "values": o.values, "runs": o.runs, "expected": o.expected, "in_x86_tso": o.in_x86_tso,
        })).collect::<Vec<_>>(),
        "expected": report.expected,
        "x86_tso": report.x86_tso,
        "hooked_retired": report.hooked_retired,
        "interpreted_retired": report.interpreted_retired,
        "jit": counters_json(&run.jit),
        "classification": classification_json(&run.classification),
    })
}

#[test]
#[ignore = "the corpus run: release, minutes (OD_S6_41E_ROUNDS)"]
fn s6_41e_the_corpus_with_the_jit_in_both_modes() {
    let rounds = env_u64("OD_S6_41E_ROUNDS", 20_000);
    let modes = std::env::var("OD_S6_41E_MODES").unwrap_or_else(|_| "both".into());
    let vcpu_filter = std::env::var("OD_S6_41E_VCPUS").unwrap_or_else(|_| "all".into());
    let want = |vcpus: usize| vcpu_filter == "all" || vcpu_filter == vcpus.to_string();
    let parallel_mode = modes == "both" || modes == "parallel";
    let interleaved_mode = modes == "both" || modes == "interleaved";

    // The corpus: the 15 fixtures and the counters of the S6-36 matrix.
    let mut cases: Vec<LitmusCase> = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(corpus())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && path
                    .file_name()
                    .is_some_and(|name| name != "counters-formula.json")
        })
        .collect();
    files.sort();
    for path in &files {
        cases.push(LitmusCase::from_fixture_file(path).unwrap());
    }
    let formula = std::fs::read_to_string(corpus().join("counters-formula.json")).unwrap();
    // (id, threads, rounds K): the S6-36 matrix (N = 2, K = 2 on 2 and 4 vCPUs; N = 4 on 4).
    let counters = [
        ("LOCK-INC", 2u64, 2u64),
        ("UNLOCKED-INC", 2, 2),
        ("THREE-INSN-COUNTER", 2, 2),
        ("LOCK-INC", 4, 2),
        ("UNLOCKED-INC", 4, 2),
    ];
    for (id, threads, k) in counters {
        cases.push(LitmusCase::from_formula_json(&formula, id, threads, k).unwrap());
    }

    let mut failures = Vec::new();
    let mut runs = Vec::new();
    let mut via_jit = Vec::new();
    let mut not_via_jit = Vec::new();
    let started = Instant::now();
    for case in &cases {
        let classification = jit_classification(case);
        let has_jit = classification.iter().any(|i| i.form == JitForm::Jit);
        let label = format!("{} ({} threads)", case.id(), case.vcpus());
        if !has_jit {
            not_via_jit.push(json!({
                "id": case.id(),
                "threads": case.vcpus(),
                "reason": "no instruction of the fixture is a JIT form (LOCK, XCHG, fence); its ordinary MOV loads and stores and the unlocked INC are not admitted into a JIT block (S6-41d profile) and stay with the interpreter",
                "classification": classification_json(&classification),
            }));
            continue;
        }
        via_jit.push(json!({
            "id": case.id(),
            "label": label,
            "threads": case.vcpus(),
            "classification": classification_json(&classification),
        }));
        if interleaved_mode && case.vcpus() <= 4 {
            runs.push(interleaved_run(case, &mut failures));
        }
        if parallel_mode {
            for vcpus in [2usize, 4] {
                if !want(vcpus) || case.vcpus() > vcpus {
                    continue;
                }
                runs.push(parallel_run(case, vcpus, rounds, &mut failures));
            }
        }
    }
    let total = started.elapsed().as_secs_f64();
    eprintln!(
        "total {total:.1}s, {} runs, {} failures",
        runs.len(),
        failures.len()
    );

    if let Ok(path) = std::env::var("OD_S6_41E_OUT") {
        let document = json!({
            "schema": "stage6-s6-41e-litmus-jit/1",
            "process": std::env::var("OD_S6_41E_PROCESS").unwrap_or_default(),
            "rounds_per_test": rounds,
            "modes": modes,
            "vcpus": vcpu_filter,
            "elapsed_s": total,
            "runs": runs,
            "via_jit": via_jit,
            "not_via_jit": not_via_jit,
            "failures": failures,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&document).unwrap()).unwrap();
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
