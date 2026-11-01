use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::process::{Command, ExitCode};

use od_isa::execute::{
    AddpdControls, AddpsControls, AddsdControls, AddssControls, ComissMemoryRaxControls,
    Cvtpd2dqMemoryControls, Cvtpd2piControls, Cvtpd2psControls, Cvtpd2psMemoryControls,
    Cvtpi2pdControls, Cvtpi2psControls, Cvtps2dqMemoryControls, Cvtps2pdMemoryControls,
    Cvtsd2ssControls, Cvtsd2ssMemoryControls, Cvtss2sdMemoryControls, Cvttpd2dqControls,
    Cvttpd2dqMemoryControls, Cvttps2dqControls, Cvttps2dqMemoryControls,
    Cvttss2siRegistersControls, DivpdControls, DivpsControls, DivsdControls, DivssControls,
    MulpdControls, MulpsControls, MulsdControls, MulssControls, SqrtpdControls, SqrtpsControls,
    SqrtsdControls, SqrtssControls, SubpdControls, SubpsControls, SubsdControls, SubssControls,
    UcomisdMemoryRaxControls, UcomissMemoryRaxControls, run_with_addpd_register_controls,
    run_with_addps_register_controls, run_with_addsd_register_controls,
    run_with_addss_register_controls, run_with_cvtpd2dq_memory_controls,
    run_with_cvtpd2dq_register_controls, run_with_cvtpd2pi_mm0_memory_rax_controls,
    run_with_cvtpd2pi_mm0_xmm1_controls, run_with_cvtpd2ps_memory_controls,
    run_with_cvtpd2ps_register_controls, run_with_cvtpi2pd_memory_controls,
    run_with_cvtpi2pd_xmm0_mm1_controls, run_with_cvtpi2ps_memory_controls,
    run_with_cvtpi2ps_xmm0_mm1_controls, run_with_cvtps2dq_memory_controls,
    run_with_cvtps2pd_memory_controls, run_with_cvtps2pi_mm0_memory_rax_controls,
    run_with_cvtps2pi_mm0_xmm1_controls, run_with_cvtsd2ss_memory_controls,
    run_with_cvtsd2ss_register_controls, run_with_cvtss2sd_memory_controls,
    run_with_cvttpd2dq_memory_controls, run_with_cvttpd2dq_register_controls,
    run_with_cvttpd2pi_mm0_memory_rax_controls, run_with_cvttpd2pi_mm0_xmm1_controls,
    run_with_cvttps2dq_memory_controls, run_with_cvttps2dq_register_controls,
    run_with_cvttps2pi_mm0_memory_rax_controls, run_with_cvttps2pi_mm0_xmm1_controls,
    run_with_cvttsd2si_registers_controls, run_with_cvttss2si_registers_controls,
    run_with_divpd_register_controls, run_with_divps_register_controls,
    run_with_divsd_register_controls, run_with_divss_register_controls,
    run_with_maxps_memory_rax_controls, run_with_minps_memory_rax_controls,
    run_with_mulpd_register_controls, run_with_mulps_register_controls,
    run_with_mulsd_register_controls, run_with_mulss_register_controls,
    run_with_sqrtpd_memory_rax_controls, run_with_sqrtpd_register_controls,
    run_with_sqrtps_memory_rax_controls, run_with_sqrtps_register_controls,
    run_with_sqrtsd_memory_rax_controls, run_with_sqrtsd_register_controls,
    run_with_sqrtss_register_controls, run_with_subpd_register_controls,
    run_with_subps_register_controls, run_with_subsd_register_controls,
    run_with_subss_register_controls,
};
use od_isa::{CpuState, ExecutionResult, interpret};
use od_testkit::case::{Fixture, LoadedFixture};
use od_testkit::compare::{
    CompareError, ComparisonReport, compare_results as compare_result_values,
};
use od_testkit::generate::generate_fixture;
use od_testkit::minimize::{MinimizeError, minimize_fixture as minimize_fixture_value};
use od_testkit::native_xorpd::{
    NATIVE_XORPD_CASES_PER_SEED, NATIVE_XORPD_FIXED_SEEDS, NATIVE_XORPD_REDUCER_DEFAULT_BUDGET,
    NATIVE_XORPD_REDUCER_MAX_BUDGET, NativeXorpdCorpusManifest, NativeXorpdReductionStop,
    generate_native_xorpd_fixture, native_xorpd_case_id,
    reduce_native_xorpd_fixture_with_input_digest,
};
use od_testkit::result::{
    Backend, BuildMetadata, CompletionStatus, CpuSnapshot, ExitReason, GprSnapshot,
    MachineMetadata, Observation, RESULT_SCHEMA_VERSION, RunResult,
};
use od_testkit::{HexU64, sha256_hex};

const EXIT_MISMATCH: u8 = 2;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_UNSUPPORTED: u8 = 4;
const EXIT_HARNESS_FAILURE: u8 = 5;

const NATIVE_XORPD_WORK_DIR: &str = "work/stage2/s2-42a-xorpd";
const NATIVE_XORPD_REDUCE_WORK_DIR: &str = "work/stage2/s2-42b-xorpd";

pub fn generate_native_xorpd_corpus(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        return invalid_input("usage: od-runner --generate-native-xorpd-corpus");
    }
    let result = (|| -> Result<(), String> {
        let root = std::path::Path::new(NATIVE_XORPD_WORK_DIR);
        let corpus_dir = root.join("corpus");
        fs::create_dir_all(root).map_err(|error| error.to_string())?;
        fs::create_dir(&corpus_dir)
            .map_err(|error| format!("could not create generated corpus directory: {error}"))?;

        let mut case_bytes = Vec::with_capacity(
            NATIVE_XORPD_FIXED_SEEDS.len() * NATIVE_XORPD_CASES_PER_SEED as usize,
        );
        for &seed in &NATIVE_XORPD_FIXED_SEEDS {
            for sequence in 0..NATIVE_XORPD_CASES_PER_SEED {
                let fixture = generate_native_xorpd_fixture(seed, sequence);
                let bytes = fixture.to_json().map_err(|error| error.to_string())?;
                let path = corpus_dir.join(format!("{}.case.json", fixture.case_id));
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|error| error.to_string())?;
                output
                    .write_all(&bytes)
                    .map_err(|error| error.to_string())?;
                case_bytes.push(bytes);
            }
        }
        let manifest = NativeXorpdCorpusManifest::from_ordered_case_bytes(&case_bytes)?;
        let manifest_path = root.join("manifest.json");
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(manifest_path)
            .map_err(|error| error.to_string())?;
        output
            .write_all(&manifest.to_json()?)
            .map_err(|error| error.to_string())?;
        println!(
            "generated {} native XORPD cases; corpus_digest={}",
            manifest.case_digests.len(),
            manifest.corpus_digest
        );
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => invalid_input(&error),
    }
}

pub fn verify_native_xorpd_corpus(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        return invalid_input("usage: od-runner --verify-native-xorpd-corpus");
    }
    match verify_native_xorpd_corpus_inner() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native XORPD corpus verification failed: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

fn verify_native_xorpd_corpus_inner() -> Result<(), String> {
    use od_testkit::v2;

    let root = std::path::Path::new(NATIVE_XORPD_WORK_DIR);
    let corpus_dir = root.join("corpus");
    let manifest_bytes = fs::read(root.join("manifest.json"))
        .map_err(|error| format!("missing or unreadable corpus manifest: {error}"))?;
    let manifest = NativeXorpdCorpusManifest::from_json(&manifest_bytes)?;
    let expected_count = NATIVE_XORPD_FIXED_SEEDS.len() * NATIVE_XORPD_CASES_PER_SEED as usize;
    let actual_files = fs::read_dir(&corpus_dir)
        .map_err(|error| format!("missing or unreadable corpus directory: {error}"))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_native_xorpd_file_count(actual_files.len(), expected_count)?;

    let mut unique_inputs = HashSet::with_capacity(expected_count);
    let mut ran_twice = 0usize;
    let mut first_replay = None;
    let mut case_index = 0usize;
    for &seed in &NATIVE_XORPD_FIXED_SEEDS {
        for sequence in 0..NATIVE_XORPD_CASES_PER_SEED {
            let case_id = native_xorpd_case_id(seed, sequence);
            let path = corpus_dir.join(format!("{case_id}.case.json"));
            let bytes =
                fs::read(&path).map_err(|error| format!("missing case {case_id}: {error}"))?;
            validate_native_xorpd_case_digest(
                &bytes,
                &manifest.case_digests[case_index],
                &case_id,
            )?;
            let loaded = v2::Fixture::from_json(&bytes)
                .map_err(|error| format!("invalid strict-schema fixture {case_id}: {error}"))?;
            if loaded.fixture.case_id != case_id
                || loaded.fixture.profile != v2::Profile::LongNativeXorpdXmm0Xmm1V1
            {
                return Err(format!("case identity or profile mismatch for {case_id}"));
            }
            let sse = loaded
                .fixture
                .initial_state
                .sse
                .as_ref()
                .ok_or_else(|| format!("missing full SSE state in {case_id}"))?;
            if !unique_inputs.insert(sse.xmm.join("")) {
                return Err(format!("duplicate XMM input contents in {case_id}"));
            }

            let first = run_native_xorpd_interpreter(&loaded)?;
            let second = run_native_xorpd_interpreter(&loaded)?;
            if first != second {
                return Err(format!("two interpreter runs differ for {case_id}"));
            }
            validate_native_xorpd_result(&loaded, &first)?;
            validate_native_xorpd_result(&loaded, &second)?;
            first_replay.get_or_insert_with(|| {
                format!(
                    "od-runner --fixture {} --engine interpreter --json",
                    path.display()
                )
            });
            ran_twice += 2;
            case_index += 1;
        }
    }

    let report = serde_json::json!({
        "report_version": 1,
        "profile": manifest.profile,
        "generator_version": manifest.generator_version,
        "corpus_digest": manifest.corpus_digest,
        "cases": expected_count,
        "interpreter_runs": ran_twice,
        "repeated_runs_per_fixture": 2,
        "comparison_scope": "all 16 GPRs, RIP, defined integer flags, all 16 XMM registers, MXCSR, retirement count, exit, exception absence, and immutable code mapping; expected XMM0 computed by test harness bytewise XOR",
        "revision": local_revision(),
        "toolchain": local_rustc_version(),
        "native": "not_run",
        "replay_command": first_replay.unwrap_or_default(),
        "physical_mac_mini": "not_run; reserved for combined final hardware batch"
    });
    let report_path = root.join("result-report.json");
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(report_path)
        .map_err(|error| format!("could not create local result report: {error}"))?;
    output
        .write_all(&serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    println!("verified {expected_count} fixtures in {ran_twice} interpreter runs; native=not_run");
    Ok(())
}

pub fn reduce_native_xorpd(args: &[String]) -> ExitCode {
    match reduce_native_xorpd_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => invalid_input(&error),
    }
}

fn reduce_native_xorpd_inner(args: &[String]) -> Result<(), String> {
    use od_testkit::v2::{self, Profile};

    let mut input_path = None;
    let mut output_dir = None;
    let mut call_budget = NATIVE_XORPD_REDUCER_DEFAULT_BUDGET;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--limit" => {
                index += 1;
                let value = args.get(index).ok_or("--limit requires an integer")?;
                call_budget = value.parse().map_err(|_| "--limit must be an integer")?;
            }
            "--output-dir" => {
                index += 1;
                output_dir = Some(
                    args.get(index)
                        .ok_or("--output-dir requires a path")?
                        .clone(),
                );
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown reducer option: {value}"));
            }
            value if input_path.is_none() => input_path = Some(value.to_string()),
            value => return Err(format!("unexpected reducer argument: {value}")),
        }
        index += 1;
    }
    let input_path = input_path
        .ok_or("usage: od-runner --reduce-native-xorpd <fixture> [--limit N] [--output-dir DIR]")?;
    if !(4..=NATIVE_XORPD_REDUCER_MAX_BUDGET).contains(&call_budget) {
        return Err(format!(
            "--limit must be 4..={NATIVE_XORPD_REDUCER_MAX_BUDGET}"
        ));
    }
    let output_dir = output_dir.unwrap_or_else(|| {
        format!(
            "{NATIVE_XORPD_REDUCE_WORK_DIR}/{}",
            std::path::Path::new(&input_path)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        )
    });
    let output_relative = std::path::Path::new(&output_dir);
    validate_native_xorpd_output_dir(output_relative)?;
    let output_path = workspace_root().join(output_relative);

    let original_bytes =
        fs::read(&input_path).map_err(|error| format!("could not read fixture: {error}"))?;
    let loaded = v2::Fixture::from_json(&original_bytes)
        .map_err(|error| format!("invalid strict-schema fixture: {error}"))?;
    if loaded.fixture.profile != Profile::LongNativeXorpdXmm0Xmm1V1 {
        return Err("reducer accepts only s2-long-native-xorpd-xmm0-xmm1-v1".into());
    }
    let reduction = reduce_native_xorpd_fixture_with_input_digest(
        &loaded.fixture,
        &loaded.input_sha256,
        call_budget,
        |candidate| {
            if candidate == &loaded.fixture {
                return run_native_xorpd_interpreter(&loaded);
            }
            let candidate_bytes = candidate.to_json().map_err(|error| error.to_string())?;
            let candidate_loaded = v2::Fixture::from_json(&candidate_bytes)
                .map_err(|error| format!("candidate fixture failed strict validation: {error}"))?;
            run_native_xorpd_interpreter(&candidate_loaded)
        },
    )?;

    fs::create_dir_all(
        output_path
            .parent()
            .ok_or("output directory has no parent")?,
    )
    .map_err(|error| format!("could not create output parent: {error}"))?;
    fs::create_dir(&output_path)
        .map_err(|error| format!("could not create fresh output directory: {error}"))?;
    write_new(output_path.join("original.case.json"), &original_bytes)?;
    let mut initial_result_sha256 = Vec::with_capacity(reduction.initial_results.len());
    for (index, result) in reduction.initial_results.iter().enumerate() {
        let result_bytes = result.to_json().map_err(|error| error.to_string())?;
        initial_result_sha256.push(sha256_hex(&result_bytes));
        write_new(
            output_path.join(format!("initial-{}.result.json", index + 1)),
            &result_bytes,
        )?;
    }
    let mut final_result_sha256 = Vec::with_capacity(reduction.final_results.len());
    for (index, result) in reduction.final_results.iter().enumerate() {
        let result_bytes = result.to_json().map_err(|error| error.to_string())?;
        final_result_sha256.push(sha256_hex(&result_bytes));
        write_new(
            output_path.join(format!("final-{}.result.json", index + 1)),
            &result_bytes,
        )?;
    }
    let expected_original = reduction
        .initial_results
        .first()
        .map(|reference| expected_native_xorpd_oracle(&loaded, reference))
        .transpose()?;
    let expected_original_sha256 = if let Some(expected) = &expected_original {
        let expected_bytes = expected.to_json().map_err(|error| error.to_string())?;
        write_new(
            output_path.join("expected-original.result.json"),
            &expected_bytes,
        )?;
        Some(sha256_hex(&expected_bytes))
    } else {
        None
    };
    let reduced_bytes = reduction
        .reduced_fixture
        .as_ref()
        .map(|fixture| fixture.to_json().map_err(|error| error.to_string()))
        .transpose()?;
    let mut expected_reduced_sha256 = None;
    if let Some(reduced_bytes) = &reduced_bytes {
        write_new(output_path.join("reduced.case.json"), &reduced_bytes)?;
        let reduced_loaded =
            v2::Fixture::from_json(&reduced_bytes).map_err(|error| error.to_string())?;
        if let Some(reference) = reduction.final_results.first() {
            let expected = expected_native_xorpd_oracle(&reduced_loaded, reference)?;
            let expected_bytes = expected.to_json().map_err(|error| error.to_string())?;
            write_new(
                output_path.join("expected-reduced.result.json"),
                &expected_bytes,
            )?;
            expected_reduced_sha256 = Some(sha256_hex(&expected_bytes));
        }
    }

    let replay_original = format!(
        "od-runner --fixture {} --engine interpreter --json",
        output_path.join("original.case.json").display()
    );
    let replay_reduced = reduction.reduced_fixture.as_ref().map(|_| {
        format!(
            "od-runner --fixture {} --engine interpreter --json",
            output_path.join("reduced.case.json").display()
        )
    });
    let report = serde_json::json!({
        "report_version": 1,
        "profile": "s2-long-native-xorpd-xmm0-xmm1-v1",
        "stop_reason": reduction.stop,
        "stop_detail": reduction.stop_detail,
        "source_fixture": input_path,
        "original_sha256": sha256_hex(&original_bytes),
        "reduced_sha256": reduced_bytes.as_ref().map(|bytes| sha256_hex(bytes)),
        "result_sha256": {
            "initial": initial_result_sha256,
            "final": final_result_sha256,
            "expected_original": expected_original_sha256,
            "expected_reduced": expected_reduced_sha256
        },
        "initial_fingerprint": reduction.initial_fingerprint,
        "final_fingerprint": reduction.final_fingerprint,
        "reducer": "od-testkit-native-xorpd-xmm-input-v1",
        "call_budget": call_budget,
        "actual_callbacks": reduction.actual_callbacks,
        "candidate_proposals": reduction.candidate_proposals,
        "candidate_runs": reduction.candidate_runs,
        "accepted_candidates": reduction.accepted_candidates,
        "initial_hamming_weight": reduction.initial_hamming_weight,
        "final_hamming_weight": reduction.final_hamming_weight,
        "revision": local_revision(),
        "toolchain": local_rustc_version(),
        "provenance": loaded.fixture.provenance,
        "replay_original": replay_original,
        "replay_reduced": replay_reduced,
        "native": "not_run",
        "physical_mac_mini": "not_run; reserved for combined final hardware batch",
        "synthetic_callbacks": "unit tests only; no real CPU bug or hardware evidence",
        "corpus_modified": false,
        "output_files": {
            "original_fixture": "original.case.json",
            "initial_results": reduction.initial_results.len(),
            "final_results": reduction.final_results.len(),
            "expected_original": expected_original.is_some(),
            "reduced_fixture": reduction.reduced_fixture.is_some()
        }
    });
    write_new(
        output_path.join("report.json"),
        &serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )?;
    if reduction.stop != NativeXorpdReductionStop::NoInitialMismatch {
        println!(
            "native XORPD reducer: {:?}; callbacks={}; output={}",
            reduction.stop,
            reduction.actual_callbacks,
            output_path.display()
        );
    } else {
        println!(
            "native XORPD reducer: no_initial_mismatch; no reduced fixture; output={}",
            output_path.display()
        );
    }
    Ok(())
}

fn validate_native_xorpd_output_dir(path: &std::path::Path) -> Result<(), String> {
    use std::path::Component;
    let parts = path.components().collect::<Vec<_>>();
    let allowed = ["work", "stage2", "s2-42b-xorpd"];
    if parts.len() <= allowed.len()
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
        || parts
            .iter()
            .take(allowed.len())
            .zip(allowed)
            .any(|(part, expected)| *part != Component::Normal(std::ffi::OsStr::new(expected)))
    {
        return Err(format!(
            "--output-dir must be a new child directory under {NATIVE_XORPD_REDUCE_WORK_DIR}"
        ));
    }
    let mut prefix = workspace_root();
    for component in parts {
        prefix.push(component.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "--output-dir path contains a symlink: {}",
                    prefix.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(format!("could not inspect output path: {error}")),
        }
    }
    Ok(())
}

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn write_new(path: std::path::PathBuf, bytes: &[u8]) -> Result<(), String> {
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    output
        .write_all(bytes)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn expected_native_xorpd_oracle(
    loaded: &od_testkit::v2::LoadedFixture,
    reference: &od_testkit::v2::RunResult,
) -> Result<od_testkit::v2::RunResult, String> {
    use od_testkit::result::MemoryRegionSnapshot;
    use od_testkit::v2::{Outcome, Snapshot};
    let fixture = &loaded.fixture;
    let sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or("missing SSE state")?;
    let mut state = fixture.initial_state.clone();
    let mut xmm0 = decode_hex_bytes(&sse.xmm[0])?;
    let xmm1 = decode_hex_bytes(&sse.xmm[1])?;
    if xmm0.len() != 16 || xmm1.len() != 16 {
        return Err("invalid XORPD operand width".into());
    }
    for (left, right) in xmm0.iter_mut().zip(xmm1) {
        *left ^= right;
    }
    state.sse.as_mut().ok_or("missing SSE state")?.xmm[0] = encode_hex_bytes(&xmm0);
    state.rip = fixture.stop_rip.ok_or("missing stop RIP")?;
    Ok(od_testkit::v2::RunResult {
        schema_version: od_testkit::v2::SCHEMA_VERSION,
        case_id: fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: fixture.profile,
        backend: reference.backend,
        outcome: Outcome::StopAddress {},
        instructions_executed: Observation::Available { value: HexU64(1) },
        snapshot: Observation::Available {
            value: Snapshot {
                state,
                rflags_defined: fixture.rflags_mask,
                memory: fixture
                    .memory
                    .iter()
                    .map(|region| MemoryRegionSnapshot {
                        region_id: region.region_id.clone(),
                        guest_address: region.guest_address,
                        bytes_hex: region.initial_bytes_hex.clone(),
                    })
                    .collect(),
                xorpd_prerequisites: None,
                minss_invalid_controls: None,
                maxss_unmasked_controls: None,
                minsd_unmasked_controls: None,
                maxsd_unmasked_controls: None,
                minpd_unmasked_controls: None,
                minps_memory_rax_controls: None,
                sqrtps_memory_rax_controls: None,
                sqrtpd_memory_rax_controls: None,
                sqrtsd_memory_rax_controls: None,
                minps_unmasked_controls: None,
                maxps_unmasked_controls: None,
                maxpd_unmasked_controls: None,
                cvtdq2pd_controls: None,
                cvtdq2pd_memory_controls: None,
                cvtdq2ps_controls: None,
                cvtdq2ps_memory_controls: None,
                cvtps2dq_controls: None,
                cvttps2dq_controls: None,
                cvttps2dq_m128_controls: None,
                cvtps2dq_m128_controls: None,
                cvtpd2dq_controls: None,
                cvttpd2dq_controls: None,
                cvttpd2dq_m128_controls: None,
                cvtpd2dq_m128_controls: None,
                sqrtss_controls: None,
                sqrtsd_controls: None,
                sqrtps_controls: None,
                sqrtpd_controls: None,
                subpd_controls: None,
                mulpd_controls: None,
                addpd_controls: None,
                cvtpd2ps_controls: None,
                addsd_controls: None,
                addss_controls: None,
                addps_controls: None,
                divpd_controls: None,
                divsd_controls: None,
                divps_controls: None,
                divss_controls: None,
                mulsd_controls: None,
                mulss_controls: None,
                mulps_controls: None,
                subps_controls: None,
                subsd_controls: None,
                subss_controls: None,
                cvtss2sd_controls: None,
                cvtsd2ss_controls: None,
                cvtpi2pd_memory_controls: None,
                cvtpi2pd_register_controls: None,
                cvtpi2ps_memory_controls: None,
                cvtpi2ps_register_controls: None,
                cvttps2pi_memory_controls: None,
                cvtps2pi_memory_controls: None,
                cvtps2pi_register_controls: None,
                cvtpd2pi_register_controls: None,
                cvttpd2pi_register_controls: None,
                cvttpd2pi_memory_controls: None,
                cvtpd2pi_memory_controls: None,
                cvtps2pd_controls: None,
                cvtps2pd_m64_controls: None,
                cvtss2sd_m32_controls: None,
                cvtsd2ss_m64_controls: None,
                cvtpd2ps_m128_controls: None,
                comiss_ucomiss_controls: None,
                cmpss_controls: None,
                cmpsd_controls: None,
                cmppd_controls: None,
                cmpps_controls: None,
                comisd_ucomisd_controls: None,
                comisd_memory_rax_controls: None,
                ucomisd_memory_rax_controls: None,
                comiss_memory_rax_controls: None,
                ucomiss_memory_rax_controls: None,
                cmpss_memory_rax_controls: None,
                cmpsd_memory_rax_controls: None,
                cmppd_memory_rax_controls: None,
                cmpps_memory_rax_controls: None,
                minsd_memory_rax_controls: None,
                maxss_memory_rax_controls: None,
                maxsd_memory_rax_controls: None,
                minpd_memory_rax_controls: None,
                maxpd_memory_rax_controls: None,
                maxps_memory_rax_controls: None,
                cvtsi2sd_r32_controls: None,
                cvtsi2sd_r32_memory_controls: None,
                cvtsi2ss_r32_memory_controls: None,
                cvtsi2ss_r64_memory_controls: None,
                cvtsi2sd_r64_controls: None,
                cvtsi2sd_r64_memory_controls: None,
                cvtsi2ss_controls: None,
                cvtss2si_controls: None,
                cvtsd2si_controls: None,
                cvtsd2si_r32_memory_controls: None,
                cvtsd2si_r64_memory_controls: None,
                cvttsd2si_r32_memory_controls: None,
                cvttsd2si_r64_memory_controls: None,
                cvtss2si_r32_memory_controls: None,
                cvtss2si_r64_memory_controls: None,
                cvttss2si_r32_memory_controls: None,
                cvttss2si_r64_memory_controls: None,
                fxsave64_cr0_controls: None,
                fxrstor64_cr0_controls: None,
                rdtsc_source_calls: None,
                exception: Observation::Available { value: None },
            },
        },
        build: reference.build.clone(),
        machine: reference.machine.clone(),
    })
}

fn validate_native_xorpd_result(
    loaded: &od_testkit::v2::LoadedFixture,
    result: &od_testkit::v2::RunResult,
) -> Result<(), String> {
    use od_testkit::v2::Outcome;

    let case_id = &loaded.fixture.case_id;
    if result.profile != od_testkit::v2::Profile::LongNativeXorpdXmm0Xmm1V1
        || result.backend != Backend::Interpreter
        || !matches!(result.outcome, Outcome::StopAddress {})
        || result.instructions_executed != (Observation::Available { value: HexU64(1) })
    {
        return Err(format!(
            "mandatory interpreter execution did not complete for {case_id}"
        ));
    }
    let Observation::Available { value: snapshot } = &result.snapshot else {
        return Err(format!("interpreter result is missing for {case_id}"));
    };
    let initial_sse = loaded
        .fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or_else(|| format!("missing SSE input for {case_id}"))?;
    let mut expected_state = loaded.fixture.initial_state.clone();
    let mut expected_xmm0 = decode_hex_bytes(&initial_sse.xmm[0])?;
    let xmm1 = decode_hex_bytes(&initial_sse.xmm[1])?;
    if expected_xmm0.len() != 16 || xmm1.len() != 16 {
        return Err(format!("XMM operand length is invalid for {case_id}"));
    }
    for (destination, source) in expected_xmm0.iter_mut().zip(xmm1) {
        *destination ^= source;
    }
    expected_state
        .sse
        .as_mut()
        .expect("strict native XORPD schema requires SSE state")
        .xmm[0] = encode_hex_bytes(&expected_xmm0);
    expected_state.rip = loaded.fixture.stop_rip.expect("strict fixture stop RIP");
    if snapshot.state != expected_state || snapshot.rflags_defined != loaded.fixture.rflags_mask {
        return Err(format!(
            "full state differs from independent XOR oracle for {case_id}"
        ));
    }
    if snapshot.memory.len() != 1
        || snapshot.memory[0].region_id != "code"
        || snapshot.memory[0].guest_address != loaded.fixture.memory[0].guest_address
        || snapshot.memory[0].bytes_hex != loaded.fixture.memory[0].initial_bytes_hex
    {
        return Err(format!("code mapping changed or is absent for {case_id}"));
    }
    if !matches!(snapshot.exception, Observation::Available { value: None }) {
        return Err(format!("unexpected exception observation for {case_id}"));
    }
    Ok(())
}

fn validate_native_xorpd_file_count(actual: usize, expected: usize) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected {expected} case files, found {actual}"))
    }
}

fn validate_native_xorpd_case_digest(
    bytes: &[u8],
    expected_digest: &str,
    case_id: &str,
) -> Result<(), String> {
    if sha256_hex(bytes) == expected_digest {
        Ok(())
    } else {
        Err(format!("case digest mismatch for {case_id}"))
    }
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn local_revision() -> String {
    if let Some(revision) = option_env!("OPEN_DESKTOP_GIT_REVISION") {
        return revision.to_string();
    }
    let Some(revision) = command_output("git", &["rev-parse", "HEAD"]) else {
        return "unavailable".into();
    };
    let dirty = command_output("git", &["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|status| !status.is_empty());
    if dirty {
        format!("{revision}+dirty")
    } else {
        revision
    }
}

fn local_rustc_version() -> String {
    option_env!("RUSTC_VERSION")
        .map(ToString::to_string)
        .or_else(|| command_output("rustc", &["--version"]))
        .unwrap_or_else(|| "rustc version unavailable".into())
}

fn run_native_xorpd_interpreter(
    loaded: &od_testkit::v2::LoadedFixture,
) -> Result<od_testkit::v2::RunResult, String> {
    let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture)
        .map_err(|error| error.to_string())?;
    let result = execute_fixture_v2(loaded, &mut memory);
    result
        .validate_for(loaded)
        .map_err(|error| format!("invalid interpreter result: {error}"))?;
    Ok(result)
}

fn decode_hex_bytes(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("invalid odd-length XMM byte string".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
            u8::from_str_radix(pair, 16).map_err(|error| error.to_string())
        })
        .collect()
}

fn encode_hex_bytes(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

pub fn convert_fixture_v1(args: &[String]) -> ExitCode {
    if args.len() != 2 {
        return invalid_input("usage: od-runner --convert-fixture-v1 CASE_V1.json CASE_V2.json");
    }
    let conversion = (|| -> Result<(), String> {
        let bytes = fs::read(&args[0]).map_err(|e| e.to_string())?;
        let input = Fixture::from_json(&bytes).map_err(|e| e.to_string())?;
        let output = od_testkit::v2::Fixture::convert_v1(&input).map_err(|e| e.to_string())?;
        let bytes = output.to_json().map_err(|e| e.to_string())?;
        // Explicit output and create_new preserve both historical and user files.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[1])
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        println!(
            "converted_fixture_sha256={}",
            od_testkit::sha256_hex(&bytes)
        );
        Ok(())
    })();
    match conversion {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => invalid_input(&error),
    }
}

pub fn generate_corpus(args: &[String]) -> ExitCode {
    if args.len() != 3 {
        return invalid_input("usage: od-runner --generate-corpus SEED COUNT OUTPUT_DIR");
    }
    let seed = match parse_u64(&args[0]) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error),
    };
    let count = match args[1].parse::<u32>() {
        Ok(value) if (1..=100_000).contains(&value) => value,
        _ => return invalid_input("COUNT must be an integer from 1 to 100000"),
    };
    let directory = std::path::Path::new(&args[2]);
    if let Err(error) = fs::create_dir(directory) {
        return invalid_input(&format!("could not create output directory: {error}"));
    }
    let mut corpus = Vec::new();
    for sequence in 0..count {
        let fixture = generate_fixture(seed, sequence);
        let bytes = match fixture.to_json() {
            Ok(bytes) => bytes,
            Err(error) => return invalid_input(&format!("generated fixture is invalid: {error}")),
        };
        corpus.extend_from_slice(&bytes);
        let path = directory.join(format!("{}.case.json", fixture.case_id));
        if let Err(error) = fs::write(path, bytes) {
            return invalid_input(&format!("could not write generated fixture: {error}"));
        }
    }
    println!(
        "generated {count} fixtures with seed 0x{seed:016x}; corpus_sha256={}",
        od_testkit::sha256_hex(&corpus)
    );
    ExitCode::SUCCESS
}

pub fn minimize_fixture(args: &[String]) -> ExitCode {
    let Some(input_path) = args.first() else {
        return invalid_input(
            "usage: od-runner --minimize CASE.json --output REDUCED.json [--max-attempts N]",
        );
    };
    let mut output_path = None;
    let mut max_attempts = 1_000usize;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--output" => {
                index += 1;
                output_path = args.get(index).map(String::as_str);
                if output_path.is_none() {
                    return invalid_input("missing value for --output");
                }
            }
            "--max-attempts" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return invalid_input("missing value for --max-attempts");
                };
                max_attempts = match value.parse::<usize>() {
                    Ok(value) if (2..=100_000).contains(&value) => value,
                    _ => return invalid_input("--max-attempts must be between 2 and 100000"),
                };
            }
            option => return invalid_input(&format!("unexpected minimizer argument '{option}'")),
        }
        index += 1;
    }
    let Some(output_path) = output_path else {
        return invalid_input("--minimize requires --output REDUCED.json");
    };
    let input = match fs::read(input_path) {
        Ok(input) => input,
        Err(error) => return invalid_input(&format!("could not read fixture: {error}")),
    };
    let loaded = match Fixture::from_json(&input) {
        Ok(loaded) => loaded,
        Err(error) => return invalid_input(&format!("invalid fixture: {error}")),
    };
    let result = minimize_fixture_value(&loaded.fixture, max_attempts, local_mismatch_class);
    let result = match result {
        Ok(result) => result,
        Err(MinimizeError::NoInitialMismatch) => {
            eprintln!("no stable interpreter/JIT mismatch exists for this fixture");
            return ExitCode::from(EXIT_MISMATCH);
        }
        Err(MinimizeError::FlakyInitialResult { .. }) => {
            eprintln!("initial mismatch was not stable across two executions");
            return ExitCode::from(EXIT_HARNESS_FAILURE);
        }
        Err(error) => return invalid_input(&format!("cannot minimize fixture: {error:?}")),
    };
    let bytes = match result.fixture.to_json() {
        Ok(bytes) => bytes,
        Err(error) => return invalid_input(&format!("minimized fixture is invalid: {error}")),
    };
    if let Err(error) = fs::write(output_path, bytes) {
        return invalid_input(&format!("could not write minimized fixture: {error}"));
    }
    println!(
        "minimized {}: {} -> {} instructions in {} attempts; mismatch={}",
        result.fixture.case_id,
        result.original_instruction_count,
        result.minimized_instruction_count,
        result.attempts,
        result.mismatch_class
    );
    ExitCode::SUCCESS
}

fn local_mismatch_class(fixture: &Fixture) -> Option<String> {
    let bytes = fixture.to_json().ok()?;
    let loaded = Fixture::from_json(&bytes).ok()?;
    let interpreter = execute_fixture(&loaded, Backend::Interpreter).ok()?;
    let jit = execute_fixture(&loaded, Backend::Jit).ok()?;
    let report = compare_result_values(&loaded, &interpreter, &jit).ok()?;
    report
        .differences
        .first()
        .map(|difference| difference.field.clone())
}

fn parse_u64(input: &str) -> Result<u64, String> {
    if let Some(hex) = input.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
            .map_err(|_| "SEED must be a decimal or 0x-prefixed u64".to_string())
    } else {
        input
            .parse()
            .map_err(|_| "SEED must be a decimal or 0x-prefixed u64".to_string())
    }
}

pub fn run_fixture(args: &[String]) -> ExitCode {
    let mut fixture_path = None;
    let mut engine = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "--engine" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return invalid_input("missing value for --engine");
                };
                engine = Some(value.as_str());
            }
            value if fixture_path.is_none() => fixture_path = Some(value),
            value => return invalid_input(&format!("unexpected fixture argument '{value}'")),
        }
        index += 1;
    }
    if !json {
        return invalid_input("--fixture requires --json");
    }
    let Some(fixture_path) = fixture_path else {
        return invalid_input(
            "usage: od-runner --fixture CASE.json --engine interpreter|jit|native --json",
        );
    };
    let backend = match engine {
        Some("interpreter") => Backend::Interpreter,
        Some("jit") => Backend::Jit,
        Some("native") => Backend::NativeX86,
        Some(other) => return invalid_input(&format!("unknown fixture engine '{other}'")),
        None => return invalid_input("--fixture requires --engine interpreter|jit|native"),
    };

    let input = match fs::read(fixture_path) {
        Ok(input) => input,
        Err(error) => return invalid_input(&format!("could not read fixture: {error}")),
    };
    match fixture_schema_version(&input) {
        Ok(2) => return run_fixture_v2(&input, backend),
        Ok(1) => {}
        Ok(version) => return invalid_input(&format!("unsupported fixture schema {version}")),
        Err(error) => return invalid_input(&error),
    }
    let loaded = match Fixture::from_json(&input) {
        Ok(loaded) => loaded,
        Err(error) => return invalid_input(&format!("invalid fixture: {error}")),
    };
    let result = match execute_fixture(&loaded, backend) {
        Ok(result) => result,
        Err(ExecutionFailure::Unsupported(reason)) => {
            return emit_result(
                unsupported_result(&loaded, backend, reason),
                EXIT_UNSUPPORTED,
            );
        }
        Err(ExecutionFailure::InvalidInput(reason)) => {
            return emit_result(
                invalid_fixture_result(&loaded, backend, reason),
                EXIT_INVALID_INPUT,
            );
        }
        Err(ExecutionFailure::Harness(reason)) => {
            eprintln!("harness failure: {reason}");
            return ExitCode::from(EXIT_HARNESS_FAILURE);
        }
    };
    emit_result(result, 0)
}

fn fixture_schema_version(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() > od_testkit::v2::MAX_FIXTURE_JSON_BYTES {
        return Err("fixture JSON exceeds the v2 size limit".into());
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "fixture requires an unsigned integer schema_version".into())
}

fn run_fixture_v2(bytes: &[u8], backend: Backend) -> ExitCode {
    use od_testkit::v2;
    let loaded = match v2::Fixture::from_json(bytes) {
        Ok(value) => value,
        Err(error) => return invalid_input(&format!("invalid v2 fixture: {error}")),
    };
    let result = if backend == Backend::Interpreter {
        let mut memory = match od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error.to_string()),
        };
        execute_fixture_v2(&loaded, &mut memory)
    } else {
        v2::RunResult {
            schema_version: v2::SCHEMA_VERSION,
            case_id: loaded.fixture.case_id.clone(),
            input_sha256: loaded.input_sha256.clone(),
            profile: loaded.fixture.profile,
            backend,
            outcome: v2::Outcome::Unsupported {
                reason: "schema v2 execution is implemented only for the interpreter".into(),
            },
            instructions_executed: unavailable("engine was not executed"),
            snapshot: unavailable("engine was not executed"),
            build: build_metadata(),
            machine: machine_metadata(),
        }
    };
    let exit = match result.outcome {
        v2::Outcome::Unsupported { .. } => EXIT_UNSUPPORTED,
        v2::Outcome::TimedOut { .. }
        | v2::Outcome::Signaled { .. }
        | v2::Outcome::HarnessError { .. } => EXIT_HARNESS_FAILURE,
        _ => 0,
    };
    match result.to_json() {
        Ok(json) => match write_json(&json) {
            Ok(()) => ExitCode::from(exit),
            Err(error) => {
                eprintln!("harness failure: could not write v2 result: {error}");
                ExitCode::from(EXIT_HARNESS_FAILURE)
            }
        },
        Err(error) => {
            eprintln!("harness failure: could not serialize v2 result: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

fn is_repne_addr32_string(bytes: &[u8]) -> bool {
    (3..=bytes.len().min(5)).any(|length| {
        let candidate = &bytes[..length];
        let Some((&opcode, prefixes)) = candidate.split_last() else {
            return false;
        };
        if !matches!(opcode, 0xae | 0xa6 | 0xaf | 0xa7) {
            return false;
        }
        if matches!(opcode, 0xae | 0xa6) {
            return matches!(prefixes, [0xf2, 0x67] | [0x67, 0xf2]);
        }
        matches!(
            prefixes,
            [0xf2, 0x67]
                | [0x67, 0xf2]
                | [0xf2, 0x67, 0x66]
                | [0x67, 0xf2, 0x66]
                | [0xf2, 0x48..=0x4f, 0x67]
                | [0xf2, 0x67, 0x48..=0x4f]
                | [0x67, 0xf2, 0x48..=0x4f]
                | [0xf2, 0x67, 0x48..=0x4f, 0x66]
                | [0xf2, 0x67, 0x66, 0x48..=0x4f]
        )
    })
}

fn execute_fixture_v2(
    loaded: &od_testkit::v2::LoadedFixture,
    memory: &mut od_testkit::memory::FlatMemory,
) -> od_testkit::v2::RunResult {
    use od_isa::execute::{
        CmppdControls, CmppdMemoryRaxControls, CmppsControls, CmppsMemoryRaxControls,
        CmpsdControls, CmpsdMemoryRaxControls, CmpssControls, CmpssMemoryRaxControls,
        ComisdMemoryRaxControls, ComisdUcomisdControls, ComissUcomissControls, Cr0Profile,
        Cr4Profile, Cvtdq2pdControls, Cvtdq2pdMemoryControls, Cvtdq2psControls,
        Cvtdq2psMemoryControls, Cvtpd2dqControls, Cvtps2dqControls, Cvtps2pdControls,
        Cvtsd2siMemoryR32Controls, Cvtsd2siMemoryR64Controls, Cvtsd2siRegistersControls,
        Cvtsi2sdR32Controls, Cvtsi2sdR64Controls, Cvtsi2sdR64MemoryControls,
        Cvtsi2ssR32MemoryControls, Cvtsi2ssR64MemoryControls, Cvtsi2ssRegistersControls,
        Cvtss2sdControls, Cvtss2siMemoryR32Controls, Cvtss2siMemoryR64Controls,
        Cvtss2siRegistersControls, Cvttsd2siMemoryR32Controls, Cvttsd2siMemoryR64Controls,
        Cvttss2siMemoryR32Controls, Cvttss2siMemoryR64Controls, Fxrstor64Cr0Controls,
        Fxsave64Cr0Controls, MaxpdUnmaskedControls, MaxpsUnmaskedControls, MaxsdMemoryRaxControls,
        MaxsdUnmaskedControls, MaxssMemoryRaxControls, MaxssUnmaskedControls,
        MinpdUnmaskedControls, MinpsUnmaskedControls, MinsdMemoryRaxControls,
        MinsdUnmaskedControls, MinssInvalidControls, RdtscControls, RunConfig, RunExit,
        XorpdPrerequisiteState, run, run_with_cli_profile, run_with_cmppd_memory_rax_controls,
        run_with_cmppd_register_controls, run_with_cmpps_memory_rax_controls,
        run_with_cmpps_register_controls, run_with_cmpsd_memory_rax_controls,
        run_with_cmpsd_register_controls, run_with_cmpss_memory_rax_controls,
        run_with_cmpss_register_controls, run_with_comisd_memory_rax_controls,
        run_with_comisd_ucomisd_register_controls, run_with_comiss_memory_rax_controls,
        run_with_comiss_ucomiss_register_controls, run_with_cr0_profile,
        run_with_cvtdq2pd_controls, run_with_cvtdq2pd_memory_controls,
        run_with_cvtdq2pd_register_controls, run_with_cvtdq2ps_memory_controls,
        run_with_cvtdq2ps_register_controls, run_with_cvtps2dq_register_controls,
        run_with_cvtps2pd_register_controls, run_with_cvtsd2si_r32_memory_controls,
        run_with_cvtsd2si_r64_memory_controls, run_with_cvtsd2si_registers_controls,
        run_with_cvtsi2sd_r32_controls, run_with_cvtsi2sd_r32_memory_controls,
        run_with_cvtsi2sd_r64_controls, run_with_cvtsi2sd_r64_memory_controls,
        run_with_cvtsi2sd_registers_controls, run_with_cvtsi2ss_r32_memory_controls,
        run_with_cvtsi2ss_r64_memory_controls, run_with_cvtsi2ss_registers_controls,
        run_with_cvtss2sd_register_controls, run_with_cvtss2si_r32_memory_controls,
        run_with_cvtss2si_r64_memory_controls, run_with_cvtss2si_registers_controls,
        run_with_cvttsd2si_r32_memory_controls, run_with_cvttsd2si_r64_memory_controls,
        run_with_cvttss2si_r32_memory_controls, run_with_cvttss2si_r64_memory_controls,
        run_with_efer_rdmsr_profile, run_with_fxrstor64_cr0_controls,
        run_with_fxsave64_cr0_controls, run_with_halt_profile, run_with_kernel_gs_base_profile,
        run_with_maxpd_memory_rax_controls, run_with_maxpd_unmasked_controls,
        run_with_maxps_unmasked_controls, run_with_maxsd_memory_rax_controls,
        run_with_maxsd_unmasked_controls, run_with_maxss_memory_rax_controls,
        run_with_maxss_unmasked_controls, run_with_minpd_memory_rax_controls,
        run_with_minpd_unmasked_controls, run_with_minps_unmasked_controls,
        run_with_minsd_memory_rax_controls, run_with_minsd_unmasked_controls,
        run_with_minss_invalid_controls, run_with_minss_unmasked_denormal_controls,
        run_with_msr_profile, run_with_rdtsc_source, run_with_rep_continuation,
        run_with_sep_profile, run_with_sti_profile, run_with_swapgs_profile,
        run_with_ucomisd_memory_rax_controls, run_with_ucomiss_memory_rax_controls,
        run_with_xorpd_prerequisites,
    };
    use od_testkit::v2;
    use std::time::{Duration, Instant};

    let mut state = loaded
        .fixture
        .initial_state
        .to_cpu_for(loaded.fixture.profile)
        .expect("validated fixture state converts to CPU state");
    let start = Instant::now();
    let limit = Duration::from_millis(u64::from(loaded.fixture.wall_timeout_ms));
    let mut timed_out = false;
    // A cooperative deadline is checked at the same instruction boundaries as
    // STOP, before budget/fetch. It cannot interrupt a stalled backend call.
    let context = if matches!(
        loaded.fixture.profile,
        v2::Profile::LongCliV1
            | v2::Profile::LongStiV1
            | v2::Profile::LongHltV1
            | v2::Profile::LongIretqCpl0ValidV1
            | v2::Profile::LongLretqCpl0ValidV1
            | v2::Profile::LongCr0FpInitV1
            | v2::Profile::LongCr4FpInitV1
            | v2::Profile::LongCr0Cr4FpInitV1
            | v2::Profile::LongCr0Cr4FpInitFxsave64V1
            | v2::Profile::LongCr0Cr4FpInitFxrstor64V1
            | v2::Profile::LongCr0Cr4FpInitFxrstor64MinssV1
            | v2::Profile::LongEferRdmsrV1
            | v2::Profile::LongCvttpd2piMemoryRaxV1
            | v2::Profile::LongCvtpd2piMemoryRaxV1
    ) {
        let cpl = if loaded.fixture.profile == v2::Profile::LongEferRdmsrV1 {
            loaded.fixture.efer_cpl.expect("validated EFER CPL")
        } else {
            loaded.fixture.cli_cpl.expect("validated CLI CPL")
        };
        let privilege = match cpl {
            v2::CliCpl::Cpl0 => od_isa::memory::PrivilegeLevel::Ring0,
            v2::CliCpl::Cpl1 => od_isa::memory::PrivilegeLevel::Ring1,
            v2::CliCpl::Cpl2 => od_isa::memory::PrivilegeLevel::Ring2,
            v2::CliCpl::Cpl3 => od_isa::memory::PrivilegeLevel::Ring3,
        };
        od_isa::memory::AccessContext {
            privilege,
            ..od_isa::memory::AccessContext::default()
        }
    } else if matches!(
        loaded.fixture.profile,
        v2::Profile::LongNullDsEsV1
            | v2::Profile::LongFsBaseMsrV1
            | v2::Profile::LongSwapgsV1
            | v2::Profile::LongKernelGsBaseV1
    ) {
        od_isa::memory::AccessContext {
            privilege: od_isa::memory::PrivilegeLevel::Ring0,
            ..od_isa::memory::AccessContext::default()
        }
    } else {
        od_isa::memory::AccessContext::default()
    };
    let config = RunConfig {
        instruction_budget: loaded.fixture.instruction_budget,
        stop_rip: loaded.fixture.stop_rip.map(HexU64::get),
        context,
    };
    let timeout = || {
        timed_out = start.elapsed() >= limit;
        timed_out
    };
    let mut rdtsc_source_calls = None;
    let entry_bytes = {
        let mut bytes = Vec::with_capacity(5);
        for offset in 0..5u64 {
            let Some(address) = state.rip.checked_add(offset) else {
                break;
            };
            let Some(region) = loaded.fixture.memory.iter().find(|region| {
                let base = region.guest_address.get();
                region.permissions.execute
                    && address >= base
                    && address - base < u64::from(region.length)
            }) else {
                break;
            };
            let Ok(region_bytes) = od_testkit::v2::decode_bytes(&region.initial_bytes_hex) else {
                break;
            };
            let offset = usize::try_from(address - region.guest_address.get()).ok();
            let Some(byte) = offset.and_then(|index| region_bytes.get(index)) else {
                break;
            };
            bytes.push(*byte);
        }
        (!bytes.is_empty()).then_some(bytes)
    };
    let execution = if entry_bytes.as_deref().is_some_and(is_repne_addr32_string) {
        let mut continuation = None;
        run_with_rep_continuation(
            &mut state,
            memory,
            config,
            od_isa::INTEGER_FLAGS,
            &mut continuation,
            timeout,
        )
        .expect("fixture supplies a valid fresh REP continuation boundary")
    } else if loaded.fixture.profile == v2::Profile::LongEferRdmsrV1 {
        run_with_efer_rdmsr_profile(
            &mut state,
            memory,
            config,
            loaded
                .fixture
                .efer_value
                .expect("validated EFER value")
                .get(),
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongFsBaseMsrV1 {
        run_with_msr_profile(
            &mut state,
            memory,
            config,
            &od_isa::msr::MsrProfile::FsBaseV1,
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSepDisabledV1 {
        run_with_sep_profile(
            &mut state,
            memory,
            config,
            &od_isa::sep::SepProfile::SepDisabledV1,
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSwapgsV1 {
        run_with_swapgs_profile(
            &mut state,
            memory,
            config,
            &od_isa::SwapgsProfile::SwapgsV1,
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCliV1 {
        run_with_cli_profile(
            &mut state,
            memory,
            config,
            &od_isa::CliProfile::NonPviLong64V1,
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongStiV1 {
        run_with_sti_profile(
            &mut state,
            memory,
            config,
            &od_isa::StiProfile::NonPviLong64V1,
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongHltV1 {
        run_with_halt_profile(
            &mut state,
            memory,
            config,
            &od_isa::HaltProfile::Long64V1,
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCr0FpInitV1 {
        run_with_cr0_profile(
            &mut state,
            memory,
            config,
            Cr0Profile::FpInitLong64V1,
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCr0Cr4FpInitFxrstor64MinssV1 {
        od_isa::execute::run_with_cr4_fxrstor64_minss_profile(&mut state, memory, config, timeout)
    } else if matches!(
        loaded.fixture.profile,
        v2::Profile::LongCr4FpInitV1
            | v2::Profile::LongCr0Cr4FpInitV1
            | v2::Profile::LongCr0Cr4FpInitFxsave64V1
            | v2::Profile::LongCr0Cr4FpInitFxrstor64V1
    ) {
        od_isa::execute::run_with_cr4_profile(
            &mut state,
            memory,
            config,
            match loaded.fixture.profile {
                v2::Profile::LongCr0Cr4FpInitFxsave64V1 => Cr4Profile::FpInitThenFxsave64Long64V1,
                v2::Profile::LongCr0Cr4FpInitFxrstor64V1 => Cr4Profile::FpInitThenFxrstor64Long64V1,
                _ => Cr4Profile::FpInitLong64V1,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongIretqCpl0ValidV1 {
        use od_isa::execute::RunResult as CpuRunResult;
        let execution = od_isa::run_with_iretq_profile(
            &mut state,
            memory,
            config,
            od_isa::INTEGER_FLAGS,
            od_isa::IretqProfile::SameCpl0ValidFrameV1,
            timeout,
        );
        let exit = match execution.exit {
            od_isa::IretqRunExit::StopAddress => RunExit::StopAddress,
            od_isa::IretqRunExit::StopRequested => RunExit::StopRequested,
            od_isa::IretqRunExit::InstructionBudgetExhausted => RunExit::InstructionBudgetExhausted,
            od_isa::IretqRunExit::Unsupported => RunExit::Unsupported { opcode: 0xcf },
            od_isa::IretqRunExit::Fault(fault) => RunExit::Fault(fault),
            od_isa::IretqRunExit::ReadAborted { phase, cause } => {
                RunExit::MemoryError(od_isa::memory::MemoryError::Backend(format!(
                    "IRETQ {phase:?} read abort: {cause:?}"
                )))
            }
        };
        CpuRunResult {
            exit,
            instructions_executed: execution.instructions_executed,
            rflags_defined: execution.rflags_defined,
        }
    } else if loaded.fixture.profile == v2::Profile::LongLretqCpl0ValidV1 {
        use od_isa::execute::RunResult as CpuRunResult;
        let execution = od_isa::run_with_lretq_profile(
            &mut state,
            memory,
            config,
            od_isa::INTEGER_FLAGS,
            od_isa::LretqProfile::SameCpl0ValidFrameV1,
            timeout,
        );
        let exit = match execution.exit {
            od_isa::LretqRunExit::StopAddress => RunExit::StopAddress,
            od_isa::LretqRunExit::StopRequested => RunExit::StopRequested,
            od_isa::LretqRunExit::InstructionBudgetExhausted => RunExit::InstructionBudgetExhausted,
            od_isa::LretqRunExit::Unsupported => RunExit::Unsupported { opcode: 0xcb },
            od_isa::LretqRunExit::Fault(fault) => RunExit::Fault(fault),
            od_isa::LretqRunExit::ReadAborted { phase, cause } => {
                RunExit::MemoryError(od_isa::memory::MemoryError::Backend(format!(
                    "LRETQ {phase:?} read abort: {cause:?}"
                )))
            }
        };
        CpuRunResult {
            exit,
            instructions_executed: execution.instructions_executed,
            rflags_defined: execution.rflags_defined,
        }
    } else if loaded.fixture.profile == v2::Profile::LongKernelGsBaseV1 {
        run_with_kernel_gs_base_profile(&mut state, memory, config, timeout)
    } else if loaded.fixture.profile == v2::Profile::LongRdtscV1 {
        let controls = loaded
            .fixture
            .rdtsc_controls
            .expect("validated RDTSC profile has explicit source and control state");
        let mut calls = 0u64;
        let value = controls.source_value.get();
        let mut source = || {
            calls += 1;
            value
        };
        let execution = run_with_rdtsc_source(
            &mut state,
            memory,
            config,
            RdtscControls {
                cr4_tsd: controls.cr4_tsd,
            },
            &mut source,
            timeout,
        );
        rdtsc_source_calls = Some(calls);
        execution
    } else if loaded.fixture.profile == v2::Profile::LongXorpdPrerequisitesV1 {
        let controls = loaded
            .fixture
            .xorpd_prerequisites
            .expect("validated XORPD profile has explicit prerequisite state");
        run_with_xorpd_prerequisites(
            &mut state,
            memory,
            config,
            XorpdPrerequisiteState {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongFxsave64Cr0GatesV1 {
        let controls = loaded
            .fixture
            .fxsave64_cr0_controls
            .expect("validated FXSAVE64 profile has explicit CR0 controls");
        run_with_fxsave64_cr0_controls(
            &mut state,
            memory,
            config,
            Fxsave64Cr0Controls {
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongFxrstor64Cr0GatesV1 {
        let controls = loaded
            .fixture
            .fxrstor64_cr0_controls
            .expect("validated FXRSTOR64 profile has explicit CR0 controls");
        run_with_fxrstor64_cr0_controls(
            &mut state,
            memory,
            config,
            Fxrstor64Cr0Controls {
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinssInvalidControlsV1 {
        let controls = loaded
            .fixture
            .minss_invalid_controls
            .expect("validated MINSS profile has explicit controls");
        run_with_minss_invalid_controls(
            &mut state,
            memory,
            config,
            MinssInvalidControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinssUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .minss_invalid_controls
            .expect("validated MINSS profile has explicit controls");
        run_with_minss_unmasked_denormal_controls(
            &mut state,
            memory,
            config,
            MinssInvalidControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxssUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .maxss_unmasked_controls
            .expect("validated MAXSS profile has explicit controls");
        run_with_maxss_unmasked_controls(
            &mut state,
            memory,
            config,
            MaxssUnmaskedControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinsdUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .minsd_unmasked_controls
            .expect("validated MINSD profile has explicit controls");
        run_with_minsd_unmasked_controls(
            &mut state,
            memory,
            config,
            MinsdUnmaskedControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxsdUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .maxsd_unmasked_controls
            .expect("validated MAXSD profile has explicit controls");
        run_with_maxsd_unmasked_controls(
            &mut state,
            memory,
            config,
            MaxsdUnmaskedControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinpdUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .minpd_unmasked_controls
            .expect("validated MINPD profile has explicit controls");
        run_with_minpd_unmasked_controls(
            &mut state,
            memory,
            config,
            MinpdUnmaskedControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinpsUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .minps_unmasked_controls
            .expect("validated MINPS profile has explicit controls");
        run_with_minps_unmasked_controls(
            &mut state,
            memory,
            config,
            MinpsUnmaskedControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxpsUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .maxps_unmasked_controls
            .expect("validated MAXPS profile has explicit controls");
        run_with_maxps_unmasked_controls(
            &mut state,
            memory,
            config,
            MaxpsUnmaskedControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxpdUnmaskedDenormalV1 {
        let controls = loaded
            .fixture
            .maxpd_unmasked_controls
            .expect("validated MAXPD profile has explicit controls");
        run_with_maxpd_unmasked_controls(
            &mut state,
            memory,
            config,
            MaxpdUnmaskedControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtdq2pdMemoryV1 {
        let controls = loaded
            .fixture
            .cvtdq2pd_memory_controls
            .expect("validated CVTDQ2PD m64 profile has explicit controls");
        run_with_cvtdq2pd_memory_controls(
            &mut state,
            memory,
            config,
            Cvtdq2pdMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if matches!(
        loaded.fixture.profile,
        v2::Profile::LongCvtdq2pdRegExactV1 | v2::Profile::LongCvtdq2pdRegistersV1
    ) {
        let controls = loaded
            .fixture
            .cvtdq2pd_controls
            .expect("validated CVTDQ2PD profile has explicit controls");
        let run = if loaded.fixture.profile == v2::Profile::LongCvtdq2pdRegExactV1 {
            run_with_cvtdq2pd_controls
        } else {
            run_with_cvtdq2pd_register_controls
        };
        run(
            &mut state,
            memory,
            config,
            Cvtdq2pdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtdq2psMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtdq2ps_memory_controls
            .expect("validated CVTDQ2PS m128 controls");
        run_with_cvtdq2ps_memory_controls(
            &mut state,
            memory,
            config,
            Cvtdq2psMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtdq2psRegistersV1 {
        let controls = loaded
            .fixture
            .cvtdq2ps_controls
            .expect("validated CVTDQ2PS controls");
        run_with_cvtdq2ps_register_controls(
            &mut state,
            memory,
            config,
            Cvtdq2psControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtps2dqRegistersV1 {
        let controls = loaded
            .fixture
            .cvtps2dq_controls
            .expect("validated CVTPS2DQ controls");
        run_with_cvtps2dq_register_controls(
            &mut state,
            memory,
            config,
            Cvtps2dqControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttps2dqRegistersV1 {
        let controls = loaded
            .fixture
            .cvttps2dq_controls
            .expect("validated CVTTPS2DQ controls");
        run_with_cvttps2dq_register_controls(
            &mut state,
            memory,
            config,
            Cvttps2dqControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtps2dqMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtps2dq_m128_controls
            .expect("validated CVTPS2DQ m128 controls");
        run_with_cvtps2dq_memory_controls(
            &mut state,
            memory,
            config,
            Cvtps2dqMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttps2dqMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvttps2dq_m128_controls
            .expect("validated CVTTPS2DQ m128 controls");
        run_with_cvttps2dq_memory_controls(
            &mut state,
            memory,
            config,
            Cvttps2dqMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttpd2dqMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvttpd2dq_m128_controls
            .expect("validated CVTTPD2DQ m128 controls");
        run_with_cvttpd2dq_memory_controls(
            &mut state,
            memory,
            config,
            Cvttpd2dqMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpd2dqMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtpd2dq_m128_controls
            .expect("validated CVTPD2DQ m128 controls");
        run_with_cvtpd2dq_memory_controls(
            &mut state,
            memory,
            config,
            Cvtpd2dqMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpd2dqRegistersV1 {
        let controls = loaded
            .fixture
            .cvtpd2dq_controls
            .expect("validated CVTPD2DQ controls");
        run_with_cvtpd2dq_register_controls(
            &mut state,
            memory,
            config,
            Cvtpd2dqControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttpd2dqRegistersV1 {
        let controls = loaded
            .fixture
            .cvttpd2dq_controls
            .expect("validated CVTTPD2DQ controls");
        run_with_cvttpd2dq_register_controls(
            &mut state,
            memory,
            config,
            Cvttpd2dqControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSqrtssRegistersV1 {
        let controls = loaded
            .fixture
            .sqrtss_controls
            .expect("validated SQRTSS controls");
        run_with_sqrtss_register_controls(
            &mut state,
            memory,
            config,
            SqrtssControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSqrtsdRegistersV1 {
        let controls = loaded
            .fixture
            .sqrtsd_controls
            .expect("validated SQRTSD controls");
        run_with_sqrtsd_register_controls(
            &mut state,
            memory,
            config,
            SqrtsdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSqrtpsRegistersV1 {
        let controls = loaded
            .fixture
            .sqrtps_controls
            .expect("validated SQRTPS controls");
        run_with_sqrtps_register_controls(
            &mut state,
            memory,
            config,
            SqrtpsControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSqrtpdRegistersV1 {
        let controls = loaded
            .fixture
            .sqrtpd_controls
            .expect("validated SQRTPD controls");
        run_with_sqrtpd_register_controls(
            &mut state,
            memory,
            config,
            SqrtpdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSubpdRegistersV1 {
        let controls = loaded
            .fixture
            .subpd_controls
            .expect("validated SUBPD controls");
        run_with_subpd_register_controls(
            &mut state,
            memory,
            config,
            SubpdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMulpdRegistersV1 {
        let controls = loaded
            .fixture
            .mulpd_controls
            .expect("validated MULPD controls");
        run_with_mulpd_register_controls(
            &mut state,
            memory,
            config,
            MulpdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongAddpdRegistersV1 {
        let controls = loaded
            .fixture
            .addpd_controls
            .expect("validated ADDPD controls");
        run_with_addpd_register_controls(
            &mut state,
            memory,
            config,
            AddpdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpd2psRegistersV1 {
        let controls = loaded
            .fixture
            .cvtpd2ps_controls
            .expect("validated CVTPD2PS controls");
        run_with_cvtpd2ps_register_controls(
            &mut state,
            memory,
            config,
            Cvtpd2psControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongAddsdRegistersV1 {
        let controls = loaded
            .fixture
            .addsd_controls
            .expect("validated ADDSD controls");
        run_with_addsd_register_controls(
            &mut state,
            memory,
            config,
            AddsdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongAddssRegistersV1 {
        let controls = loaded
            .fixture
            .addss_controls
            .expect("validated ADDSS controls");
        run_with_addss_register_controls(
            &mut state,
            memory,
            config,
            AddssControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongAddpsRegistersV1 {
        let controls = loaded
            .fixture
            .addps_controls
            .expect("validated ADDPS controls");
        run_with_addps_register_controls(
            &mut state,
            memory,
            config,
            AddpsControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongDivpdRegistersV1 {
        let controls = loaded
            .fixture
            .divpd_controls
            .expect("validated DIVPD controls");
        run_with_divpd_register_controls(
            &mut state,
            memory,
            config,
            DivpdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongDivsdRegistersV1 {
        let controls = loaded
            .fixture
            .divsd_controls
            .expect("validated DIVSD controls");
        run_with_divsd_register_controls(
            &mut state,
            memory,
            config,
            DivsdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongDivssRegistersV1 {
        let controls = loaded
            .fixture
            .divss_controls
            .expect("validated DIVSS controls");
        run_with_divss_register_controls(
            &mut state,
            memory,
            config,
            DivssControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongDivpsRegistersV1 {
        let controls = loaded
            .fixture
            .divps_controls
            .expect("validated DIVPS controls");
        run_with_divps_register_controls(
            &mut state,
            memory,
            config,
            DivpsControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMulsdRegistersV1 {
        let controls = loaded
            .fixture
            .mulsd_controls
            .expect("validated MULSD controls");
        run_with_mulsd_register_controls(
            &mut state,
            memory,
            config,
            MulsdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMulssRegistersV1 {
        let controls = loaded
            .fixture
            .mulss_controls
            .expect("validated MULSS controls");
        run_with_mulss_register_controls(
            &mut state,
            memory,
            config,
            MulssControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMulpsRegistersV1 {
        let controls = loaded
            .fixture
            .mulps_controls
            .expect("validated MULPS controls");
        run_with_mulps_register_controls(
            &mut state,
            memory,
            config,
            MulpsControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSubsdRegistersV1 {
        let controls = loaded
            .fixture
            .subsd_controls
            .expect("validated SUBSD controls");
        run_with_subsd_register_controls(
            &mut state,
            memory,
            config,
            SubsdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSubssRegistersV1 {
        let controls = loaded
            .fixture
            .subss_controls
            .expect("validated SUBSS controls");
        run_with_subss_register_controls(
            &mut state,
            memory,
            config,
            SubssControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSubpsRegistersV1 {
        let controls = loaded
            .fixture
            .subps_controls
            .expect("validated SUBPS controls");
        run_with_subps_register_controls(
            &mut state,
            memory,
            config,
            SubpsControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtps2pdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtps2pd_m64_controls
            .expect("validated CVTPS2PD m64 controls");
        run_with_cvtps2pd_memory_controls(
            &mut state,
            memory,
            config,
            Cvtps2pdMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtss2sdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtss2sd_m32_controls
            .expect("validated CVTSS2SD m32 controls");
        run_with_cvtss2sd_memory_controls(
            &mut state,
            memory,
            config,
            Cvtss2sdMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtps2pdRegistersV1 {
        let controls = loaded
            .fixture
            .cvtps2pd_controls
            .expect("validated CVTPS2PD controls");
        run_with_cvtps2pd_register_controls(
            &mut state,
            memory,
            config,
            Cvtps2pdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongComissUcomissRegistersV1 {
        let controls = loaded
            .fixture
            .comiss_ucomiss_controls
            .expect("validated COMISS/UCOMISS controls");
        run_with_comiss_ucomiss_register_controls(
            &mut state,
            memory,
            config,
            ComissUcomissControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmpssRegistersV1 {
        let controls = loaded
            .fixture
            .cmpss_controls
            .expect("validated CMPSS controls");
        run_with_cmpss_register_controls(
            &mut state,
            memory,
            config,
            CmpssControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmpssMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cmpss_memory_rax_controls
            .expect("validated CMPSS m32 controls");
        run_with_cmpss_memory_rax_controls(
            &mut state,
            memory,
            config,
            CmpssMemoryRaxControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmpsdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cmpsd_memory_rax_controls
            .expect("validated CMPSD m64 controls");
        run_with_cmpsd_memory_rax_controls(
            &mut state,
            memory,
            config,
            CmpsdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmppdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cmppd_memory_rax_controls
            .expect("validated CMPPD m128 controls");
        run_with_cmppd_memory_rax_controls(
            &mut state,
            memory,
            config,
            CmppdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinsdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .minsd_memory_rax_controls
            .expect("validated MINSD m64 controls");
        run_with_minsd_memory_rax_controls(
            &mut state,
            memory,
            config,
            MinsdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxssMemoryRaxV1 {
        let controls = loaded
            .fixture
            .maxss_memory_rax_controls
            .expect("validated MAXSS m32 controls");
        run_with_maxss_memory_rax_controls(
            &mut state,
            memory,
            config,
            MaxssMemoryRaxControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxsdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .maxsd_memory_rax_controls
            .expect("validated MAXSD m64 controls");
        run_with_maxsd_memory_rax_controls(
            &mut state,
            memory,
            config,
            MaxsdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinpdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .minpd_memory_rax_controls
            .expect("validated MINPD m128 controls");
        run_with_minpd_memory_rax_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::MinpdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMinpsMemoryRaxV1 {
        let controls = loaded
            .fixture
            .minps_memory_rax_controls
            .expect("validated MINPS m128 controls");
        run_with_minps_memory_rax_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::MinpsMemoryRaxControls {
                sse_available: controls.sse_available,
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSqrtpsMemoryRaxV1 {
        let controls = loaded
            .fixture
            .sqrtps_memory_rax_controls
            .expect("validated SQRTPS m128 controls");
        run_with_sqrtps_memory_rax_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::SqrtpsMemoryRaxControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSqrtsdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .sqrtsd_memory_rax_controls
            .expect("validated SQRTSD m64 controls");
        run_with_sqrtsd_memory_rax_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::SqrtsdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongSqrtpdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .sqrtpd_memory_rax_controls
            .expect("validated SQRTPD m128 controls");
        run_with_sqrtpd_memory_rax_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::SqrtpdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxpdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .maxpd_memory_rax_controls
            .expect("validated MAXPD m128 controls");
        run_with_maxpd_memory_rax_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::MaxpdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongMaxpsMemoryRaxV1 {
        let controls = loaded
            .fixture
            .maxps_memory_rax_controls
            .expect("validated MAXPS m128 controls");
        run_with_maxps_memory_rax_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::MaxpsMemoryRaxControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmppsMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cmpps_memory_rax_controls
            .expect("validated CMPPS m128 controls");
        run_with_cmpps_memory_rax_controls(
            &mut state,
            memory,
            config,
            CmppsMemoryRaxControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmpsdRegistersV1 {
        let controls = loaded
            .fixture
            .cmpsd_controls
            .expect("validated CMPSD controls");
        run_with_cmpsd_register_controls(
            &mut state,
            memory,
            config,
            CmpsdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmppdRegistersV1 {
        let controls = loaded
            .fixture
            .cmppd_controls
            .expect("validated CMPPD controls");
        run_with_cmppd_register_controls(
            &mut state,
            memory,
            config,
            CmppdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCmppsRegistersV1 {
        let controls = loaded
            .fixture
            .cmpps_controls
            .expect("validated CMPPS controls");
        run_with_cmpps_register_controls(
            &mut state,
            memory,
            config,
            CmppsControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongComisdUcomisdRegistersV1 {
        let controls = loaded
            .fixture
            .comisd_ucomisd_controls
            .expect("validated COMISD/UCOMISD controls");
        run_with_comisd_ucomisd_register_controls(
            &mut state,
            memory,
            config,
            ComisdUcomisdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongComisdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .comisd_memory_rax_controls
            .expect("validated COMISD m64 controls");
        run_with_comisd_memory_rax_controls(
            &mut state,
            memory,
            config,
            ComisdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongUcomisdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .ucomisd_memory_rax_controls
            .expect("validated UCOMISD m64 controls");
        run_with_ucomisd_memory_rax_controls(
            &mut state,
            memory,
            config,
            UcomisdMemoryRaxControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongComissMemoryRaxV1 {
        let controls = loaded
            .fixture
            .comiss_memory_rax_controls
            .expect("validated COMISS m32 controls");
        run_with_comiss_memory_rax_controls(
            &mut state,
            memory,
            config,
            ComissMemoryRaxControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongUcomissMemoryRaxV1 {
        let controls = loaded
            .fixture
            .ucomiss_memory_rax_controls
            .expect("validated UCOMISS m32 controls");
        run_with_ucomiss_memory_rax_controls(
            &mut state,
            memory,
            config,
            UcomissMemoryRaxControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtss2sdRegistersV1 {
        let controls = loaded
            .fixture
            .cvtss2sd_controls
            .expect("validated CVTSS2SD controls");
        run_with_cvtss2sd_register_controls(
            &mut state,
            memory,
            config,
            Cvtss2sdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsd2ssRegistersV1 {
        let controls = loaded
            .fixture
            .cvtsd2ss_controls
            .expect("validated CVTSD2SS controls");
        run_with_cvtsd2ss_register_controls(
            &mut state,
            memory,
            config,
            Cvtsd2ssControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsd2ssMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtsd2ss_m64_controls
            .expect("validated CVTSD2SS m64 controls");
        run_with_cvtsd2ss_memory_controls(
            &mut state,
            memory,
            config,
            Cvtsd2ssMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpd2psMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtpd2ps_m128_controls
            .expect("validated CVTPD2PS m128 controls");
        run_with_cvtpd2ps_memory_controls(
            &mut state,
            memory,
            config,
            Cvtpd2psMemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpi2pdMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtpi2pd_memory_controls
            .expect("validated CVTPI2PD memory controls");
        run_with_cvtpi2pd_memory_controls(
            &mut state,
            memory,
            config,
            Cvtpi2pdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpi2pdRegisterMaskedV1 {
        let controls = loaded
            .fixture
            .cvtpi2pd_register_controls
            .expect("validated CVTPI2PD register controls");
        run_with_cvtpi2pd_xmm0_mm1_controls(
            &mut state,
            memory,
            config,
            Cvtpi2pdControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpi2psMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtpi2ps_memory_controls
            .expect("validated CVTPI2PS memory controls");
        run_with_cvtpi2ps_memory_controls(
            &mut state,
            memory,
            config,
            Cvtpi2psControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpi2psRegisterMaskedV1 {
        let controls = loaded
            .fixture
            .cvtpi2ps_register_controls
            .expect("validated CVTPI2PS register controls");
        run_with_cvtpi2ps_xmm0_mm1_controls(
            &mut state,
            memory,
            config,
            Cvtpi2psControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttps2piRegisterMaskedV1 {
        let controls = loaded.fixture.cvtpi2ps_memory_controls.unwrap();
        run_with_cvttps2pi_mm0_xmm1_controls(
            &mut state,
            memory,
            config,
            Cvtpi2psControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttps2piMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvttps2pi_memory_controls
            .expect("validated CVTTPS2PI memory controls");
        run_with_cvttps2pi_mm0_memory_rax_controls(
            &mut state,
            memory,
            config,
            Cvtpi2psControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtps2piMemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtps2pi_memory_controls
            .expect("validated CVTPS2PI memory controls");
        run_with_cvtps2pi_mm0_memory_rax_controls(
            &mut state,
            memory,
            config,
            Cvtpi2psControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtps2piRegisterMaskedV1 {
        let controls = loaded.fixture.cvtps2pi_register_controls.unwrap();
        run_with_cvtps2pi_mm0_xmm1_controls(
            &mut state,
            memory,
            config,
            Cvtpi2psControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpd2piRegisterMaskedV1 {
        let controls = loaded.fixture.cvtpd2pi_register_controls.unwrap();
        run_with_cvtpd2pi_mm0_xmm1_controls(
            &mut state,
            memory,
            config,
            Cvtpd2piControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttpd2piRegisterMaskedV1 {
        let controls = loaded.fixture.cvttpd2pi_register_controls.unwrap();
        run_with_cvttpd2pi_mm0_xmm1_controls(
            &mut state,
            memory,
            config,
            Cvtpd2piControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttpd2piMemoryRaxV1 {
        let controls = loaded.fixture.cvttpd2pi_memory_controls.unwrap();
        run_with_cvttpd2pi_mm0_memory_rax_controls(
            &mut state,
            memory,
            config,
            Cvtpd2piControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtpd2piMemoryRaxV1 {
        let controls = loaded.fixture.cvtpd2pi_memory_controls.unwrap();
        run_with_cvtpd2pi_mm0_memory_rax_controls(
            &mut state,
            memory,
            config,
            Cvtpd2piControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2sdMemory32RaxV1 {
        let controls = loaded
            .fixture
            .cvtsi2sd_r32_memory_controls
            .expect("validated CVTSI2SD m32 profile has explicit controls");
        run_with_cvtsi2sd_r32_memory_controls(
            &mut state,
            memory,
            config,
            Cvtsi2sdR32Controls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2ssMemory32RaxV1 {
        let controls = loaded
            .fixture
            .cvtsi2ss_r32_memory_controls
            .expect("validated CVTSI2SS m32 profile has explicit SSE controls");
        run_with_cvtsi2ss_r32_memory_controls(
            &mut state,
            memory,
            config,
            Cvtsi2ssR32MemoryControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2ssMemory64RaxV1 {
        let controls = loaded
            .fixture
            .cvtsi2ss_r64_memory_controls
            .expect("validated CVTSI2SS m64 profile has explicit SSE controls");
        run_with_cvtsi2ss_r64_memory_controls(
            &mut state,
            memory,
            config,
            Cvtsi2ssR64MemoryControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2sdMemory64RaxV1 {
        let controls = loaded
            .fixture
            .cvtsi2sd_r64_memory_controls
            .expect("validated CVTSI2SD m64 profile has explicit controls");
        run_with_cvtsi2sd_r64_memory_controls(
            &mut state,
            memory,
            config,
            Cvtsi2sdR64MemoryControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2sdR32ExactV1 {
        let controls = loaded
            .fixture
            .cvtsi2sd_r32_controls
            .expect("validated CVTSI2SD r32 profile has explicit controls");
        run_with_cvtsi2sd_r32_controls(
            &mut state,
            memory,
            config,
            Cvtsi2sdR32Controls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2sdR64ExactV1 {
        let controls = loaded
            .fixture
            .cvtsi2sd_r64_controls
            .expect("validated CVTSI2SD r64 profile has explicit controls");
        run_with_cvtsi2sd_r64_controls(
            &mut state,
            memory,
            config,
            Cvtsi2sdR64Controls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2sdRegistersV1 {
        let controls = loaded
            .fixture
            .cvtsi2sd_r32_controls
            .expect("validated CVTSI2SD register profile has explicit controls");
        run_with_cvtsi2sd_registers_controls(
            &mut state,
            memory,
            config,
            od_isa::execute::Cvtsi2sdRegistersControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsi2ssRegistersV1 {
        let controls = loaded
            .fixture
            .cvtsi2ss_controls
            .expect("validated CVTSI2SS controls");
        run_with_cvtsi2ss_registers_controls(
            &mut state,
            memory,
            config,
            Cvtsi2ssRegistersControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtss2siRegistersV1 {
        let controls = loaded
            .fixture
            .cvtss2si_controls
            .expect("validated CVTSS2SI controls");
        run_with_cvtss2si_registers_controls(
            &mut state,
            memory,
            config,
            Cvtss2siRegistersControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttss2siRegistersV1 {
        let controls = loaded
            .fixture
            .cvtss2si_controls
            .expect("validated CVTTSS2SI controls");
        run_with_cvttss2si_registers_controls(
            &mut state,
            memory,
            config,
            Cvttss2siRegistersControls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsd2siR32MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtsd2si_r32_memory_controls
            .expect("validated CVTSD2SI m64 controls");
        run_with_cvtsd2si_r32_memory_controls(
            &mut state,
            memory,
            config,
            Cvtsd2siMemoryR32Controls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsd2siR64MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtsd2si_r64_memory_controls
            .expect("validated CVTSD2SI m64-to-r64 controls");
        run_with_cvtsd2si_r64_memory_controls(
            &mut state,
            memory,
            config,
            Cvtsd2siMemoryR64Controls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttsd2siR32MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvttsd2si_r32_memory_controls
            .expect("validated CVTTSD2SI m64 controls");
        run_with_cvttsd2si_r32_memory_controls(
            &mut state,
            memory,
            config,
            Cvttsd2siMemoryR32Controls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttsd2siR64MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvttsd2si_r64_memory_controls
            .expect("validated CVTTSD2SI m64-to-r64 controls");
        run_with_cvttsd2si_r64_memory_controls(
            &mut state,
            memory,
            config,
            Cvttsd2siMemoryR64Controls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtss2siR32MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtss2si_r32_memory_controls
            .expect("validated CVTSS2SI m32 controls");
        run_with_cvtss2si_r32_memory_controls(
            &mut state,
            memory,
            config,
            Cvtss2siMemoryR32Controls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtss2siR64MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvtss2si_r64_memory_controls
            .expect("validated CVTSS2SI m32-to-r64 controls");
        run_with_cvtss2si_r64_memory_controls(
            &mut state,
            memory,
            config,
            Cvtss2siMemoryR64Controls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttss2siR32MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvttss2si_r32_memory_controls
            .expect("validated CVTTSS2SI m32 controls");
        run_with_cvttss2si_r32_memory_controls(
            &mut state,
            memory,
            config,
            Cvttss2siMemoryR32Controls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttss2siR64MemoryRaxV1 {
        let controls = loaded
            .fixture
            .cvttss2si_r64_memory_controls
            .expect("validated CVTTSS2SI m32-to-r64 controls");
        run_with_cvttss2si_r64_memory_controls(
            &mut state,
            memory,
            config,
            Cvttss2siMemoryR64Controls {
                sse_available: controls.sse_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvtsd2siRegistersV1 {
        let controls = loaded
            .fixture
            .cvtsd2si_controls
            .expect("validated CVTSD2SI controls");
        run_with_cvtsd2si_registers_controls(
            &mut state,
            memory,
            config,
            Cvtsd2siRegistersControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else if loaded.fixture.profile == v2::Profile::LongCvttsd2siRegistersV1 {
        let controls = loaded
            .fixture
            .cvtsd2si_controls
            .expect("validated CVTTSD2SI controls");
        run_with_cvttsd2si_registers_controls(
            &mut state,
            memory,
            config,
            Cvtsd2siRegistersControls {
                sse2_available: controls.sse2_available,
                cr0_em: controls.cr0_em,
                cr0_ts: controls.cr0_ts,
                cr4_osfxsr: controls.cr4_osfxsr,
                cr4_osxmmexcpt: controls.cr4_osxmmexcpt,
            },
            timeout,
        )
    } else {
        run(&mut state, memory, config, timeout)
    };
    let exception = match &execution.exit {
        RunExit::Fault(fault) => Some(od_testkit::result::ExceptionSnapshot {
            vector: fault.vector,
            error_code: fault.error_code.map_or_else(
                || unavailable("exception has no architectural error code"),
                |code| Observation::Available {
                    value: HexU64(u64::from(code)),
                },
            ),
            faulting_rip: HexU64(state.rip),
            fault_address: fault.linear_address.map_or_else(
                || unavailable("exception has no reported linear address"),
                |address| Observation::Available {
                    value: HexU64(address),
                },
            ),
        }),
        _ => None,
    };
    let mut outcome = match execution.exit {
        RunExit::StopAddress => v2::Outcome::StopAddress {},
        RunExit::Halted => v2::Outcome::Halted {},
        RunExit::InstructionBudgetExhausted => v2::Outcome::InstructionBudgetExhausted {},
        RunExit::RepIterationBudgetExhausted => v2::Outcome::RepIterationBudgetExhausted {},
        RunExit::StopRequested if timed_out => v2::Outcome::TimedOut {
            reason: "cooperative wall-time deadline reached at an instruction boundary".into(),
        },
        RunExit::StopRequested => v2::Outcome::StopRequested {},
        RunExit::Fault(_) => v2::Outcome::Exception {},
        RunExit::Unsupported { opcode } => v2::Outcome::Unsupported {
            reason: format!(
                "interpreter does not support opcode 0x{opcode:02x} at RIP 0x{:016x}",
                state.rip
            ),
        },
        RunExit::MemoryError(error) => v2::Outcome::HarnessError {
            reason: format!("fixture memory backend: {error:?}"),
        },
    };
    let snapshot = match v2::IntegerState::from_cpu_for(&state, loaded.fixture.profile) {
        Ok(state) => Observation::Available {
            value: v2::Snapshot {
                state,
                // The interpreter tracks architectural flag definedness as
                // instructions retire, including undefined outputs such as AF
                // after logical operations.
                rflags_defined: HexU64(
                    if matches!(
                        loaded.fixture.profile,
                        v2::Profile::LongCvttpd2piMemoryRaxV1
                            | v2::Profile::LongCvtpd2piMemoryRaxV1
                            | v2::Profile::LongCvttps2dqMemoryRaxV1
                            | v2::Profile::LongCvtps2dqMemoryRaxV1
                            | v2::Profile::LongCvttpd2dqMemoryRaxV1
                            | v2::Profile::LongCvtpd2dqMemoryRaxV1
                            | v2::Profile::LongCvtps2pdMemoryRaxV1
                            | v2::Profile::LongCvtss2sdMemoryRaxV1
                            | v2::Profile::LongCvtpd2psMemoryRaxV1
                            | v2::Profile::LongCvtsd2ssMemoryRaxV1
                            | v2::Profile::LongCvtsi2sdMemory64RaxV1
                            | v2::Profile::LongCvtsi2ssMemory64RaxV1
                    ) {
                        v2::INTEGER_FLAGS | (1 << 18)
                    } else {
                        execution.rflags_defined
                    },
                ),
                memory: memory.snapshot(),
                xorpd_prerequisites: loaded.fixture.xorpd_prerequisites,
                minss_invalid_controls: loaded.fixture.minss_invalid_controls,
                maxss_unmasked_controls: loaded.fixture.maxss_unmasked_controls,
                minsd_unmasked_controls: loaded.fixture.minsd_unmasked_controls,
                maxsd_unmasked_controls: loaded.fixture.maxsd_unmasked_controls,
                minpd_unmasked_controls: loaded.fixture.minpd_unmasked_controls,
                minps_unmasked_controls: loaded.fixture.minps_unmasked_controls,
                maxps_unmasked_controls: loaded.fixture.maxps_unmasked_controls,
                maxpd_unmasked_controls: loaded.fixture.maxpd_unmasked_controls,
                cvtdq2pd_controls: loaded.fixture.cvtdq2pd_controls,
                cvtdq2pd_memory_controls: loaded.fixture.cvtdq2pd_memory_controls,
                cvtdq2ps_controls: loaded.fixture.cvtdq2ps_controls,
                cvtdq2ps_memory_controls: loaded.fixture.cvtdq2ps_memory_controls,
                cvtps2dq_controls: loaded.fixture.cvtps2dq_controls,
                cvttps2dq_controls: loaded.fixture.cvttps2dq_controls,
                cvttps2dq_m128_controls: loaded.fixture.cvttps2dq_m128_controls,
                cvtps2dq_m128_controls: loaded.fixture.cvtps2dq_m128_controls,
                cvtpd2dq_controls: loaded.fixture.cvtpd2dq_controls,
                cvttpd2dq_controls: loaded.fixture.cvttpd2dq_controls,
                cvttpd2dq_m128_controls: loaded.fixture.cvttpd2dq_m128_controls,
                cvtpd2dq_m128_controls: loaded.fixture.cvtpd2dq_m128_controls,
                sqrtss_controls: loaded.fixture.sqrtss_controls,
                sqrtsd_controls: loaded.fixture.sqrtsd_controls,
                sqrtps_controls: loaded.fixture.sqrtps_controls,
                sqrtpd_controls: loaded.fixture.sqrtpd_controls,
                subpd_controls: loaded.fixture.subpd_controls,
                mulpd_controls: loaded.fixture.mulpd_controls,
                addpd_controls: loaded.fixture.addpd_controls,
                cvtpd2ps_controls: loaded.fixture.cvtpd2ps_controls,
                addsd_controls: loaded.fixture.addsd_controls,
                addss_controls: loaded.fixture.addss_controls,
                addps_controls: loaded.fixture.addps_controls,
                divpd_controls: loaded.fixture.divpd_controls,
                divsd_controls: loaded.fixture.divsd_controls,
                divps_controls: loaded.fixture.divps_controls,
                divss_controls: loaded.fixture.divss_controls,
                mulsd_controls: loaded.fixture.mulsd_controls,
                mulss_controls: loaded.fixture.mulss_controls,
                mulps_controls: loaded.fixture.mulps_controls,
                subps_controls: loaded.fixture.subps_controls,
                subsd_controls: loaded.fixture.subsd_controls,
                subss_controls: loaded.fixture.subss_controls,
                cvtss2sd_controls: loaded.fixture.cvtss2sd_controls,
                cvtsd2ss_controls: loaded.fixture.cvtsd2ss_controls,
                cvtpi2pd_memory_controls: loaded.fixture.cvtpi2pd_memory_controls,
                cvtpi2pd_register_controls: loaded.fixture.cvtpi2pd_register_controls,
                cvtpi2ps_memory_controls: loaded.fixture.cvtpi2ps_memory_controls,
                cvtpi2ps_register_controls: loaded.fixture.cvtpi2ps_register_controls,
                cvttps2pi_memory_controls: loaded.fixture.cvttps2pi_memory_controls,
                cvtps2pi_memory_controls: loaded.fixture.cvtps2pi_memory_controls,
                cvtps2pi_register_controls: loaded.fixture.cvtps2pi_register_controls,
                cvtpd2pi_register_controls: loaded.fixture.cvtpd2pi_register_controls,
                cvttpd2pi_register_controls: loaded.fixture.cvttpd2pi_register_controls,
                cvttpd2pi_memory_controls: loaded.fixture.cvttpd2pi_memory_controls,
                cvtpd2pi_memory_controls: loaded.fixture.cvtpd2pi_memory_controls,
                cvtps2pd_controls: loaded.fixture.cvtps2pd_controls,
                cvtps2pd_m64_controls: loaded.fixture.cvtps2pd_m64_controls,
                cvtss2sd_m32_controls: loaded.fixture.cvtss2sd_m32_controls,
                cvtsd2ss_m64_controls: loaded.fixture.cvtsd2ss_m64_controls,
                cvtpd2ps_m128_controls: loaded.fixture.cvtpd2ps_m128_controls,
                comiss_ucomiss_controls: loaded.fixture.comiss_ucomiss_controls,
                cmpss_controls: loaded.fixture.cmpss_controls,
                cmpsd_controls: loaded.fixture.cmpsd_controls,
                cmppd_controls: loaded.fixture.cmppd_controls,
                cmpps_controls: loaded.fixture.cmpps_controls,
                comisd_ucomisd_controls: loaded.fixture.comisd_ucomisd_controls,
                comisd_memory_rax_controls: loaded.fixture.comisd_memory_rax_controls,
                ucomisd_memory_rax_controls: loaded.fixture.ucomisd_memory_rax_controls,
                comiss_memory_rax_controls: loaded.fixture.comiss_memory_rax_controls,
                ucomiss_memory_rax_controls: loaded.fixture.ucomiss_memory_rax_controls,
                cmpss_memory_rax_controls: loaded.fixture.cmpss_memory_rax_controls,
                cmpsd_memory_rax_controls: loaded.fixture.cmpsd_memory_rax_controls,
                cmppd_memory_rax_controls: loaded.fixture.cmppd_memory_rax_controls,
                cmpps_memory_rax_controls: loaded.fixture.cmpps_memory_rax_controls,
                minsd_memory_rax_controls: loaded.fixture.minsd_memory_rax_controls,
                maxss_memory_rax_controls: loaded.fixture.maxss_memory_rax_controls,
                maxsd_memory_rax_controls: loaded.fixture.maxsd_memory_rax_controls,
                minpd_memory_rax_controls: loaded.fixture.minpd_memory_rax_controls,
                minps_memory_rax_controls: loaded.fixture.minps_memory_rax_controls,
                sqrtps_memory_rax_controls: loaded.fixture.sqrtps_memory_rax_controls,
                sqrtpd_memory_rax_controls: loaded.fixture.sqrtpd_memory_rax_controls,
                sqrtsd_memory_rax_controls: loaded.fixture.sqrtsd_memory_rax_controls,
                maxpd_memory_rax_controls: loaded.fixture.maxpd_memory_rax_controls,
                maxps_memory_rax_controls: loaded.fixture.maxps_memory_rax_controls,
                cvtsi2sd_r32_controls: loaded.fixture.cvtsi2sd_r32_controls,
                cvtsi2sd_r32_memory_controls: loaded.fixture.cvtsi2sd_r32_memory_controls,
                cvtsi2ss_r32_memory_controls: loaded.fixture.cvtsi2ss_r32_memory_controls,
                cvtsi2ss_r64_memory_controls: loaded.fixture.cvtsi2ss_r64_memory_controls,
                cvtsi2sd_r64_controls: loaded.fixture.cvtsi2sd_r64_controls,
                cvtsi2sd_r64_memory_controls: loaded.fixture.cvtsi2sd_r64_memory_controls,
                cvtsi2ss_controls: loaded.fixture.cvtsi2ss_controls,
                cvtss2si_controls: loaded.fixture.cvtss2si_controls,
                cvtsd2si_controls: loaded.fixture.cvtsd2si_controls,
                cvtsd2si_r32_memory_controls: loaded.fixture.cvtsd2si_r32_memory_controls,
                cvtsd2si_r64_memory_controls: loaded.fixture.cvtsd2si_r64_memory_controls,
                cvttsd2si_r32_memory_controls: loaded.fixture.cvttsd2si_r32_memory_controls,
                cvttsd2si_r64_memory_controls: loaded.fixture.cvttsd2si_r64_memory_controls,
                cvtss2si_r32_memory_controls: loaded.fixture.cvtss2si_r32_memory_controls,
                cvtss2si_r64_memory_controls: loaded.fixture.cvtss2si_r64_memory_controls,
                cvttss2si_r32_memory_controls: loaded.fixture.cvttss2si_r32_memory_controls,
                cvttss2si_r64_memory_controls: loaded.fixture.cvttss2si_r64_memory_controls,
                fxsave64_cr0_controls: loaded.fixture.fxsave64_cr0_controls,
                fxrstor64_cr0_controls: loaded.fixture.fxrstor64_cr0_controls,
                rdtsc_source_calls,
                exception: Observation::Available { value: exception },
            },
        },
        Err(error) => {
            outcome = v2::Outcome::HarnessError {
                reason: format!("cannot export CPU state to fixture snapshot: {error}"),
            };
            unavailable("CPU state contains data outside the fixture snapshot schema")
        }
    };
    v2::RunResult {
        schema_version: v2::SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend: Backend::Interpreter,
        outcome,
        instructions_executed: Observation::Available {
            value: HexU64(execution.instructions_executed),
        },
        snapshot,
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

pub fn compare_results(args: &[String]) -> ExitCode {
    let Some(left_path) = args.first() else {
        return invalid_input(
            "usage: od-runner --compare-results RESULT_A.json RESULT_B.json --case CASE.json --json",
        );
    };
    let Some(right_path) = args.get(1) else {
        return invalid_input("--compare-results requires two result paths");
    };
    let mut case_path = None;
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--case" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return invalid_input("missing value for --case");
                };
                case_path = Some(value.as_str());
            }
            "--json" => json = true,
            value => return invalid_input(&format!("unexpected compare argument '{value}'")),
        }
        index += 1;
    }
    if !json {
        return invalid_input("--compare-results requires --json");
    }
    let Some(case_path) = case_path else {
        return invalid_input("--compare-results requires --case CASE.json");
    };

    let case_bytes = match fs::read(case_path) {
        Ok(bytes) => bytes,
        Err(error) => return invalid_input(&format!("could not read case: {error}")),
    };
    match fixture_schema_version(&case_bytes) {
        Ok(2) => return compare_results_v2(&case_bytes, left_path, right_path),
        Ok(1) => {}
        Ok(version) => return invalid_input(&format!("unsupported fixture schema {version}")),
        Err(error) => return invalid_input(&error),
    }
    let fixture = match Fixture::from_json(&case_bytes) {
        Ok(fixture) => fixture,
        Err(error) => return invalid_input(&format!("invalid case: {error}")),
    };
    let left = match read_result(left_path) {
        Ok(result) => result,
        Err(error) => return invalid_input(&format!("invalid left result: {error}")),
    };
    let right = match read_result(right_path) {
        Ok(result) => result,
        Err(error) => return invalid_input(&format!("invalid right result: {error}")),
    };
    let report = match compare_result_values(&fixture, &left, &right) {
        Ok(report) => report,
        Err(CompareError::Incomplete {
            status: CompletionStatus::Unsupported,
            ..
        }) => {
            eprintln!("comparison unsupported: one result has unsupported status");
            return ExitCode::from(EXIT_UNSUPPORTED);
        }
        Err(CompareError::Incomplete { status, .. }) => {
            eprintln!("harness failure: result cannot be compared because it ended as {status:?}");
            return ExitCode::from(EXIT_HARNESS_FAILURE);
        }
        Err(error) => return invalid_input(&format!("results are not comparable: {error:?}")),
    };
    let mismatch = !report.matches();
    emit_report(report, if mismatch { EXIT_MISMATCH } else { 0 })
}

fn read_result(path: &str) -> Result<RunResult, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    RunResult::from_json(&bytes).map_err(|error| error.to_string())
}

fn compare_results_v2(case: &[u8], left_path: &str, right_path: &str) -> ExitCode {
    use od_testkit::v2;
    let loaded = match v2::Fixture::from_json(case) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error.to_string()),
    };
    let read = |path| -> Result<v2::RunResult, String> {
        let raw = fs::read(path).map_err(|e| e.to_string())?;
        v2::RunResult::from_json(&raw).map_err(|e| e.to_string())
    };
    let (left, right) = match (read(left_path), read(right_path)) {
        (Ok(left), Ok(right)) => (left, right),
        (Err(error), _) | (_, Err(error)) => return invalid_input(&error),
    };
    for result in [&left, &right] {
        if let Err(error) = result.validate_for(&loaded) {
            return invalid_input(&error.to_string());
        }
    }
    for result in [&left, &right] {
        match result.outcome {
            v2::Outcome::Unsupported { .. } => return ExitCode::from(EXIT_UNSUPPORTED),
            v2::Outcome::TimedOut { .. }
            | v2::Outcome::Signaled { .. }
            | v2::Outcome::HarnessError { .. } => return ExitCode::from(EXIT_HARNESS_FAILURE),
            _ => {}
        }
    }
    let differences = match v2::compare(&loaded, &left, &right) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error.to_string()),
    };
    println!(
        "{}",
        serde_json::json!({"schema_version":2,"case_id":loaded.fixture.case_id,"input_sha256":loaded.input_sha256,"matches":differences.is_empty(),"differences":differences})
    );
    ExitCode::from(if differences.is_empty() {
        0
    } else {
        EXIT_MISMATCH
    })
}

enum ExecutionFailure {
    InvalidInput(String),
    Unsupported(String),
    Harness(String),
}

fn interpreter_failure(error: od_isa::DecodeErrorOrExecute) -> ExecutionFailure {
    let reason = format!("interpreter: {error:?}");
    if matches!(
        error,
        od_isa::DecodeErrorOrExecute::Execute(od_isa::ExecuteError::UnexpectedCoreExit { .. })
    ) {
        ExecutionFailure::Harness(reason)
    } else {
        ExecutionFailure::Unsupported(reason)
    }
}

fn execute_fixture(
    loaded: &LoadedFixture,
    backend: Backend,
) -> Result<RunResult, ExecutionFailure> {
    let fixture = &loaded.fixture;
    let code = fixture
        .program
        .bytes()
        .map_err(|error| ExecutionFailure::InvalidInput(error.to_string()))?;
    let mut state = initial_state(fixture).map_err(ExecutionFailure::InvalidInput)?;
    let execution = match backend {
        Backend::Interpreter => {
            interpret(&code, &mut state, fixture.instruction_budget).map_err(interpreter_failure)?
        }
        Backend::Jit => {
            od_isa::decode(&code).map_err(|error| {
                ExecutionFailure::Unsupported(format!("JIT cannot decode fixture: {error:?}"))
            })?;
            execute_jit(&code, &mut state, fixture.instruction_budget)?
        }
        Backend::NativeX86 => {
            return Err(ExecutionFailure::Unsupported(
                "native x86 execution is implemented in a later stage".to_string(),
            ));
        }
    };
    Ok(completed_result(loaded, backend, execution))
}

fn initial_state(fixture: &od_testkit::case::Fixture) -> Result<CpuState, String> {
    let initial = &fixture.initial_state;
    let rip = usize::try_from(initial.rip.get())
        .map_err(|_| "fixture RIP does not fit host usize".to_string())?;
    Ok(CpuState {
        gpr: [
            initial.rax.get(),
            initial.rcx.get(),
            initial.rdx.get(),
            initial.rbx.get(),
        ],
        rip,
        rflags: initial.rflags.get(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn execute_jit(
    code: &[u8],
    state: &mut CpuState,
    budget: u64,
) -> Result<ExecutionResult, ExecutionFailure> {
    super::execute_legacy_jit(code, state, budget).map_err(ExecutionFailure::Harness)
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn execute_jit(
    _code: &[u8],
    _state: &mut CpuState,
    _budget: u64,
) -> Result<ExecutionResult, ExecutionFailure> {
    Err(ExecutionFailure::Unsupported(
        "ARM64 JIT execution requires an aarch64 macOS host".to_string(),
    ))
}

fn completed_result(
    loaded: &LoadedFixture,
    backend: Backend,
    execution: ExecutionResult,
) -> RunResult {
    let state = execution.state;
    RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend,
        status: CompletionStatus::Completed,
        expected_exit: loaded.fixture.expected_exit,
        exit_reason: Observation::Available {
            value: match execution.reason {
                od_isa::ExitReason::ReachedCodeEnd => ExitReason::ReachedCodeEnd,
                od_isa::ExitReason::InstructionBudgetExhausted => {
                    ExitReason::InstructionBudgetExhausted
                }
                od_isa::ExitReason::StopRequested => ExitReason::StopRequested,
            },
        },
        instructions_executed: Observation::Available {
            value: HexU64(execution.instructions_executed),
        },
        snapshot: CpuSnapshot {
            gpr: Observation::Available {
                value: GprSnapshot {
                    rax: HexU64(state.gpr[0]),
                    rcx: HexU64(state.gpr[1]),
                    rdx: HexU64(state.gpr[2]),
                    rbx: HexU64(state.gpr[3]),
                },
            },
            rip: Observation::Available {
                value: HexU64(state.rip as u64),
            },
            rflags: Observation::Available {
                value: HexU64(state.rflags),
            },
            memory: unavailable("S0-09 register-only profile does not capture memory"),
            exception: unavailable("S0-09 profile does not capture exceptions"),
        },
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

fn unsupported_result(loaded: &LoadedFixture, backend: Backend, reason: String) -> RunResult {
    unavailable_result(loaded, backend, CompletionStatus::Unsupported, reason)
}

fn invalid_fixture_result(loaded: &LoadedFixture, backend: Backend, reason: String) -> RunResult {
    unavailable_result(loaded, backend, CompletionStatus::InvalidInput, reason)
}

fn unavailable_result(
    loaded: &LoadedFixture,
    backend: Backend,
    status: CompletionStatus,
    reason: String,
) -> RunResult {
    RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend,
        status,
        expected_exit: loaded.fixture.expected_exit,
        exit_reason: unavailable(&reason),
        instructions_executed: unavailable(&reason),
        snapshot: CpuSnapshot {
            gpr: unavailable(&reason),
            rip: unavailable(&reason),
            rflags: unavailable(&reason),
            memory: unavailable(&reason),
            exception: unavailable(&reason),
        },
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

fn unavailable<T>(reason: &str) -> Observation<T> {
    Observation::Unavailable {
        reason: reason.to_string(),
    }
}

fn build_metadata() -> BuildMetadata {
    BuildMetadata {
        revision: option_env!("OPEN_DESKTOP_GIT_REVISION")
            .unwrap_or("unavailable")
            .to_string(),
        target: build_target(),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .to_string(),
        compiler: option_env!("RUSTC_VERSION")
            .unwrap_or("rustc version unavailable at build time")
            .to_string(),
    }
}

fn build_target() -> String {
    let arch = std::env::consts::ARCH;
    match (arch, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin".to_string(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_string(),
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu".to_string(),
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu".to_string(),
        _ => format!("{arch}-unknown-{}", std::env::consts::OS),
    }
}

fn machine_metadata() -> MachineMetadata {
    MachineMetadata {
        host_os: std::env::consts::OS.to_string(),
        host_arch: std::env::consts::ARCH.to_string(),
        cpu_model: None,
        kernel: None,
        page_size: None,
    }
}

fn emit_result(result: RunResult, code: u8) -> ExitCode {
    match result.to_json() {
        Ok(json) => match write_json(&json) {
            Ok(()) => ExitCode::from(code),
            Err(error) => {
                eprintln!("harness failure: could not write JSON result: {error}");
                ExitCode::from(EXIT_HARNESS_FAILURE)
            }
        },
        Err(error) => {
            eprintln!("harness failure: could not serialize result: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

fn emit_report(report: ComparisonReport, code: u8) -> ExitCode {
    match serde_json::to_vec(&report) {
        Ok(json) => match write_json(&json) {
            Ok(()) => ExitCode::from(code),
            Err(error) => {
                eprintln!("harness failure: could not write JSON report: {error}");
                ExitCode::from(EXIT_HARNESS_FAILURE)
            }
        },
        Err(error) => {
            eprintln!("harness failure: could not serialize comparison: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

fn write_json(json: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(json)?;
    stdout.write_all(b"\n")
}

fn invalid_input(message: &str) -> ExitCode {
    eprintln!("invalid input: {message}");
    ExitCode::from(EXIT_INVALID_INPUT)
}

#[cfg(test)]
mod native_xorpd_corpus_tests {
    use super::*;
    use od_testkit::v2;
    use od_testkit::v2::{Fixture, Outcome};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn loaded_case() -> od_testkit::v2::LoadedFixture {
        let fixture = generate_native_xorpd_fixture(NATIVE_XORPD_FIXED_SEEDS[0], 0);
        Fixture::from_json(&fixture.to_json().unwrap()).unwrap()
    }

    #[test]
    fn mandatory_unsupported_interpreter_result_fails_verification() {
        let loaded = loaded_case();
        let mut result = run_native_xorpd_interpreter(&loaded).unwrap();
        result.outcome = Outcome::Unsupported {
            reason: "synthetic unsupported outcome for verifier rejection test".into(),
        };
        assert!(
            validate_native_xorpd_result(&loaded, &result)
                .unwrap_err()
                .contains("mandatory interpreter execution did not complete")
        );
    }

    #[test]
    fn wrong_mandatory_profile_fails_verification() {
        let loaded = loaded_case();
        let mut result = run_native_xorpd_interpreter(&loaded).unwrap();
        result.profile = od_testkit::v2::Profile::LongXorpdRegV1;
        assert!(
            validate_native_xorpd_result(&loaded, &result)
                .unwrap_err()
                .contains("mandatory interpreter execution did not complete")
        );
    }

    #[test]
    fn result_mismatch_fails_independent_oracle_comparison() {
        let loaded = loaded_case();
        let mut result = run_native_xorpd_interpreter(&loaded).unwrap();
        let Observation::Available { value: snapshot } = &mut result.snapshot else {
            panic!("completed interpreter result must include a snapshot");
        };
        snapshot.state.sse.as_mut().unwrap().xmm[0] = "00".repeat(16);
        assert!(
            validate_native_xorpd_result(&loaded, &result)
                .unwrap_err()
                .contains("independent XOR oracle")
        );
    }

    #[test]
    fn missing_case_result_digest_corruption_and_mismatch_fail_closed() {
        let loaded = loaded_case();
        let mut result = run_native_xorpd_interpreter(&loaded).unwrap();
        result.snapshot = Observation::Unavailable {
            reason: "synthetic missing result snapshot".into(),
        };
        assert!(
            validate_native_xorpd_result(&loaded, &result)
                .unwrap_err()
                .contains("interpreter result is missing")
        );
        assert!(
            validate_native_xorpd_file_count(1023, 1024)
                .unwrap_err()
                .contains("expected 1024 case files")
        );
        let bytes = loaded.fixture.to_json().unwrap();
        let digest = sha256_hex(&bytes);
        assert!(validate_native_xorpd_case_digest(&bytes, &digest, "case").is_ok());
        assert!(
            validate_native_xorpd_case_digest(b"corrupt", &digest, "case")
                .unwrap_err()
                .contains("digest mismatch")
        );
    }

    #[test]
    fn per_fixture_reducer_writes_verified_no_mismatch_report_without_touching_corpus() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let loaded = loaded_case();
        let original_bytes = loaded.fixture.to_json().unwrap();
        let reordered_value: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
        let reordered_bytes = serde_json::to_vec(&reordered_value).unwrap();
        assert_ne!(reordered_bytes, original_bytes);
        let whitespace_bytes = [b" \n".as_slice(), &original_bytes, b" \n".as_slice()].concat();
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        for (suffix, input_bytes) in [
            ("canonical", original_bytes.as_slice()),
            ("whitespace", whitespace_bytes.as_slice()),
            ("reordered", reordered_bytes.as_slice()),
        ] {
            let input = std::env::temp_dir().join(format!(
                "s2-42b-input-{}-{unique}-{suffix}.json",
                std::process::id()
            ));
            let output = format!(
                "{NATIVE_XORPD_REDUCE_WORK_DIR}/test-{}-{unique}-{suffix}",
                std::process::id()
            );
            fs::write(&input, input_bytes).unwrap();
            let args = vec![
                input.to_string_lossy().into_owned(),
                "--limit".into(),
                "4".into(),
                "--output-dir".into(),
                output.clone(),
            ];
            reduce_native_xorpd_inner(&args).unwrap();
            let output_path = workspace_root().join(&output);
            assert_eq!(
                fs::read(output_path.join("original.case.json")).unwrap(),
                input_bytes
            );
            for index in 1..=2 {
                let result = v2::RunResult::from_json(
                    &fs::read(output_path.join(format!("initial-{index}.result.json"))).unwrap(),
                )
                .unwrap();
                assert_eq!(result.input_sha256, sha256_hex(input_bytes));
            }
            let expected = v2::RunResult::from_json(
                &fs::read(output_path.join("expected-original.result.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(expected.input_sha256, sha256_hex(input_bytes));
            assert!(!output_path.join("reduced.case.json").exists());
            let report: serde_json::Value =
                serde_json::from_slice(&fs::read(output_path.join("report.json")).unwrap())
                    .unwrap();
            assert_eq!(report["stop_reason"], "no_initial_mismatch");
            assert_eq!(report["native"], "not_run");
            assert_eq!(report["corpus_modified"], false);
            assert_eq!(report["output_files"]["reduced_fixture"], false);
            assert_eq!(
                report["result_sha256"]["initial"][0],
                sha256_hex(&fs::read(output_path.join("initial-1.result.json")).unwrap())
            );
            assert_eq!(
                report["result_sha256"]["expected_original"],
                sha256_hex(&fs::read(output_path.join("expected-original.result.json")).unwrap())
            );
            assert!(output_path.starts_with(workspace_root().join(NATIVE_XORPD_REDUCE_WORK_DIR)));
            assert!(!output_path.starts_with(workspace_root().join(NATIVE_XORPD_WORK_DIR)));
            fs::remove_dir_all(output_path).unwrap();
            fs::remove_file(input).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_testkit::v2;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Files(PathBuf);

    impl Files {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "od-runner-cli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_string()
        }
    }

    impl Drop for Files {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn v2_repne_entry_probe_is_overflow_safe_at_stop_and_budget_boundaries() {
        let mut loaded = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_19q_scasb_zero_arbitrary_flags.case.json"
        ))
        .unwrap();
        loaded.fixture.memory[0].guest_address = HexU64(0);
        loaded.fixture.memory[0].permissions.execute = false;
        loaded.fixture.initial_state.rip = HexU64(u64::MAX);
        loaded.fixture.stop_rip = Some(HexU64(u64::MAX));
        loaded.fixture.instruction_budget = 0;
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let stop = execute_fixture_v2(&loaded, &mut memory);
        assert!(matches!(stop.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            stop.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );

        loaded.fixture.stop_rip = None;
        loaded.fixture.expected_exit = v2::ExpectedExit::InstructionBudgetExhausted;
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let budget = execute_fixture_v2(&loaded, &mut memory);
        assert!(matches!(
            budget.outcome,
            v2::Outcome::InstructionBudgetExhausted {}
        ));
        assert_eq!(
            budget.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
    }

    #[test]
    fn frndint_signed_zero_literal_fnstsw_fixture_preserves_state_and_sign() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31dc_frndint_negative_zero_fnstsw.case.json"
        ))
        .expect("S2-31dc signed-zero fixture schema");
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31dc signed-zero composition must be observable");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_0004));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x6d20);
        let observed = value.state.x87.unwrap();
        let before = initial.x87.unwrap();
        let top = ((before.status_word.0 >> 11) & 7) as usize;
        assert_eq!(top, 5);
        assert_eq!(observed.registers[top], "00000000000000000080");
        assert_eq!(observed.tag_word, before.tag_word);
        assert_eq!(observed.status_word, HexU64(0x6d20));
        assert_eq!(observed.control_word, before.control_word);
        assert_eq!(observed.instruction_pointer, initial.rip);
        assert_eq!(observed.data_pointer, before.data_pointer);
        assert_eq!(observed.last_opcode, before.last_opcode);
        for physical in 0..8 {
            if physical != top {
                assert_eq!(observed.registers[physical], before.registers[physical]);
            }
        }
    }

    #[test]
    fn fld1_fldz_schema_fixtures_execute_and_jit_stays_denied() {
        for bytes in [
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bn_fld1.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bn_fldz.case.json").as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bn_fld1_fnstsw.case.json")
                .as_slice(),
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_30bn_fldz_fnstsw.case.json")
                .as_slice(),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-30bn fixture schema");
            let code = &fixture.fixture.memory[0].initial_bytes_hex;
            let code: Vec<u8> = code
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            assert!(od_jit::compile(&code).is_err());
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "case={} outcome={:?}",
                fixture.fixture.case_id,
                result.outcome
            );
            let instruction_count = (code.len() / 2) as u64;
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(instruction_count)
                },
                "case={}",
                fixture.fixture.case_id
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("FLD1/FLDZ fixture must provide a complete snapshot");
            };
            let initial = &fixture.fixture.initial_state;
            let before_x87 = initial.x87.as_ref().unwrap();
            let after_x87 = value.state.x87.as_ref().unwrap();
            let is_fld1 = code[1] == 0xe8;
            let result_bytes = if is_fld1 {
                "0000000000000080ff3f"
            } else {
                "00000000000000000000"
            };
            let result_tag = if is_fld1 { 0 } else { 1 };
            assert_eq!(
                value.state.rip,
                HexU64(initial.rip.get() + code.len() as u64)
            );
            assert_eq!(after_x87.status_word, HexU64(0x3800));
            assert_eq!(after_x87.registers[7], result_bytes);
            assert_eq!((after_x87.tag_word.get() >> 14) & 3, result_tag);
            assert_eq!(after_x87.instruction_pointer, initial.rip);
            assert_eq!(after_x87.control_word, before_x87.control_word);
            assert_eq!(after_x87.data_pointer, before_x87.data_pointer);
            assert_eq!(after_x87.last_opcode, before_x87.last_opcode);
            for physical in 0..7 {
                assert_eq!(
                    after_x87.registers[physical],
                    before_x87.registers[physical]
                );
                let shift = physical * 2;
                assert_eq!(
                    (after_x87.tag_word.get() >> shift) & 3,
                    (before_x87.tag_word.get() >> shift) & 3
                );
            }
            assert_eq!(
                value.state.gpr[0].get() & 0xffff,
                if code.len() == 4 { 0x3800 } else { 0 }
            );
        }
    }

    #[test]
    fn fld1_occupied_destination_fixture_rejects_without_retirement_or_mutation() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30bn_fld1_occupied_destination_rejects.case.json"
        ))
        .expect("S2-30bn occupied-destination rejection fixture schema");
        assert!(od_jit::compile(&[0xd9, 0xe8]).is_err());
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("rejected FLD1 must preserve a complete state snapshot");
        };
        assert_eq!(value.state.rip, fixture.fixture.initial_state.rip);
        assert_eq!(value.state.x87, fixture.fixture.initial_state.x87);
    }

    #[test]
    fn frndint_small_normal_literal_fnstsw_fixture_rounds_to_negative_zero() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31di_frndint_min_normal_to_negative_zero_fnstsw.case.json"
        ))
        .expect("S2-31di minNormal fixture schema");
        assert!(od_jit::compile(&[0xd9, 0xfc]).is_err());
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31di composition must expose the complete state");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_0004));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x6d21);
        let observed = value.state.x87.unwrap();
        let before = initial.x87.unwrap();
        let top = ((before.status_word.0 >> 11) & 7) as usize;
        assert_eq!(top, 5);
        assert_eq!(observed.registers[top], "00000000000000000080");
        assert_eq!(observed.tag_word.0 & (3 << (top * 2)), 1 << (top * 2));
        assert_eq!(observed.status_word, HexU64(0x6d21));
        assert_eq!(observed.control_word, before.control_word);
        assert_eq!(observed.instruction_pointer, initial.rip);
        assert_eq!(observed.data_pointer, before.data_pointer);
        assert_eq!(observed.last_opcode, before.last_opcode);
        for physical in 0..8 {
            if physical != top {
                assert_eq!(observed.registers[physical], before.registers[physical]);
            }
        }
    }

    #[test]
    fn frndint_seeded_subnormal_literal_fnstsw_fixture_records_project_state() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31do_frndint_seeded_subnormal_fnstsw.case.json"
        ))
        .expect("S2-31do seeded subnormal fixture schema");
        assert!(od_jit::compile(&[0xd9, 0xfc]).is_err());
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31do seeded fixture must expose the complete state");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_0004));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x6d23);
        let observed = value.state.x87.unwrap();
        let before = initial.x87.unwrap();
        let top = ((before.status_word.0 >> 11) & 7) as usize;
        assert_eq!(top, 5);
        assert_eq!(observed.registers[top], "00000000000000000000");
        assert_eq!(observed.tag_word.0 & (3 << (top * 2)), 1 << (top * 2));
        assert_eq!(observed.status_word, HexU64(0x6d23));
        assert_eq!(observed.control_word, before.control_word);
        assert_eq!(observed.instruction_pointer, initial.rip);
        assert_eq!(observed.data_pointer, before.data_pointer);
        assert_eq!(observed.last_opcode, before.last_opcode);
        for physical in 0..8 {
            if physical != top {
                assert_eq!(observed.registers[physical], before.registers[physical]);
            }
        }
    }

    #[test]
    fn frndint_fld_pseudo_denormal_literal_sets_de_only_during_arithmetic() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31dp_frndint_fld_pseudo_denormal_fnstsw.case.json"
        ))
        .expect("S2-31dp pseudo-denormal composition fixture schema");
        assert!(od_jit::compile(&[0xd9, 0xfc]).is_err());
        let initial = fixture.fixture.initial_state.clone();
        let initial_x87 = initial.x87.as_ref().unwrap();
        assert_eq!(initial_x87.status_word.0 & 0x0002, 0);
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(3) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31dp fixture must expose the complete state");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_0006));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x3822);
        let observed = value.state.x87.unwrap();
        let top = 7;
        assert_eq!(observed.registers[top], "00000000000000000000");
        assert_eq!(observed.tag_word.0 & (3 << (top * 2)), 1 << (top * 2));
        assert_eq!(observed.status_word, HexU64(0x3822));
        assert_eq!(observed.instruction_pointer, HexU64(0x0010_0002));
        assert_eq!(observed.data_pointer, HexU64(0x0040_0000));
        assert_eq!(observed.control_word, initial_x87.control_word);
    }

    #[test]
    fn frndint_large_exact_integer_literal_fnstsw_fixture_preserves_raw_value() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31dd_frndint_large_exact_integer_fnstsw.case.json"
        ))
        .expect("S2-31dd exact-integer fixture schema");
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31dd fixture must expose the complete state");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_0004));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x6d20);
        let observed = value.state.x87.unwrap();
        let before = initial.x87.unwrap();
        let top = ((before.status_word.0 >> 11) & 7) as usize;
        assert_eq!(top, 5);
        assert_eq!(observed.registers[top], "bc9a785634125aa53e40");
        assert_eq!(observed.registers[top], before.registers[top]);
        assert_eq!(observed.tag_word, before.tag_word);
        assert_eq!(observed.status_word, HexU64(0x6d20));
        assert_eq!(observed.control_word, before.control_word);
        assert_eq!(observed.instruction_pointer, initial.rip);
        assert_eq!(observed.data_pointer, before.data_pointer);
        assert_eq!(observed.last_opcode, before.last_opcode);
    }

    #[test]
    fn frndint_loaded_negative_infinity_literal_fixture_preserves_raw_value() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31de_frndint_negative_infinity_fnstsw.case.json"
        ))
        .expect("S2-31de FLD/FRNDINT fixture schema");
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(3) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31de composition must expose the complete state");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_0006));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x6520);
        let observed = value.state.x87.unwrap();
        let before = initial.x87.unwrap();
        assert_eq!((observed.status_word.0 >> 11) & 7, 4);
        assert_eq!(observed.registers[4], "0000000000000080ffff");
        assert_eq!(observed.tag_word.0 & (3 << 8), 2 << 8);
        assert_eq!(observed.status_word, HexU64(0x6520));
        assert_eq!(observed.control_word, before.control_word);
        assert_eq!(observed.instruction_pointer, HexU64(initial.rip.0 + 2));
        assert_eq!(observed.data_pointer, HexU64(0x0000000000400000));
        assert_eq!(observed.last_opcode, before.last_opcode);
        for physical in 0..8 {
            if physical != 4 {
                assert_eq!(observed.registers[physical], before.registers[physical]);
            }
        }
    }

    #[test]
    fn frndint_loaded_quiet_nan_literal_fixture_preserves_payload_and_status() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31df_frndint_quiet_nan_fnstsw.case.json"
        ))
        .expect("S2-31df FLD/FRNDINT fixture schema");
        let code = fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31df code region")
            .initial_bytes_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&[0xd9, 0xfc]).is_err(),
            "FRNDINT remains JIT-denied"
        );
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(3) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31df composition must expose the complete state");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_0006));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x6520);
        let observed = value.state.x87.unwrap();
        let before = initial.x87.unwrap();
        assert_eq!((observed.status_word.0 >> 11) & 7, 4);
        assert_eq!(observed.registers[4], "efcdab89674523c1ff7f");
        assert_eq!(observed.tag_word.0 & (3 << 8), 2 << 8);
        assert_eq!(observed.status_word, HexU64(0x6520));
        assert_eq!(observed.control_word, before.control_word);
        assert_eq!(observed.instruction_pointer, HexU64(initial.rip.0 + 2));
        assert_eq!(observed.data_pointer, HexU64(0x0000000000400000));
        assert_eq!(observed.last_opcode, before.last_opcode);
        for physical in 0..8 {
            if physical != 4 {
                assert_eq!(observed.registers[physical], before.registers[physical]);
            }
        }
    }

    #[test]
    fn frndint_loaded_signaling_nan_fnclex_literal_fixture_quiets_and_clears_ie() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31dg_frndint_signaling_nan_fnclex.case.json"
        ))
        .expect("S2-31dg FLD/FRNDINT/FNSTSW/FNCLEX fixture schema");
        let code = fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31dg code region")
            .initial_bytes_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(od_jit::compile(&[0xd9, 0xfc]).is_err());
        assert!(od_jit::compile(&code).is_err());
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(6) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31dg composition must expose the complete state");
        };
        assert_eq!(value.state.rip, HexU64(0x0010_000c));
        assert_eq!(value.state.gpr[0].0 & 0xffff, 0x6521);
        let observed = value.state.x87.unwrap();
        let before = initial.x87.unwrap();
        assert_eq!((observed.status_word.0 >> 11) & 7, 4);
        assert_eq!(observed.registers[4], "efcdab89674523c1ff7f");
        assert_eq!(observed.tag_word.0 & (3 << 8), 2 << 8);
        assert_eq!(observed.status_word, HexU64(0x6500));
        assert_eq!(observed.control_word, before.control_word);
        assert_eq!(observed.instruction_pointer, HexU64(initial.rip.0 + 10));
        assert_eq!(observed.data_pointer, HexU64(0x0000000000400000));
        assert_eq!(observed.last_opcode, before.last_opcode);
        for physical in 0..8 {
            if physical != 4 {
                assert_eq!(observed.registers[physical], before.registers[physical]);
            }
        }
    }

    fn old() -> LoadedFixture {
        Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
        ))
        .unwrap()
    }

    #[test]
    fn interpreter_internal_invariant_failure_is_a_harness_error() {
        use od_isa::{DecodeErrorOrExecute, ExecuteError};
        assert!(matches!(
            interpreter_failure(DecodeErrorOrExecute::Execute(
                ExecuteError::UnexpectedCoreExit {
                    exit: od_isa::execute::RunExit::InstructionBudgetExhausted,
                }
            )),
            ExecutionFailure::Harness(_)
        ));
        assert!(matches!(
            interpreter_failure(DecodeErrorOrExecute::Execute(
                ExecuteError::InvalidInstructionPointer { rip: 1 }
            )),
            ExecutionFailure::Unsupported(_)
        ));
    }

    #[test]
    fn real_v2_execution_projects_injected_page_fault_and_backend_error_distinctly() {
        use od_isa::memory::{AccessKind, ArchitecturalFault, MemoryError};
        let input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap();
        for error in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(6),
                linear_address: Some(0x1000),
            }),
            MemoryError::Backend("injected".into()),
        ] {
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&input.fixture).unwrap();
            let before = memory.snapshot();
            memory.inject_once(0x1000, AccessKind::Write, error.clone());
            let result = execute_fixture_v2(&input, &mut memory);
            result.validate_for(&input).unwrap();
            let Observation::Available { value: observed } = &result.snapshot else {
                panic!("observed interpreter state");
            };
            assert_eq!(observed.memory, before);
            assert_eq!(observed.state.gpr[0], HexU64(12));
            assert_eq!(observed.state.rip, HexU64(0x4007));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(2) }
            );
            if matches!(error, MemoryError::Architectural(_)) {
                assert!(matches!(result.outcome, v2::Outcome::Exception {}));
                let Observation::Available { value: Some(fault) } = &observed.exception else {
                    panic!("observed page fault");
                };
                assert_eq!(fault.vector, 14);
                assert_eq!(
                    fault.error_code,
                    Observation::Available { value: HexU64(6) }
                );
                assert_eq!(
                    fault.fault_address,
                    Observation::Available {
                        value: HexU64(0x1000)
                    }
                );
                assert_eq!(fault.faulting_rip, HexU64(0x4007));
            } else {
                assert!(matches!(result.outcome, v2::Outcome::HarnessError { .. }));
                assert_eq!(observed.exception, Observation::Available { value: None });
            }
        }
    }

    // Synthetic snapshots exercise CLI plumbing and validation only, not CPU execution.
    fn synthetic(input: &v2::LoadedFixture) -> v2::RunResult {
        let mut state = input.fixture.initial_state.clone();
        state.rip = input.fixture.stop_rip.unwrap();
        v2::RunResult {
            schema_version: 2,
            case_id: input.fixture.case_id.clone(),
            input_sha256: input.input_sha256.clone(),
            profile: input.fixture.profile,
            backend: Backend::Interpreter,
            outcome: v2::Outcome::StopAddress {},
            instructions_executed: Observation::Unavailable {
                reason: "synthetic infrastructure test".into(),
            },
            snapshot: Observation::Available {
                value: v2::Snapshot {
                    state,
                    rflags_defined: HexU64(v2::INTEGER_FLAGS),
                    memory: od_testkit::memory::FlatMemory::from_fixture(&input.fixture)
                        .unwrap()
                        .snapshot(),
                    xorpd_prerequisites: input.fixture.xorpd_prerequisites,
                    minss_invalid_controls: input.fixture.minss_invalid_controls,
                    maxss_unmasked_controls: input.fixture.maxss_unmasked_controls,
                    minsd_unmasked_controls: input.fixture.minsd_unmasked_controls,
                    maxsd_unmasked_controls: input.fixture.maxsd_unmasked_controls,
                    minpd_unmasked_controls: input.fixture.minpd_unmasked_controls,
                    minps_unmasked_controls: input.fixture.minps_unmasked_controls,
                    maxps_unmasked_controls: input.fixture.maxps_unmasked_controls,
                    maxpd_unmasked_controls: input.fixture.maxpd_unmasked_controls,
                    cvtdq2pd_controls: input.fixture.cvtdq2pd_controls,
                    cvtdq2pd_memory_controls: input.fixture.cvtdq2pd_memory_controls,
                    cvtdq2ps_controls: input.fixture.cvtdq2ps_controls,
                    cvtdq2ps_memory_controls: input.fixture.cvtdq2ps_memory_controls,
                    cvtps2dq_controls: input.fixture.cvtps2dq_controls,
                    cvttps2dq_controls: input.fixture.cvttps2dq_controls,
                    cvttps2dq_m128_controls: input.fixture.cvttps2dq_m128_controls,
                    cvtps2dq_m128_controls: input.fixture.cvtps2dq_m128_controls,
                    cvtpd2dq_controls: input.fixture.cvtpd2dq_controls,
                    cvttpd2dq_controls: input.fixture.cvttpd2dq_controls,
                    cvttpd2dq_m128_controls: input.fixture.cvttpd2dq_m128_controls,
                    cvtpd2dq_m128_controls: input.fixture.cvtpd2dq_m128_controls,
                    sqrtss_controls: input.fixture.sqrtss_controls,
                    sqrtsd_controls: input.fixture.sqrtsd_controls,
                    sqrtps_controls: input.fixture.sqrtps_controls,
                    sqrtpd_controls: input.fixture.sqrtpd_controls,
                    subpd_controls: input.fixture.subpd_controls,
                    mulpd_controls: input.fixture.mulpd_controls,
                    addpd_controls: input.fixture.addpd_controls,
                    cvtpd2ps_controls: input.fixture.cvtpd2ps_controls,
                    addsd_controls: input.fixture.addsd_controls,
                    addss_controls: input.fixture.addss_controls,
                    addps_controls: input.fixture.addps_controls,
                    divpd_controls: input.fixture.divpd_controls,
                    divsd_controls: input.fixture.divsd_controls,
                    divps_controls: input.fixture.divps_controls,
                    divss_controls: input.fixture.divss_controls,
                    mulsd_controls: input.fixture.mulsd_controls,
                    mulss_controls: input.fixture.mulss_controls,
                    mulps_controls: input.fixture.mulps_controls,
                    subps_controls: input.fixture.subps_controls,
                    subsd_controls: input.fixture.subsd_controls,
                    subss_controls: input.fixture.subss_controls,
                    cvtss2sd_controls: input.fixture.cvtss2sd_controls,
                    cvtsd2ss_controls: input.fixture.cvtsd2ss_controls,
                    cvtpi2pd_memory_controls: input.fixture.cvtpi2pd_memory_controls,
                    cvtpi2pd_register_controls: input.fixture.cvtpi2pd_register_controls,
                    cvtpi2ps_memory_controls: input.fixture.cvtpi2ps_memory_controls,
                    cvtpi2ps_register_controls: input.fixture.cvtpi2ps_register_controls,
                    cvttps2pi_memory_controls: input.fixture.cvttps2pi_memory_controls,
                    cvtps2pi_memory_controls: input.fixture.cvtps2pi_memory_controls,
                    cvtps2pi_register_controls: input.fixture.cvtps2pi_register_controls,
                    cvtpd2pi_register_controls: input.fixture.cvtpd2pi_register_controls,
                    cvttpd2pi_register_controls: input.fixture.cvttpd2pi_register_controls,
                    cvttpd2pi_memory_controls: input.fixture.cvttpd2pi_memory_controls,
                    cvtpd2pi_memory_controls: input.fixture.cvtpd2pi_memory_controls,
                    cvtps2pd_controls: input.fixture.cvtps2pd_controls,
                    cvtps2pd_m64_controls: input.fixture.cvtps2pd_m64_controls,
                    cvtss2sd_m32_controls: input.fixture.cvtss2sd_m32_controls,
                    cvtsd2ss_m64_controls: input.fixture.cvtsd2ss_m64_controls,
                    cvtpd2ps_m128_controls: input.fixture.cvtpd2ps_m128_controls,
                    comiss_ucomiss_controls: input.fixture.comiss_ucomiss_controls,
                    cmpss_controls: input.fixture.cmpss_controls,
                    cmpsd_controls: input.fixture.cmpsd_controls,
                    cmppd_controls: input.fixture.cmppd_controls,
                    cmpps_controls: input.fixture.cmpps_controls,
                    comisd_ucomisd_controls: input.fixture.comisd_ucomisd_controls,
                    comisd_memory_rax_controls: input.fixture.comisd_memory_rax_controls,
                    ucomisd_memory_rax_controls: input.fixture.ucomisd_memory_rax_controls,
                    comiss_memory_rax_controls: input.fixture.comiss_memory_rax_controls,
                    ucomiss_memory_rax_controls: input.fixture.ucomiss_memory_rax_controls,
                    cmpss_memory_rax_controls: input.fixture.cmpss_memory_rax_controls,
                    cmpsd_memory_rax_controls: input.fixture.cmpsd_memory_rax_controls,
                    cmppd_memory_rax_controls: input.fixture.cmppd_memory_rax_controls,
                    cmpps_memory_rax_controls: input.fixture.cmpps_memory_rax_controls,
                    minsd_memory_rax_controls: input.fixture.minsd_memory_rax_controls,
                    maxss_memory_rax_controls: input.fixture.maxss_memory_rax_controls,
                    maxsd_memory_rax_controls: input.fixture.maxsd_memory_rax_controls,
                    minpd_memory_rax_controls: input.fixture.minpd_memory_rax_controls,
                    minps_memory_rax_controls: input.fixture.minps_memory_rax_controls,
                    sqrtps_memory_rax_controls: input.fixture.sqrtps_memory_rax_controls,
                    sqrtpd_memory_rax_controls: input.fixture.sqrtpd_memory_rax_controls,
                    sqrtsd_memory_rax_controls: input.fixture.sqrtsd_memory_rax_controls,
                    maxpd_memory_rax_controls: input.fixture.maxpd_memory_rax_controls,
                    maxps_memory_rax_controls: input.fixture.maxps_memory_rax_controls,
                    cvtsi2sd_r32_controls: input.fixture.cvtsi2sd_r32_controls,
                    cvtsi2sd_r32_memory_controls: input.fixture.cvtsi2sd_r32_memory_controls,
                    cvtsi2ss_r32_memory_controls: input.fixture.cvtsi2ss_r32_memory_controls,
                    cvtsi2ss_r64_memory_controls: input.fixture.cvtsi2ss_r64_memory_controls,
                    cvtsi2sd_r64_controls: input.fixture.cvtsi2sd_r64_controls,
                    cvtsi2sd_r64_memory_controls: input.fixture.cvtsi2sd_r64_memory_controls,
                    cvtsi2ss_controls: input.fixture.cvtsi2ss_controls,
                    cvtss2si_controls: input.fixture.cvtss2si_controls,
                    cvtsd2si_controls: input.fixture.cvtsd2si_controls,
                    cvtsd2si_r32_memory_controls: input.fixture.cvtsd2si_r32_memory_controls,
                    cvtsd2si_r64_memory_controls: input.fixture.cvtsd2si_r64_memory_controls,
                    cvttsd2si_r32_memory_controls: input.fixture.cvttsd2si_r32_memory_controls,
                    cvttsd2si_r64_memory_controls: input.fixture.cvttsd2si_r64_memory_controls,
                    cvtss2si_r32_memory_controls: input.fixture.cvtss2si_r32_memory_controls,
                    cvtss2si_r64_memory_controls: input.fixture.cvtss2si_r64_memory_controls,
                    cvttss2si_r32_memory_controls: input.fixture.cvttss2si_r32_memory_controls,
                    cvttss2si_r64_memory_controls: input.fixture.cvttss2si_r64_memory_controls,
                    fxsave64_cr0_controls: input.fixture.fxsave64_cr0_controls,
                    fxrstor64_cr0_controls: input.fixture.fxrstor64_cr0_controls,
                    rdtsc_source_calls: None,
                    exception: Observation::Available { value: None },
                },
            },
            build: build_metadata(),
            machine: machine_metadata(),
        }
    }

    fn compare(files: &Files, case: &[u8], left: &[u8], right: &[u8]) -> ExitCode {
        for (name, bytes) in [("case", case), ("left", left), ("right", right)] {
            fs::write(files.path(name), bytes).unwrap();
        }
        compare_results(&[
            files.path("left"),
            files.path("right"),
            "--case".into(),
            files.path("case"),
            "--json".into(),
        ])
    }

    #[test]
    fn conversion_preserves_input_and_refuses_existing_output() {
        let files = Files::new();
        let bytes = old().fixture.to_json().unwrap();
        fs::write(files.path("v1"), &bytes).unwrap();
        assert_eq!(
            convert_fixture_v1(&[files.path("v1"), files.path("v2")]),
            ExitCode::SUCCESS
        );
        let output = fs::read(files.path("v2")).unwrap();
        let converted = v2::Fixture::from_json(&output).unwrap();
        assert_eq!(
            converted.fixture.converted_from_v1_sha256,
            Some(od_testkit::sha256_hex(&bytes))
        );
        assert_ne!(converted.input_sha256, od_testkit::sha256_hex(&bytes));
        for target in ["v1", "v2"] {
            assert_eq!(
                convert_fixture_v1(&[files.path("v1"), files.path(target)]),
                ExitCode::from(EXIT_INVALID_INPUT)
            );
        }
        assert_eq!(fs::read(files.path("v1")).unwrap(), bytes);
        assert_eq!(fs::read(files.path("v2")).unwrap(), output);
        assert_eq!(
            convert_fixture_v1(&[files.path("v2"), files.path("unused")]),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
        assert!(!files.0.join("unused").exists());
    }

    #[test]
    fn comparison_dispatches_schemas_and_reports_mismatch_or_invalid_binding() {
        let files = Files::new();
        let v1 = old();
        let legacy = execute_fixture(&v1, Backend::Interpreter)
            .unwrap_or_else(|_| panic!("legacy fixture execution"));
        let legacy_bytes = legacy.to_json().unwrap();
        // Preserve exact source bytes: result digests bind to these bytes, not reserialization.
        let v1_bytes = include_bytes!("../../../tests/fixtures/x86/s0_09_mov_imm64.case.json");
        assert_eq!(
            compare(&files, v1_bytes, &legacy_bytes, &legacy_bytes),
            ExitCode::SUCCESS
        );
        let case = v2::Fixture::convert_v1(&v1).unwrap().to_json().unwrap();
        let loaded = v2::Fixture::from_json(&case).unwrap();
        let baseline = synthetic(&loaded);
        let baseline_bytes = baseline.to_json().unwrap();
        assert_eq!(
            compare(&files, &case, &baseline_bytes, &baseline_bytes),
            ExitCode::SUCCESS
        );
        let mut changed = baseline.clone();
        if let Observation::Available { value } = &mut changed.snapshot {
            value.state.gpr[15].0 ^= 1;
        }
        assert_eq!(
            compare(&files, &case, &baseline_bytes, &changed.to_json().unwrap()),
            ExitCode::from(EXIT_MISMATCH)
        );
        assert_eq!(
            compare(&files, &case, &legacy_bytes, &baseline_bytes),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
        let mut different_whitespace = case.clone();
        different_whitespace.push(b'\n');
        assert_eq!(
            compare(
                &files,
                &different_whitespace,
                &baseline_bytes,
                &baseline_bytes
            ),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
        let mut unknown: serde_json::Value = serde_json::from_slice(&case).unwrap();
        unknown["schema_version"] = serde_json::json!(3);
        assert_eq!(
            compare(
                &files,
                &serde_json::to_vec(&unknown).unwrap(),
                &baseline_bytes,
                &baseline_bytes
            ),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
    }

    #[test]
    fn v2_comparison_validates_both_bindings_before_returning_status() {
        let files = Files::new();
        let case = v2::Fixture::convert_v1(&old()).unwrap().to_json().unwrap();
        let loaded = v2::Fixture::from_json(&case).unwrap();
        let baseline = synthetic(&loaded);
        for (outcome, exit) in [
            (
                v2::Outcome::Unsupported {
                    reason: "test".into(),
                },
                EXIT_UNSUPPORTED,
            ),
            (
                v2::Outcome::TimedOut {
                    reason: "test".into(),
                },
                EXIT_HARNESS_FAILURE,
            ),
            (
                v2::Outcome::Signaled {
                    signal: 11,
                    reason: "test".into(),
                },
                EXIT_HARNESS_FAILURE,
            ),
            (
                v2::Outcome::HarnessError {
                    reason: "test".into(),
                },
                EXIT_HARNESS_FAILURE,
            ),
        ] {
            let mut incomplete = baseline.clone();
            incomplete.outcome = outcome;
            incomplete.snapshot = unavailable("synthetic test");
            let incomplete = incomplete.to_json().unwrap();
            let good = baseline.to_json().unwrap();
            for (left, right) in [(&incomplete, &good), (&good, &incomplete)] {
                assert_eq!(compare(&files, &case, left, right), ExitCode::from(exit));
            }
            for kind in ["digest", "layout"] {
                let mut invalid = baseline.clone();
                if kind == "digest" {
                    invalid.input_sha256 = "0".repeat(64);
                } else if let Observation::Available { value } = &mut invalid.snapshot {
                    value.memory[0].guest_address.0 += 1;
                }
                let invalid = invalid.to_json().unwrap();
                for (left, right) in [(&incomplete, &invalid), (&invalid, &incomplete)] {
                    assert_eq!(
                        compare(&files, &case, left, right),
                        ExitCode::from(EXIT_INVALID_INPUT)
                    );
                }
            }
        }
    }

    #[test]
    fn fs_base_profile_cli_result_contains_observed_base_and_alias_load() {
        let input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22a_fs_base_qword_load_alias.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&input.fixture).unwrap();
        let result = execute_fixture_v2(&input, &mut memory);
        result.validate_for(&input).unwrap();
        let json = result.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            value["snapshot"]["value"]["state"]["fs_base"],
            "0xffff800000000000"
        );
        assert_eq!(
            value["snapshot"]["value"]["state"]["gpr"][0],
            "0x8877665544332211"
        );
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
    }

    #[test]
    fn x87_fadd_fixture_runner_executes_all_register_sources_and_rejects_bad_tag() {
        fn normal(significand: u64, exponent: u16) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        }

        for source in 0u8..=7 {
            let mut input = v2::Fixture::from_json(include_bytes!(
                "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
            ))
            .unwrap()
            .fixture;
            input.case_id = format!("local_fadd_st0_st{source}");
            input.profile = v2::Profile::LongX87StateV1;
            input.instruction_budget = 1;
            input.initial_state.rip = HexU64(0x4000);
            input.stop_rip = Some(HexU64(0x4002));
            input.expected_exit = v2::ExpectedExit::StopAddress;
            let code = input
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.guest_address = HexU64(0x4000);
            code.length = 2;
            code.initial_bytes_hex = format!("d8c{source:x}");
            code.comparison_mask_hex = "ffff".into();

            let top = if source == 7 || source == 1 {
                7
            } else {
                (source + 2) & 7
            };
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0400;
            x87.tag_word = 0;
            let one = normal(0x8000_0000_0000_0000, 0x3fff);
            x87.registers[usize::from(top)] = one;
            x87.registers[usize::from((top + source) & 7)] = one;
            input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

            let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let result = execute_fixture_v2(&loaded, &mut memory);
            result.validate_for(&loaded).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "source {source}, TOP {top}: {:?}",
                result.outcome
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(1) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("x87 FADD result must be observable");
            };
            let observed_x87 = value.state.x87.unwrap();
            assert_eq!(
                observed_x87.registers[usize::from(top)],
                "00000000000000800040"
            );
            assert_eq!((observed_x87.status_word.0 >> 11) & 7, u64::from(top));
            assert_eq!(observed_x87.status_word.0 & (1 << 9), 0);
            assert_eq!(observed_x87.instruction_pointer, HexU64(0x4000));
        }

        let mut rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap()
        .fixture;
        rejected.profile = v2::Profile::LongX87StateV1;
        rejected.instruction_budget = 1;
        rejected.initial_state.rip = HexU64(0x4000);
        rejected.stop_rip = Some(HexU64(0x4002));
        rejected.expected_exit = v2::ExpectedExit::StopAddress;
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.guest_address = HexU64(0x4000);
        code.length = 2;
        code.initial_bytes_hex = "d8c7".into();
        code.comparison_mask_hex = "ffff".into();
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0;
        x87.registers[0] = normal(0x8000_0000_0000_0000, 0x3fff);
        x87.registers[7] = normal(0x8000_0000_0000_0000, 0x3fff);
        x87.tag_word |= 3 << (7 * 2);
        rejected.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
        let loaded = v2::Fixture::from_json(&rejected.to_json().unwrap()).unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let result = execute_fixture_v2(&loaded, &mut memory);
        result.validate_for(&loaded).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        let Observation::Available { value } = result.snapshot else {
            panic!("rejected x87 state must remain observable");
        };
        assert_eq!(value.state.x87.unwrap(), v2::X87Snapshot::from(&x87));
    }

    #[test]
    fn x87_faddp_fixture_runner_covers_every_index_and_top() {
        fn normal(significand: u64, exponent: u16) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        }

        let one = normal(0x8000_0000_0000_0000, 0x3fff);
        let two = "00000000000000800040";
        for top in 0u8..=7 {
            for destination in 0u8..=7 {
                let old_top = top;
                let physical_destination = top.wrapping_add(destination) & 7;
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_faddp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x5000);
                input.stop_rip = Some(HexU64(0x5002));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x5000);
                code.length = 2;
                code.initial_bytes_hex = format!("dec{destination:x}");
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4500 | 0x0200;
                x87.tag_word = u16::MAX;
                for physical in 0..8 {
                    x87.registers[physical] = [physical as u8; 10];
                }
                x87.registers[usize::from(old_top)] = one;
                x87.registers[usize::from(physical_destination)] = one;
                x87.set_tag(old_top, od_isa::state::X87Tag::Valid);
                x87.set_tag(physical_destination, od_isa::state::X87Tag::Valid);
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(
                    matches!(result.outcome, v2::Outcome::StopAddress {}),
                    "ST({destination}), TOP={top}: {:?}",
                    result.outcome
                );
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FADDP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[usize::from(physical_destination)] =
                    normal(0x8000_0000_0000_0000, 0x4000);
                expected.set_tag(old_top, od_isa::state::X87Tag::Empty);
                let new_top = old_top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x5000;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
                assert_eq!(observed.registers[usize::from(physical_destination)], two);
            }
        }
    }

    #[test]
    fn x87_fmulp_fixture_runner_covers_every_index_top_and_square_alias() {
        fn normal(significand: u64, exponent: u16) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        }

        let one_and_half = normal(0xc000_0000_0000_0000, 0x3fff);
        let two = normal(0x8000_0000_0000_0000, 0x4000);
        let three = normal(0xc000_0000_0000_0000, 0x4000);
        let two_and_quarter = normal(0x9000_0000_0000_0000, 0x4000);
        for top in 0u8..=7 {
            for destination in 0u8..=7 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + destination) & 7);
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_fmulp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x5300);
                input.stop_rip = Some(HexU64(0x5302));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x5300);
                code.length = 2;
                code.initial_bytes_hex = format!("de{:x}", 0xc8 | destination);
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                for physical in 0..8 {
                    x87.registers[physical] = [0x20 + physical as u8; 10];
                }
                x87.registers[old_top] = one_and_half;
                x87.set_tag(top, od_isa::state::X87Tag::Valid);
                if destination != 0 {
                    x87.registers[physical_destination] = two;
                    x87.set_tag(physical_destination as u8, od_isa::state::X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(
                    matches!(result.outcome, v2::Outcome::StopAddress {}),
                    "ST({destination}), TOP={top}: {:?}",
                    result.outcome
                );
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FMULP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[physical_destination] = if destination == 0 {
                    two_and_quarter
                } else {
                    three
                };
                expected.set_tag(top, od_isa::state::X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x5300;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
            }
        }

        // The fixture schema accepts DE C8 with an explicit unsupported exit
        // and no stop address. This instance rejects atomically because the
        // finite product is inexact; the i=0 exact square succeeds above.
        let mut input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap()
        .fixture;
        input.case_id = "local_fmulp_i0_inexact_rejection".into();
        input.profile = v2::Profile::LongX87StateV1;
        input.instruction_budget = 1;
        input.initial_state.rip = HexU64(0x5400);
        input.stop_rip = None;
        input.expected_exit = v2::ExpectedExit::Unsupported;
        let code = input
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.guest_address = HexU64(0x5400);
        code.length = 2;
        code.initial_bytes_hex = "dec8".into();
        code.comparison_mask_hex = "ffff".into();
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = normal(0x8000_0000_0000_0001, 0x3fff);
        x87.set_tag(0, od_isa::state::X87Tag::Valid);
        input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
        let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let result = execute_fixture_v2(&loaded, &mut memory);
        result.validate_for(&loaded).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("unsupported x87 square rejection must be observable");
        };
        assert_eq!(value.state, loaded.fixture.initial_state);
    }

    #[test]
    fn x87_fsubrp_fixture_runner_covers_every_index_top_and_alias_rejection() {
        let normal = |significand: u64, exponent: u16| {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        };
        let one = normal(0x8000_0000_0000_0000, 0x3fff);
        let one_and_half = normal(0xc000_0000_0000_0000, 0x3fff);
        let half = normal(0x8000_0000_0000_0000, 0x3ffe);
        for top in 0u8..8 {
            for destination in 1u8..8 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + destination) & 7);
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_fsubrp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x7100);
                input.stop_rip = Some(HexU64(0x7102));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x7100);
                code.length = 2;
                code.initial_bytes_hex = format!("dee{destination:x}");
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word =
                    u16::MAX & !(3 << (old_top * 2)) & !(3 << (physical_destination * 2));
                for physical in 0..8 {
                    x87.registers[physical] = [physical as u8; 10];
                }
                x87.registers[old_top] = one_and_half;
                x87.registers[physical_destination] = one;
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FSUBRP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[physical_destination] = half;
                expected.set_tag(top, od_isa::state::X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x7100;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
            }
        }

        for top in 0u8..8 {
            let mut input = v2::Fixture::from_json(include_bytes!(
                "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
            ))
            .unwrap()
            .fixture;
            input.case_id = format!("local_fsubrp_alias_top{top}");
            input.profile = v2::Profile::LongX87StateV1;
            input.instruction_budget = 1;
            input.initial_state.rip = HexU64(0x7200);
            input.stop_rip = None;
            input.expected_exit = v2::ExpectedExit::Unsupported;
            let code = input
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.guest_address = HexU64(0x7200);
            code.length = 2;
            code.initial_bytes_hex = "dee0".into();
            code.comparison_mask_hex = "ffff".into();
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0200;
            x87.tag_word = u16::MAX & !(3 << (usize::from(top) * 2));
            x87.registers[usize::from(top)] = one;
            input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
            let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let result = execute_fixture_v2(&loaded, &mut memory);
            result.validate_for(&loaded).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("unsupported x87 alias remains observable");
            };
            assert_eq!(value.state, loaded.fixture.initial_state);
        }
    }

    #[test]
    fn x87_fdivp_fixture_runner_covers_every_index_top_and_inexact_rejection() {
        fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..]
                .copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
            bytes
        }
        let fifteen = normal(0xf000_0000_0000_0000, 0x4002, false);
        let three = normal(0xc000_0000_0000_0000, 0x4000, false);
        let five = normal(0xa000_0000_0000_0000, 0x4001, false);
        let one = normal(0x8000_0000_0000_0000, 0x3fff, false);

        for top in 0u8..8 {
            for destination in 0u8..8 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + destination) & 7);
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_fdivp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x5500);
                input.stop_rip = Some(HexU64(0x5502));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x5500);
                code.length = 2;
                code.initial_bytes_hex = format!("de{:x}", 0xf8 | destination);
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                for physical in 0..8 {
                    x87.registers[physical] = [0x30 + physical as u8; 10];
                }
                if destination == 0 {
                    x87.registers[old_top] = normal(0xd123_4567_89ab_cdef, 0x4000, top & 1 != 0);
                } else {
                    x87.registers[physical_destination] = fifteen;
                    x87.set_tag((top + destination) & 7, od_isa::state::X87Tag::Valid);
                    x87.registers[old_top] = three;
                }
                x87.set_tag(top, od_isa::state::X87Tag::Valid);
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FDIVP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[physical_destination] =
                    if destination == 0 { one } else { five };
                expected.set_tag(top, od_isa::state::X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x5500;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
            }
        }

        let mut input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap()
        .fixture;
        input.case_id = "local_fdivp_inexact_rejection".into();
        input.profile = v2::Profile::LongX87StateV1;
        input.instruction_budget = 1;
        input.initial_state.rip = HexU64(0x5600);
        input.stop_rip = None;
        input.expected_exit = v2::ExpectedExit::Unsupported;
        let code = input
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.guest_address = HexU64(0x5600);
        code.length = 2;
        code.initial_bytes_hex = "def9".into();
        code.comparison_mask_hex = "ffff".into();
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = normal(0xc000_0000_0000_0000, 0x4000, false);
        x87.registers[1] = normal(0x8000_0000_0000_0000, 0x3fff, false);
        input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
        let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let result = execute_fixture_v2(&loaded, &mut memory);
        result.validate_for(&loaded).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("inexact FDIVP state must remain observable");
        };
        assert_eq!(value.state, loaded.fixture.initial_state);
    }

    #[test]
    fn x87_fucom_literal_fixtures_run_success_and_atomic_rhs_rejection() {
        let success = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ag_fucom_equal_fnstsw.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&success.fixture).unwrap();
        let result = execute_fixture_v2(&success, &mut memory);
        result.validate_for(&success).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOM literal success fixture must be observable");
        };
        let before = success.fixture.initial_state.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x4000));
        assert_eq!(after.tag_word, before.tag_word);
        assert_eq!(after.registers, before.registers);
        assert_eq!(value.state.gpr[0].get() & 0xffff, 0x4000);

        let rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ag_fucom_invalid_rhs_tag.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&rejected.fixture).unwrap();
        let result = execute_fixture_v2(&rejected, &mut memory);
        result.validate_for(&rejected).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOM rejected fixture must preserve an observable state");
        };
        assert_eq!(value.state, rejected.fixture.initial_state);
    }

    #[test]
    fn x87_fucomp_literal_fixtures_run_compare_pop_and_atomic_rejection() {
        let success = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ah_fucomp_equal_fnstsw.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&success.fixture).unwrap();
        let result = execute_fixture_v2(&success, &mut memory);
        result.validate_for(&success).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMP literal success fixture must be observable");
        };
        let before = success.fixture.initial_state.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x4800));
        assert_eq!(after.tag_word, HexU64(0xfff3));
        assert_eq!(after.registers, before.registers);
        assert_eq!(value.state.gpr[0].get() & 0xffff, 0x4800);

        let rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ah_fucomp_invalid_rhs_tag.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&rejected.fixture).unwrap();
        let result = execute_fixture_v2(&rejected, &mut memory);
        result.validate_for(&rejected).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMP rejected fixture must preserve an observable state");
        };
        assert_eq!(value.state, rejected.fixture.initial_state);
    }

    #[test]
    fn x87_fucompp_literal_fixtures_run_compare_double_pop_and_atomic_rejection() {
        let success = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ai_fucompp_equal_fnstsw.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&success.fixture).unwrap();
        let result = execute_fixture_v2(&success, &mut memory);
        result.validate_for(&success).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMPP literal success fixture must be observable");
        };
        let before = success.fixture.initial_state.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x5000));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, before.registers);
        assert_eq!(value.state.gpr[0].get() & 0xffff, 0x5000);

        let rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ai_fucompp_invalid_rhs_tag.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&rejected.fixture).unwrap();
        let result = execute_fixture_v2(&rejected, &mut memory);
        result.validate_for(&rejected).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMPP rejected fixture must preserve an observable state");
        };
        assert_eq!(value.state, rejected.fixture.initial_state);
    }

    #[test]
    fn fistp_m32int_literal_fixtures_store_and_reject_without_commit() {
        for (name, bytes, expected_rax) in [
            (
                "s2_31b_fistp_m32int_success.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_success.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0001,
            ),
            (
                "s2_31b_fistp_m32int_fnstsw.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4f20,
            ),
            (
                "s2_31c_fist_m32int_success.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_success.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0001,
            ),
            (
                "s2_31c_fist_m32int_fnstsw.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4720,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(if name.contains("fnstsw") { 2 } else { 1 })
                },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} must have an observable state");
            };
            let before = fixture.fixture.initial_state.x87.as_ref().unwrap();
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let is_fist = name.contains("31c");
            let expected_fsw = if is_fist { 0x4720 } else { 0x4f20 };
            let expected_tags = if is_fist { 0xfffc } else { 0xffff };
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word, HexU64(expected_tags), "{name}");
            assert_eq!(after.registers, before.registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x100000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x300001), "{name}");
            let left = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_left")
                .unwrap();
            let right = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            assert_eq!(left.bytes_hex, "02", "{name}");
            assert_eq!(right.bytes_hex, "000000eeff11223344556677889900", "{name}");
        }

        for (name, bytes) in [
            (
                "s2_31b_fistp_m32int_invalid_tag.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_invalid_tag.case.json"
                )
                .as_slice(),
            ),
            (
                "s2_31c_fist_m32int_invalid_tag.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_invalid_tag.case.json"
                )
                .as_slice(),
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::Unsupported { .. }),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} must have an observable state");
            };
            assert_eq!(value.state, fixture.fixture.initial_state, "{name}");
            assert_eq!(memory.snapshot(), initial_memory, "{name}");
        }

        for (name, bytes, expected_fsw, expected_tag) in [
            (
                "s2_31b_fistp_m32int_overflow.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_overflow.case.json"
                )
                .as_slice(),
                0x0801,
                0xffff,
            ),
            (
                "s2_31c_fist_m32int_overflow.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_overflow.case.json"
                )
                .as_slice(),
                0x0001,
                0xfffc,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.x87.as_ref().unwrap();
            let before_registers = before.registers.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(1) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} must have an observable state");
            };
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(after.registers, before_registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x0030_0001), "{name}");
            let regions = memory.snapshot();
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "data")
                    .unwrap()
                    .bytes_hex,
                "00000080eeff11223344556677889900",
                "{name}"
            );
        }

        let mut nearby_form: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_success.case.json"
        ))
        .unwrap();
        nearby_form["memory"][0]["initial_bytes_hex"] = serde_json::Value::String("db11".into());
        assert!(
            v2::Fixture::from_json(&serde_json::to_vec(&nearby_form).unwrap()).is_err(),
            "the literal schema must reject nearby DB ModR/M forms"
        );

        let mut exact_min_must_execute: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_success.case.json"
        ))
        .unwrap();
        exact_min_must_execute["initial_state"]["x87"]["registers"][0] =
            serde_json::Value::String("00000000000000801ec0".into());
        exact_min_must_execute["stop_rip"] = serde_json::Value::Null;
        exact_min_must_execute["expected_exit"] = serde_json::Value::String("unsupported".into());
        assert!(
            v2::Fixture::from_json(&serde_json::to_vec(&exact_min_must_execute).unwrap()).is_err(),
            "the schema must not classify exact negative 2^31 as an overflow rejection"
        );
    }

    #[test]
    fn fistp_m16int_positive_overflow_fixture_stores_indefinite_and_observes_ie() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ck_fistp_m16int_positive_overflow_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ck overflow fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ck overflow fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d01));
        assert_eq!(after.status_word, HexU64(0x4d01));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "80ccddeeff11223344556677889900"
        );
    }

    #[test]
    fn s2_31da_fistp_m16_unnormal_fixture_stores_word_indefinite_and_pops() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31da_fistp_m16int_negative_unnormal.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31da fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31da code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31da snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "aa"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "0080ddeeff11223344556677889900aa"
        );
    }

    #[test]
    fn s2_31db_fistp_m64_unnormal_fixture_stores_qword_indefinite_and_pops() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31db_fistp_m64int_negative_unnormal.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31db fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31db code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31db snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "aa"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000000000000008044556677889900aa"
        );
    }

    #[test]
    fn fistp_after_subnormal_load_preserves_de_and_observes_fnstsw() {
        for (name, bytes, expected_right, expected_rip) in [
            (
                "S2-31cm m32 loaded negative max subnormal",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cm_fistp_m32int_loaded_max_subnormal_fnstsw.case.json").as_slice(),
                "ffffffeeff11223344556677889900",
                0x0010_0004,
            ),
            (
                "S2-31cm m64 loaded negative max subnormal",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cm_fistp_m64int_loaded_max_subnormal_fnstsw.case.json").as_slice(),
                "ffffffffffffff3344556677889900",
                0x0010_0004,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(2) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let after = value.state.x87.as_ref().unwrap();
            let expected_fsw = 0x4f22;
            assert_eq!(value.state.gpr[0], HexU64(0x0030_0000 | expected_fsw), "{name}");
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.status_word.0 & 0x3800, 0x0800, "{name}");
            assert_eq!(after.tag_word, HexU64(0xffff), "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x0030_0001), "{name}");
            assert_eq!(value.state.rip, HexU64(expected_rip), "{name}");
            let regions = memory.snapshot();
            assert_eq!(
                regions.iter().find(|region| region.region_id == "data_left").unwrap().bytes_hex,
                "ff",
                "{name}"
            );
            assert_eq!(
                regions.iter().find(|region| region.region_id == "data_right").unwrap().bytes_hex,
                expected_right,
                "{name}"
            );
        }
    }

    #[test]
    fn bounded_integer_stores_admit_sticky_ue_and_observe_fnstsw() {
        for (name, bytes, expected_right) in [
            (
                "S2-31cn m32 sticky UE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cn_fistp_m32int_sticky_ue_fnstsw.case.json").as_slice(),
                "000000eeff11223344556677889900",
            ),
            (
                "S2-31cn m64 sticky UE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cn_fistp_m64int_sticky_ue_fnstsw.case.json").as_slice(),
                "000000000000003344556677889900",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(2) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(value.state.gpr[0], HexU64(0x0030_4d30), "{name}");
            assert_eq!(after.status_word, HexU64(0x4d30), "{name}");
            assert_eq!(after.tag_word, HexU64(0xffff), "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x0030_0001), "{name}");
            let regions = memory.snapshot();
            assert_eq!(
                regions.iter().find(|region| region.region_id == "data_left").unwrap().bytes_hex,
                "00",
                "{name}"
            );
            assert_eq!(
                regions.iter().find(|region| region.region_id == "data_right").unwrap().bytes_hex,
                expected_right,
                "{name}"
            );
        }
    }

    #[test]
    fn bounded_integer_stores_preserve_sticky_oe_and_observe_fnstsw() {
        for (name, bytes, expected_right) in [
            (
                "S2-31co m32 sticky OE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31co_fistp_m32int_sticky_oe_fnstsw.case.json").as_slice(),
                "000000eeff11223344556677889900",
            ),
            (
                "S2-31co m64 sticky OE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31co_fistp_m64int_sticky_oe_fnstsw.case.json").as_slice(),
                "000000000000003344556677889900",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(result.instructions_executed, Observation::Available { value: HexU64(2) }, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(value.state.gpr[0], HexU64(0x0030_4d28), "{name}");
            assert_eq!(after.status_word, HexU64(0x4d28), "{name}");
            assert_eq!(after.tag_word, HexU64(0xffff), "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x0030_0001), "{name}");
            let regions = memory.snapshot();
            assert_eq!(regions.iter().find(|region| region.region_id == "data_left").unwrap().bytes_hex, "00", "{name}");
            assert_eq!(regions.iter().find(|region| region.region_id == "data_right").unwrap().bytes_hex, expected_right, "{name}");
        }
    }

    #[test]
    fn bounded_integer_stores_preserve_sticky_ze_and_observe_fnstsw() {
        for (name, bytes, expected_right) in [
            (
                "S2-31cp m32 sticky ZE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cp_fistp_m32int_sticky_ze_fnstsw.case.json").as_slice(),
                "000000eeff11223344556677889900",
            ),
            (
                "S2-31cp m64 sticky ZE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31cp_fistp_m64int_sticky_ze_fnstsw.case.json").as_slice(),
                "000000000000003344556677889900",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(result.instructions_executed, Observation::Available { value: HexU64(2) }, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(value.state.gpr[0], HexU64(0x0030_4d24), "{name}");
            assert_eq!(after.status_word, HexU64(0x4d24), "{name}");
            assert_eq!(after.tag_word, HexU64(0xffff), "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x0030_0001), "{name}");
            let regions = memory.snapshot();
            assert_eq!(regions.iter().find(|region| region.region_id == "data_left").unwrap().bytes_hex, "00", "{name}");
            assert_eq!(regions.iter().find(|region| region.region_id == "data_right").unwrap().bytes_hex, expected_right, "{name}");
        }
    }

    #[test]
    fn fist_m16int_positive_roundback_fixture_preserves_in_range_result_and_observes_pe() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ck_fist_m16int_positive_roundback_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ck roundback fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ck roundback fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4520));
        assert_eq!(after.status_word, HexU64(0x4520));
        assert_eq!(after.tag_word, HexU64(0xfffc));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "ff"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "7fccddeeff11223344556677889900"
        );
    }

    #[test]
    fn fist_m16int_previous_overflow_fixtures_now_store_integer_indefinite() {
        for (name, bytes, expected_fsw, expected_tag) in [
            (
                "s2_31d_fistp_m16int_overflow.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31d_fistp_m16int_overflow.case.json"
                )
                .as_slice(),
                0x4d01,
                0xffff,
            ),
            (
                "s2_31e_fist_m16int_overflow.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31e_fist_m16int_overflow.case.json"
                )
                .as_slice(),
                0x4501,
                0xfffc,
            ),
            (
                "s2_31d_fistp_m16int_invalid_exponent.case.json",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31d_fistp_m16int_invalid_exponent.case.json").as_slice(),
                0x4d01,
                0xffff,
            ),
            (
                "s2_31e_fist_m16int_invalid_exponent.case.json",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31e_fist_m16int_invalid_exponent.case.json").as_slice(),
                0x4501,
                0xfffc,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} schema: {error}"));
            let initial_x87 = fixture.fixture.initial_state.x87.as_ref().unwrap();
            let initial_registers = initial_x87.registers.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(1) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(after.registers, initial_registers, "{name}");
            assert_eq!(after.data_pointer, HexU64(0x0030_0001), "{name}");
            let regions = memory.snapshot();
            assert_eq!(
                regions
                    .iter()
                    .find(|r| r.region_id == "data_left")
                    .unwrap()
                    .bytes_hex,
                "00",
                "{name}"
            );
            assert_eq!(
                regions
                    .iter()
                    .find(|r| r.region_id == "data_right")
                    .unwrap()
                    .bytes_hex,
                "80ccddeeff11223344556677889900",
                "{name}"
            );
        }
    }

    #[test]
    fn fist_m32int_signed_zero_cli_fixtures_store_zero_and_observe_status() {
        for (name, bytes, expected_rax, expected_fsw, expected_tag_word) in [
            (
                "s2_31bt_fist_m32int_negative_zero_fnstsw.case.json",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bt_fist_m32int_negative_zero_fnstsw.case.json").as_slice(),
                0x0000_0000_0030_4520,
                0x4520,
                0xfffd,
            ),
            (
                "s2_31bt_fistp_m32int_positive_zero_fnstsw.case.json",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bt_fistp_m32int_positive_zero_fnstsw.case.json").as_slice(),
                0x0000_0000_0030_4d20,
                0x4d20,
                0xffff,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(2) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} must have an observable state");
            };
            let after = value.state.x87.as_ref().unwrap();
            let initial = before.x87.as_ref().unwrap();
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(after.registers, initial.registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x100000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x300001), "{name}");
            let regions = memory.snapshot();
            let left = regions.iter().find(|region| region.region_id == "data_left").unwrap();
            let right = regions.iter().find(|region| region.region_id == "data_right").unwrap();
            assert_eq!(left.bytes_hex, "00", "{name}");
            assert_eq!(right.bytes_hex, "000000eeff11223344556677889900", "{name}");
        }
    }

    #[test]
    fn fstp_m64fp_literal_fixtures_store_then_pop_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31l_fstp_m64fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31l_fstp_m64fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4d00,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31l fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31l fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(
                after.registers,
                fixture
                    .fixture
                    .initial_state
                    .x87
                    .as_ref()
                    .unwrap()
                    .registers
            );
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], "000000000000f83f");
        }
    }

    #[test]
    fn fstp_m80fp_literal_fixtures_store_raw_binary80_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4d00,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31q fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31q fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(
                after.registers,
                fixture
                    .fixture
                    .initial_state
                    .x87
                    .as_ref()
                    .unwrap()
                    .registers
            );
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..28], "00000000000000c0ff3f");
        }

        let template: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp.case.json"
        ))
        .unwrap();
        for bytes in ["db39", "48db38", "9bdb38", "db38dfe1"] {
            let mut nearby = template.clone();
            let code = &mut nearby["memory"][0];
            code["initial_bytes_hex"] = serde_json::Value::String(bytes.into());
            code["length"] = serde_json::Value::from(bytes.len() / 2);
            code["comparison_mask_hex"] = serde_json::Value::String("ff".repeat(bytes.len() / 2));
            nearby["stop_rip"] = serde_json::Value::String(format!(
                "0x{:016x}",
                0x0010_0000 + (bytes.len() / 2) as u64
            ));
            assert!(
                v2::Fixture::from_json(&serde_json::to_vec(&nearby).unwrap()).is_err(),
                "the S2-31q schema must reject {bytes}"
            );
        }
    }

    #[test]
    fn fstp_m80fp_literal_zero_fixtures_preserve_sign_and_run_fnstsw() {
        for (bytes, expected_rax, retired, raw) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31an_fstp_m80fp_zero.case.json")
                    .as_slice(),
                0x0000_0000_0030_0008,
                1,
                "00000000000000000000",
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31an_fstp_m80fp_negative_zero_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_4d00,
                2,
                "00000000000000000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31an fixture schema");
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(retired) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31an fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0008));
            assert_eq!(
                after.registers,
                fixture.fixture.initial_state.x87.as_ref().unwrap().registers
            );
            let stack = memory.snapshot().into_iter()
                .find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[16..36], raw);
        }
    }

    #[test]
    fn fstp_m80fp_literal_infinity_fixtures_preserve_raw_sign_and_run_fnstsw() {
        for (bytes, expected_rax, retired, raw) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ao_fstp_m80fp_infinity.case.json")
                    .as_slice(),
                0x0000_0000_0030_0008,
                1,
                "0000000000000080ff7f",
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ao_fstp_m80fp_negative_infinity_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_4d00,
                2,
                "0000000000000080ffff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31ao fixture schema");
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(retired) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31ao fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0008));
            assert_eq!(
                after.registers,
                fixture.fixture.initial_state.x87.as_ref().unwrap().registers
            );
            let stack = memory.snapshot().into_iter()
                .find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[16..36], raw);
        }
    }

    #[test]
    fn fstp_m80fp_literal_qnan_fixtures_preserve_payload_and_run_fnstsw() {
        for (bytes, expected_rax, retired, raw) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ar_fstp_m80fp_qnan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0008,
                1,
                "01000000000000c0ff7f",
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ar_fstp_m80fp_negative_indefinite_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_4d00,
                2,
                "00000000000000c0ffff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31ar fixture schema");
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(retired) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31ar fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0008));
            assert_eq!(
                after.registers,
                fixture.fixture.initial_state.x87.as_ref().unwrap().registers
            );
            let stack = memory.snapshot().into_iter()
                .find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[16..36], raw);
        }
    }

    #[test]
    fn fstp_m80fp_literal_snan_fixtures_preserve_signaling_bits_and_run_fnstsw() {
        for (bytes, expected_rax, retired, raw) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31as_fstp_m80fp_snan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0008,
                1,
                "0100000000000080ff7f",
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31as_fstp_m80fp_negative_snan_payload_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_4d00,
                2,
                "efcdab8967452381ffff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31as fixture schema");
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(retired) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31as fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0008));
            assert_eq!(
                after.registers,
                fixture.fixture.initial_state.x87.as_ref().unwrap().registers
            );
            let stack = memory.snapshot().into_iter()
                .find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[16..36], raw);
        }
    }

    #[test]
    fn fstp_m80fp_literal_subnormal_fixtures_preserve_raw_bytes_and_run_fnstsw() {
        for (bytes, expected_rax, retired, raw) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31at_fstp_m80fp_subnormal.case.json")
                    .as_slice(),
                0x0000_0000_0030_0008,
                1,
                "01000000000000000000",
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31at_fstp_m80fp_negative_subnormal_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_4d00,
                2,
                "ffffffffffffff7f0080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31at fixture schema");
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(retired) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31at fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0008));
            assert_eq!(
                after.registers,
                fixture.fixture.initial_state.x87.as_ref().unwrap().registers
            );
            let stack = memory.snapshot().into_iter()
                .find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[16..36], raw);
        }
    }

    #[test]
    fn fstp_m80fp_literal_sticky_status_fixtures_preserve_ie_de_oe_ue_pe() {
        for (bytes, expected_rax, expected_status, retired) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_sticky_status.case.json")
                    .as_slice(),
                0x0000_0000_0030_0008,
                0x4d23,
                1,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ba_fstp_m80fp_negative_snan_sticky_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_4d23,
                0x4d23,
                2,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bi_fstp_m80fp_negative_snan_sticky_ue_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_4d10,
                0x4d10,
                2,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bs_fstp_m80fp_negative_snan_sticky_oe_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_0808,
                0x0808,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31ba fixture schema");
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(retired) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31ba fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(expected_status));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0008));
            assert_eq!(
                after.registers,
                fixture.fixture.initial_state.x87.as_ref().unwrap().registers
            );
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(
                &stack.bytes_hex[16..36],
                &fixture.fixture.initial_state.x87.as_ref().unwrap().registers[0]
            );
        }
    }

    #[test]
    fn fld_m32fp_literal_fixtures_expand_binary32_values_and_reject_bad_class() {
        for (bytes, expected_rax, expected_raw, expected_tag_word, retired, expected_status) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31s_fld_m32fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "00000000000000000000",
                0x7ffc,
                1,
                0x7d00,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31s_fld_m32fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                "00000000000000000080",
                0x7ffc,
                2,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_infinity.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "0000000000000080ff7f",
                0xbffc,
                1,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_negative_infinity_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                "0000000000000080ffff",
                0xbffc,
                2,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_qnan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "00000000000000c0ff7f",
                0xbffc,
                1,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_negative_qnan_payload_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                "00000000004523c1ffff",
                0xbffc,
                2,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_snan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "00000000000100c0ff7f",
                0xbffc,
                1,
                0x7d01,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_negative_snan_payload_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d01,
                "00000000004523e1ffff",
                0xbffc,
                2,
                0x7d01,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31ah fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31s fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(expected_status));
            assert_eq!(after.tag_word, HexU64(expected_tag_word));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(after.registers[7], expected_raw);
            for physical in 0..7 {
                assert_eq!(
                    after.registers[physical],
                    fixture
                        .fixture
                        .initial_state
                        .x87
                        .as_ref()
                        .unwrap()
                        .registers[physical]
                );
            }
            let regions = memory.snapshot();
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "stack")
                    .unwrap()
                    .bytes_hex,
                fixture.fixture.memory[1].initial_bytes_hex
            );
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "canary")
                    .unwrap()
                    .bytes_hex,
                "a5"
            );
        }

        let invalid = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m32fp_invalid_status.case.json"
        ))
        .expect("S2-31ak binary32 invalid-status fixture schema");
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&invalid.fixture).unwrap();
        let result = execute_fixture_v2(&invalid, &mut memory);
        result.validate_for(&invalid).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("invalid-status rejection must preserve an observable state");
        };
        assert_eq!(value.state, invalid.fixture.initial_state);
        let stack = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(stack.bytes_hex, invalid.fixture.memory[1].initial_bytes_hex);
        let canary = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "canary")
            .unwrap();
        assert_eq!(canary.bytes_hex, "a5");

        let invalid_json: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m32fp_invalid_status.case.json"
        ))
        .unwrap();
        for source in ["0000803f", "0100807f", "0000c07f", "ffffbf7f"] {
            let mut source_class_irrelevant = invalid_json.clone();
            source_class_irrelevant["memory"][1]["initial_bytes_hex"] =
                serde_json::Value::String(source.into());
            v2::Fixture::from_json(&serde_json::to_vec(&source_class_irrelevant).unwrap())
                .unwrap_or_else(|error| panic!("environment gate should admit {source}: {error}"));
        }
        let mut ue_only_unsupported = invalid_json.clone();
        ue_only_unsupported["initial_state"]["x87"]["status_word"] =
            serde_json::Value::String("0x0000000000000010".into());
        assert!(
            v2::Fixture::from_json(&serde_json::to_vec(&ue_only_unsupported).unwrap()).is_err(),
            "the D900 schema must not admit a UE-only environment-rejected fixture"
        );
        let mut clean_snan = invalid_json;
        clean_snan["initial_state"]["x87"]["status_word"] =
            serde_json::Value::String("0x0000000000004500".into());
        clean_snan["memory"][1]["initial_bytes_hex"] = serde_json::Value::String("0100807f".into());
        assert!(v2::Fixture::from_json(&serde_json::to_vec(&clean_snan).unwrap()).is_err());
    }

    #[test]
    fn fld_m64fp_literal_fixtures_expand_binary64_and_reject_bad_class() {
        for (bytes, retired, raw, expected_tag_word, expected_status) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp.case.json")
                    .as_slice(),
                1,
                "00000000000000000000",
                0x7ffc,
                0x7d00,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp_fnstsw.case.json"
                )
                .as_slice(),
                2,
                "00000000000000000000",
                0x7ffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ae_fld_m64fp_infinity.case.json")
                    .as_slice(),
                1,
                "0000000000000080ff7f",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ae_fld_m64fp_negative_infinity_fnstsw.case.json")
                    .as_slice(),
                2,
                "0000000000000080ffff",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_qnan.case.json")
                    .as_slice(),
                1,
                "00000000000000c0ff7f",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_negative_qnan_payload_fnstsw.case.json")
                    .as_slice(),
                2,
                "00e0d5c4b3a291c0ffff",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_snan.case.json")
                    .as_slice(),
                1,
                "00080000000000c0ff7f",
                0xbffc,
                0x7d01,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_negative_snan_payload_fnstsw.case.json")
                    .as_slice(),
                2,
                "00f8ffffffffffffffff",
                0xbffc,
                0x7d01,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31ae fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31u fixture must produce observable state")
            };
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(
                value.state.gpr[0],
                HexU64(if retired == 1 {
                    0x0030_0004
                } else {
                    0x0030_0000 | expected_status
                })
            );
            assert_eq!(after.status_word, HexU64(expected_status));
            assert_eq!(after.tag_word, HexU64(expected_tag_word));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(after.registers[7], raw);
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(stack.bytes_hex, fixture.fixture.memory[1].initial_bytes_hex);
            let canary = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "canary")
                .unwrap();
            assert_eq!(canary.bytes_hex, "a5");
        }
        let invalid = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m64fp_invalid_status.case.json"
        ))
        .expect("invalid-status fixture schema");
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&invalid.fixture).unwrap();
        let result = execute_fixture_v2(&invalid, &mut memory);
        result.validate_for(&invalid).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("invalid-status rejection must preserve an observable state")
        };
        assert_eq!(value.state, invalid.fixture.initial_state);
        let stack = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(stack.bytes_hex, invalid.fixture.memory[1].initial_bytes_hex);
        let mut clean_environment = invalid.fixture.clone();
        clean_environment
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(0x4500);
        assert!(v2::Fixture::from_json(&serde_json::to_vec(&clean_environment).unwrap()).is_err());
        let mut ue_only_unsupported: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m64fp_invalid_status.case.json"
        ))
        .unwrap();
        ue_only_unsupported["initial_state"]["x87"]["status_word"] =
            serde_json::Value::String("0x0000000000000010".into());
        assert!(
            v2::Fixture::from_json(&serde_json::to_vec(&ue_only_unsupported).unwrap()).is_err(),
            "the DD00 schema must not admit a UE-only environment-rejected fixture"
        );
        let template: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp.case.json"
        ))
        .unwrap();
        for bytes in ["dd01", "dd08", "48dd00", "9bdd00", "67dd00", "dd00dfe1"] {
            let mut nearby = template.clone();
            nearby["memory"][0]["initial_bytes_hex"] = serde_json::Value::String(bytes.into());
            nearby["memory"][0]["length"] = serde_json::Value::from(bytes.len() / 2);
            nearby["memory"][0]["comparison_mask_hex"] =
                serde_json::Value::String("ff".repeat(bytes.len() / 2));
            nearby["stop_rip"] = serde_json::Value::String(format!(
                "0x{:016x}",
                0x0010_0000 + (bytes.len() / 2) as u64
            ));
            assert!(
                v2::Fixture::from_json(&serde_json::to_vec(&nearby).unwrap()).is_err(),
                "S2-31t schema must reject {bytes}"
            );
        }
        let mut rejected_subnormal: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m64fp_invalid_status.case.json"
        ))
        .unwrap();
        rejected_subnormal["initial_state"]["x87"]["status_word"] =
            serde_json::Value::String("0x0000000000004500".into());
        rejected_subnormal["memory"][1]["initial_bytes_hex"] =
            serde_json::Value::String("0100000000000000".into());
        assert!(v2::Fixture::from_json(&serde_json::to_vec(&rejected_subnormal).unwrap()).is_err());
    }

    #[test]
    fn fld_m80fp_literal_fixtures_load_raw_binary80_and_reject_bad_class() {
        for (bytes, expected_rax, retired, expected_raw, expected_tag_word) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "00000000000000000000",
                0x7ffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "00000000000000000080",
                0x7ffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31x_fld_m80fp_subnormal.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "01000000000000000000",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31x_fld_m80fp_largest_negative_subnormal_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "ffffffffffffff7f0080",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31aa_fld_m80fp_infinity.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "0000000000000080ff7f",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31aa_fld_m80fp_negative_infinity_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "0000000000000080ffff",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31ab_fld_m80fp_qnan.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "35120000000000c0ff7f",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31ab_fld_m80fp_indefinite_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "00000000000000c0ffff",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ac_fld_m80fp_snan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "0100000000000080ff7f",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ac_fld_m80fp_snan_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "ffffffffffffffbfffff",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ad_fld_m80fp_pseudo_denormal.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "00000000000000800000",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ad_fld_m80fp_negative_max_pseudo_denormal_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "ffffffffffffffff0080",
                0xbffc,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("FLD m80fp fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
            panic!("FLD m80fp fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x7d00));
            assert_eq!(after.tag_word, HexU64(expected_tag_word));
            assert_eq!(
                (after.tag_word.get() >> 14) & 3,
                if expected_tag_word == 0xbffc { 0b10 } else { 0b01 }
            );
            assert_eq!(after.status_word.get() & 0x0012, 0, "no new DE/UE");
            assert_eq!(after.status_word.get() & 0x0001, 0, "no IE");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(after.registers[7], expected_raw);
            for physical in 0..7 {
                assert_eq!(
                    after.registers[physical],
                    fixture
                        .fixture
                        .initial_state
                        .x87
                        .as_ref()
                        .unwrap()
                        .registers[physical]
                );
            }
            let regions = memory.snapshot();
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "stack")
                    .unwrap()
                    .bytes_hex,
                expected_raw
            );
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "canary")
                    .unwrap()
                    .bytes_hex,
                "a5"
            );
        }

        let invalid = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp_invalid_class.case.json"
        ))
        .expect("invalid-status fixture schema");
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&invalid.fixture).unwrap();
        let result = execute_fixture_v2(&invalid, &mut memory);
        result.validate_for(&invalid).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("invalid-status rejection must preserve an observable state");
        };
        assert_eq!(value.state, invalid.fixture.initial_state);
        let stack = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(stack.bytes_hex, invalid.fixture.memory[1].initial_bytes_hex);
        let canary = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "canary")
            .unwrap();
        assert_eq!(canary.bytes_hex, "a5");

        let template: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp_invalid_class.case.json"
        ))
        .unwrap();
        for (raw, should_be_admitted) in [
            ("01000000000000000000", false),
            ("01000000000000000080", false),
            ("00000000000000000000", false),
            ("00000000000000000080", false),
            ("00000000000000800000", false),
            ("00000000000000800080", false),
            ("ffffffffffffff7f0000", false),
            ("0000000000000080003f", false),
            ("0000000000000080ff7f", false),
            ("0000000000000080ffff", false),
            ("00000000000000c0ff7f", false),
            ("00000000000000c0ffff", false),
            ("0100000000000080ffff", false),
            ("0100000000000080ff7f", false),
            ("0100000000000040ff7f", false),
            ("0100000000000040ffff", false),
            ("5555555555555595ff7f", false),
            ("5555555555555595ffff", false),
            ("ffffffffffffffbfff7f", false),
            ("ffffffffffffffbfffff", false),
            ("0100000000000000ff7f", false),
            ("0100000000000000ffff", false),
            ("0000000000000000ff7f", true),
            ("0000000000000000ffff", false),
        ] {
            let mut nearby = template.clone();
            nearby["memory"][1]["initial_bytes_hex"] = serde_json::Value::String(raw.into());
            let parsed = v2::Fixture::from_json(&serde_json::to_vec(&nearby).unwrap());
            assert_eq!(
                parsed.is_ok(),
                should_be_admitted,
                "S2-31ac schema admission for raw binary80 {raw}"
            );
        }
    }

    #[test]
    fn fst_m32fp_signed_zero_fixtures_preserve_or_pop_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_zero) in [
            (
                "positive FST zero",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_positive_zero.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffd,
                "00000000000000000000",
            ),
            (
                "negative FST zero with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_negative_zero_fnstsw.case.json").as_slice(),
                0x0030_4500,
                0x4500,
                0xfffd,
                "00000000000000000080",
            ),
            (
                "positive FSTP zero",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_positive_zero.case.json").as_slice(),
                0x0030_0004,
                0x4d00,
                0xffff,
                "00000000000000000000",
            ),
            (
                "negative FSTP zero with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_negative_zero_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                "00000000000000000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(fixture.fixture.instruction_budget) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_zero, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory.snapshot().into_iter().find(|region| region.region_id == "stack").unwrap();
            let expected_memory = if raw_zero.ends_with("80") {
                "00000080"
            } else {
                "00000000"
            };
            assert_eq!(&stack.bytes_hex[8..16], expected_memory, "{name}");
        }
    }

    #[test]
    fn fst_m32fp_signed_infinity_fixtures_store_exact_bytes_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_infinity, stored) in [
            (
                "positive FST m32 infinity",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aq_fst_m32fp_positive_infinity.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffe,
                "0000000000000080ff7f",
                "0000807f",
            ),
            (
                "negative FSTP m32 infinity with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aq_fstp_m32fp_negative_infinity_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                "0000000000000080ffff",
                "000080ff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_infinity, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..16], stored, "{name}");
        }
    }

    #[test]
    fn fld_fst_m32fp_maximum_finite_fixtures_preserve_exponent_127() {
        for (
            name,
            bytes,
            expected_rax,
            expected_status,
            expected_tag_word,
            expected_register_index,
            expected_register,
            expected_fip,
            expected_memory_offset,
            expected_output,
        ) in [
            (
                "FLD positive maximum finite",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bb_fld_m32fp_positive_max_finite.case.json").as_slice(),
                0x0030_0004,
                0x7d00,
                0x3ffc,
                7,
                "0000000000ffffff7e40",
                0x0010_0000,
                0,
                "ffff7f7f",
            ),
            (
                "FST positive maximum finite",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bb_fst_m32fp_positive_max_finite.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffc,
                0,
                "0000000000ffffff7e40",
                0x0010_0000,
                8,
                "ffff7f7f",
            ),
            (
                "FSTP negative maximum finite with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bb_fstp_m32fp_negative_max_finite_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                0,
                "0000000000ffffff7ec0",
                0x0010_0000,
                8,
                "ffff7fff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[expected_register_index], expected_register, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(expected_fip), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(
                &stack.bytes_hex[expected_memory_offset..expected_memory_offset + 8],
                expected_output,
                "{name}"
            );
        }
    }

    #[test]
    fn fld_fst_m64fp_maximum_finite_fixtures_preserve_exponent_1023() {
        for (
            name,
            bytes,
            expected_rax,
            expected_status,
            expected_tag_word,
            register_index,
            expected_register,
            expected_memory,
        ) in [
            (
                "FLD positive maximum finite",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bc_fld_m64fp_positive_max_finite.case.json").as_slice(),
                0x0030_0004,
                0x7d00,
                0x3ffc,
                7,
                "00f8fffffffffffffe43",
                None,
            ),
            (
                "FST positive maximum finite",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bc_fst_m64fp_positive_max_finite.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffc,
                0,
                "00f8fffffffffffffe43",
                Some("ffffffffffffef7f"),
            ),
            (
                "FSTP negative maximum finite with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bc_fstp_m64fp_negative_max_finite_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                0,
                "00f8fffffffffffffec3",
                Some("ffffffffffffefff"),
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[register_index], expected_register, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            if let Some(expected_memory) = expected_memory {
                let stack = memory
                    .snapshot()
                    .into_iter()
                    .find(|region| region.region_id == "stack")
                    .unwrap();
                assert_eq!(&stack.bytes_hex[8..24], expected_memory, "{name}");
            }
        }
    }

    #[test]
    fn fst_m32fp_exact_subnormal_fixtures_preserve_de_and_observe_fnstsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive minimum subnormal",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bd_fst_m32fp_positive_min_subnormal.case.json").as_slice(),
                0x0030_0004,
                0x4502,
                0xfffc,
                "00000000000000806a3f",
                "01000000",
            ),
            (
                "FSTP negative minimum subnormal with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bd_fstp_m32fp_negative_min_subnormal_fnstsw.case.json").as_slice(),
                0x0030_4d02,
                0x4d02,
                0xffff,
                "00000000000000806abf",
                "01000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}: {:?}", result.outcome);
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..16], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_exact_subnormal_fixtures_preserve_de_and_observe_fnstsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive minimum binary64 subnormal",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31be_fst_m64fp_positive_min_subnormal.case.json").as_slice(),
                0x0030_0004,
                0x4502,
                0xfffc,
                "0000000000000080cd3b",
                "0100000000000000",
            ),
            (
                "FSTP negative minimum binary64 subnormal with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31be_fstp_m64fp_negative_min_subnormal_fnstsw.case.json").as_slice(),
                0x0030_4d02,
                0x4d02,
                0xffff,
                "0000000000000080cdbb",
                "0100000000000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}: {:?}", result.outcome);
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory.snapshot().into_iter().find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[8..24], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_inexact_subnormal_fixtures_observe_ue_pe_and_fnstsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive 1.5 minimum binary64 subnormal",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bg_fst_m64fp_positive_1_5_minsub.case.json").as_slice(),
                0x0030_0004,
                0x4732,
                0xfffc,
                "00000000000000c0cd3b",
                "0200000000000000",
            ),
            (
                "FST positive 0.75 minimum binary64 subnormal",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bk_fst_m64fp_positive_0_75_minsub_fnstsw.case.json").as_slice(),
                0x0030_4732,
                0x4732,
                0xfffc,
                "00000000000000c0cc3b",
                "0100000000000000",
            ),
            (
                "FSTP negative 1.5 minimum binary64 subnormal with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bg_fstp_m64fp_negative_1_5_minsub_fnstsw.case.json").as_slice(),
                0x0030_4f32,
                0x4f32,
                0xffff,
                "00000000000000c0cdbb",
                "0200000000000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}: {:?}", result.outcome);
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory.snapshot().into_iter().find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[8..24], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_binary80_subnormal_fixtures_observe_underflow_and_preserve_raw_source() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive binary80 subnormal to minimum binary64 subnormal",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bm_fst_m64fp_positive_min_subnormal_fnstsw.case.json").as_slice(),
                0x0030_4730,
                0x4730,
                0xfffe,
                "01000000000000000000",
                "0100000000000000",
            ),
            (
                "FSTP negative binary80 subnormal to signed zero",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bm_fstp_m64fp_negative_signed_zero_fnstsw.case.json").as_slice(),
                0x0030_4d30,
                0x4d30,
                0xffff,
                "01000000000000000080",
                "0000000000000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}: FCW={:04x} FSW={:04x}", value.state.x87.as_ref().unwrap().control_word.0, value.state.x87.as_ref().unwrap().status_word.0);
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory.snapshot().into_iter().find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[8..24], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m32fp_inexact_subnormal_fixtures_observe_ue_pe_with_fnstsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive 1.5 minimum subnormal with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bf_fst_m32fp_positive_1_5_minsub_fnstsw.case.json").as_slice(),
                0x0030_4732,
                0x4732,
                0xfffc,
                "00000000000000c06a3f",
                "02000000",
            ),
            (
                "FST positive 0.75 minimum subnormal with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bj_fst_m32fp_positive_0_75_minsub_fnstsw.case.json").as_slice(),
                0x0030_4732,
                0x4732,
                0xfffc,
                "00000000000000c0693f",
                "01000000",
            ),
            (
                "FST negative binary80 subnormal to minimum binary32 subnormal with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bl_fst_m32fp_negative_min_subnormal_fnstsw.case.json").as_slice(),
                0x0030_4730,
                0x4730,
                0xfffe,
                "01000000000000000080",
                "01000080",
            ),
            (
                "FST with pre-existing UE and FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bh_fst_m32fp_sticky_ue_fnstsw.case.json").as_slice(),
                0x0030_0230,
                0x0230,
                0xfffc,
                "00000000000000c06a3f",
                "02000000",
            ),
            (
                "FSTP negative 1.5 minimum subnormal with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bf_fstp_m32fp_negative_1_5_minsub_fnstsw.case.json").as_slice(),
                0x0030_4f32,
                0x4f32,
                0xffff,
                "00000000000000c06abf",
                "02000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}: {:?}", result.outcome);
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory.snapshot().into_iter().find(|region| region.region_id == "stack").unwrap();
            assert_eq!(&stack.bytes_hex[8..16], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m32fp_binary80_exponent_minus_127_fixtures_observe_minimum_normal_flags() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive binary80 exponent -127 at nearest minimum-normal threshold",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bn_fst_m32fp_positive_minimum_normal_nearest_tie_fnstsw.case.json").as_slice(),
                0x0030_4730,
                0x4730,
                0xfffc,
                "0000000000ffffff803f",
                "00008000",
            ),
            (
                "FSTP negative binary80 exponent -127 toward negative with no new UE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bn_fstp_m32fp_negative_minimum_normal_directed_fnstsw.case.json").as_slice(),
                0x0030_4f20,
                0x4f20,
                0xffff,
                "0000000080ffffff80bf",
                "00008080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}: {:?}",
                result.outcome
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..16], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_binary80_exponent_minus_1023_fixtures_observe_minimum_normal_flags() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw) in [
            (
                "FST positive binary80 exponent -1023 at nearest minimum-normal threshold",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bo_fst_m64fp_positive_minimum_normal_nearest_tie_fnstsw.case.json").as_slice(),
                0x0030_4730,
                0x4730,
                0xfffc,
                "00f8ffffffffffff003c",
            ),
            (
                "FSTP negative binary80 exponent -1023 toward negative with no new UE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bo_fstp_m64fp_negative_minimum_normal_directed_fnstsw.case.json").as_slice(),
                0x0030_4f20,
                0x4f20,
                0xffff,
                "ffffffffffffffff00bc",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}: {:?}",
                result.outcome
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], if expected_tag == 0xffff { "0000000000001080" } else { "0000000000001000" }, "{name}");
        }
    }

    #[test]
    fn fst_m32fp_binary80_exponent_127_overflow_fixtures_observe_oe_pe_c1() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive exponent-127 binary80 midpoint rounds to infinity",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bp_fst_m32fp_positive_infinity_overflow_nearest_fnstsw.case.json").as_slice(),
                0x0030_4728,
                0x4728,
                0xfffc,
                "0000000080ffffff7e40",
                "0000807f",
            ),
            (
                "FSTP negative exponent-127 binary80 toward-negative overflow",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bp_fstp_m32fp_negative_infinity_overflow_directed_fnstsw.case.json").as_slice(),
                0x0030_4f28,
                0x4f28,
                0xffff,
                "ffffffffffffffff7ec0",
                "000080ff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}: {:?}",
                result.outcome
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..16], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_binary80_exponent_1023_overflow_fixtures_observe_oe_pe_c1() {
        for (name, bytes, expected_rax, expected_status, expected_tag, expected_raw, expected_bits) in [
            (
                "FST positive exponent-1023 binary80 midpoint rounds to infinity",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bq_fst_m64fp_positive_infinity_overflow_nearest_fnstsw.case.json").as_slice(),
                0x0030_4728,
                0x4728,
                0xfffc,
                "00fcfffffffffffffe43",
                "000000000000f07f",
            ),
            (
                "FSTP negative exponent-1023 binary80 toward-negative overflow",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31bq_fstp_m64fp_negative_infinity_overflow_directed_fnstsw.case.json").as_slice(),
                0x0030_4f28,
                0x4f28,
                0xffff,
                "00fcfffffffffffffec3",
                "000000000000f0ff",
            ),
            (
                "exact FST remains admitted with OE already sticky",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31br_m64fp_sticky_oe_exact_store_fnstsw.case.json").as_slice(),
                0x0030_4508,
                0x4508,
                0xfffc,
                "0000000000000080ff3f",
                "000000000000f03f",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}: {:?}",
                result.outcome
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag), "{name}");
            assert_eq!(x87.registers[0], expected_raw, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], expected_bits, "{name}");
        }
    }

    #[test]
    fn fst_m32fp_quiet_nan_fixtures_preserve_payload_conversion_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_nan, stored) in [
            (
                "positive FST m32 quiet NaN",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31av_fst_m32fp_positive_qnan.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffe,
                "00000000000000c0ff7f",
                "0000c07f",
            ),
            (
                "negative FSTP m32 quiet NaN with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31av_fstp_m32fp_negative_qnan_payload_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                "f0debc9a785634d2ffff",
                "5634d2ff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_nan, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..16], stored, "{name}");
        }
    }

    #[test]
    fn fst_m32fp_signaling_nan_fixtures_quiet_payload_set_ie_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_nan, stored) in [
            (
                "positive FST m32 signaling NaN",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ax_fst_m32fp_positive_snan.case.json").as_slice(),
                0x0030_0004,
                0x4501,
                0xfffe,
                "0100000000000080ff7f",
                "0000c07f",
            ),
            (
                "negative FSTP m32 signaling NaN with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ax_fstp_m32fp_negative_snan_payload_fnstsw.case.json").as_slice(),
                0x0030_4d01,
                0x4d01,
                0xffff,
                "f0debc9a78563492ffff",
                "5634d2ff",
            ),
            (
                "positive FST m32 signaling NaN with pre-existing IE",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ay_fst_m32fp_positive_snan_sticky_ie.case.json").as_slice(),
                0x0030_0004,
                0x4501,
                0xfffe,
                "0100000000000080ff7f",
                "0000c07f",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_nan, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..16], stored, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_signed_zero_fixtures_preserve_or_pop_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_zero) in [
            (
                "positive FST m64 zero",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fst_m64fp_positive_zero.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffd,
                "00000000000000000000",
            ),
            (
                "negative FST m64 zero with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fst_m64fp_negative_zero_fnstsw.case.json").as_slice(),
                0x0030_4500,
                0x4500,
                0xfffd,
                "00000000000000000080",
            ),
            (
                "positive FSTP m64 zero",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fstp_m64fp_positive_zero.case.json").as_slice(),
                0x0030_0004,
                0x4d00,
                0xffff,
                "00000000000000000000",
            ),
            (
                "negative FSTP m64 zero with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31am_fstp_m64fp_negative_zero_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                "00000000000000000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(fixture.fixture.instruction_budget) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_zero, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(
                &stack.bytes_hex[8..24],
                if raw_zero.ends_with("80") {
                    "0000000000000080"
                } else {
                    "0000000000000000"
                },
                "{name}"
            );
        }
    }

    #[test]
    fn fst_m64fp_signed_infinity_fixtures_store_exact_bytes_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_infinity, stored) in [
            (
                "positive FST m64 infinity",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ap_fst_m64fp_positive_infinity.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffe,
                "0000000000000080ff7f",
                "000000000000f07f",
            ),
            (
                "negative FSTP m64 infinity with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ap_fstp_m64fp_negative_infinity_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                "0000000000000080ffff",
                "000000000000f0ff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_infinity, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], stored, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_quiet_nan_fixtures_preserve_payload_conversion_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_nan, stored) in [
            (
                "positive FST m64 quiet NaN",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31au_fst_m64fp_positive_quiet_nan.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffe,
                "00000000000000c0ff7f",
                "000000000000f87f",
            ),
            (
                "negative FSTP m64 quiet NaN with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31au_fstp_m64fp_negative_quiet_nan_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                "ffffffffffffffffffff",
                "ffffffffffffffff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_nan, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], stored, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_signaling_nan_fixtures_quiet_payload_and_observe_ie() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_nan, stored) in [
            (
                "positive FST m64 signaling NaN",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aw_fst_m64fp_positive_signaling_nan.case.json").as_slice(),
                0x0030_0004,
                0x4501,
                0xfffe,
                "0100000000000080ff7f",
                "000000000000f87f",
            ),
            (
                "negative FSTP m64 signaling NaN with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aw_fstp_m64fp_negative_signaling_nan_fnstsw.case.json").as_slice(),
                0x0030_4d01,
                0x4d01,
                0xffff,
                "f0debc9a78563492ffff",
                "9b5713cf8a46faff",
            ),
            (
                "negative FSTP m64 signaling NaN with pre-existing IE and FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ay_fstp_m64fp_negative_snan_sticky_ie_fnstsw.case.json").as_slice(),
                0x0030_4d01,
                0x4d01,
                0xffff,
                "f0debc9a78563492ffff",
                "9b5713cf8a46faff",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_nan, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], stored, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_literal_fixtures_store_without_pop_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31m_fst_m64fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31m_fst_m64fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4500,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31m fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31m fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4500));
            assert_eq!(after.tag_word, HexU64(0xfffc));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(
                after.registers,
                fixture
                    .fixture
                    .initial_state
                    .x87
                    .as_ref()
                    .unwrap()
                    .registers
            );
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], "000000000000f83f");
        }
    }

    #[test]
    fn fild_m32_literal_fixtures_push_to_st7_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31n_fild_m32int_success.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0001,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31n_fild_m32int_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31n fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31n fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x7d00));
            assert_eq!(after.tag_word, HexU64(0x3ffc));
            assert_eq!(after.registers[7], "0000000000000080ff3f");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0001));
            let memory_after = memory.snapshot();
            let source_left = memory_after
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap();
            assert_eq!(source_left.bytes_hex, "01");
        }
    }

    #[test]
    fn fistp_m16int_literal_fixtures_cover_rounding_edges_and_rejections() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let successful = [
            ("rc0_positive", 2_i16, true, true),
            ("rc0_negative", -2, true, true),
            ("rc1_positive", 1, true, false),
            ("rc1_negative", -2, true, true),
            ("rc2_positive", 2, true, true),
            ("rc2_negative", -1, true, false),
            ("rc3_positive", 1, true, false),
            ("rc3_negative", -1, true, false),
            ("fnstsw_tie", -2, true, false),
            ("exact_max", i16::MAX, false, false),
            ("exact_min", i16::MIN, false, false),
            ("rounded_max", 32767, true, false),
            ("rounded_min", i16::MIN, true, false),
        ];
        for (suffix, expected, inexact, incremented) in successful {
            let name = format!("s2_31d_fistp_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(if suffix == "fnstsw_tie" { 2 } else { 1 })
                },
                "{name}"
            );
            let left = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_left")
                .unwrap();
            let right = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            let bytes = expected.to_le_bytes();
            assert_eq!(left.bytes_hex, format!("{:02x}", bytes[0]), "{name}");
            assert_eq!(
                right.bytes_hex,
                format!("{:02x}{}", bytes[1], "ccddeeff11223344556677889900"),
                "{name}"
            );
            assert_ne!(memory.snapshot(), initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let before_x87 = before.x87.as_ref().unwrap();
            let after = value.state.x87.as_ref().unwrap();
            let expected_fsw = (before_x87.status_word.get() & !(7 << 11 | 1 << 9))
                | (1 << 11)
                | if inexact { 0x20 } else { 0 }
                | if incremented { 0x200 } else { 0 };
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word.get() & 3, 3, "{name}");
            assert_eq!(after.registers, before_x87.registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x100000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x300001), "{name}");
            let mut expected_state = before;
            expected_state.rip = HexU64(if suffix == "fnstsw_tie" {
                0x100004
            } else {
                0x100002
            });
            let expected_x87 = expected_state.x87.as_mut().unwrap();
            expected_x87.status_word = HexU64(expected_fsw);
            expected_x87.tag_word = HexU64((expected_x87.tag_word.get() & !3) | 3);
            expected_x87.instruction_pointer = HexU64(0x100000);
            expected_x87.data_pointer = HexU64(0x300001);
            if suffix == "fnstsw_tie" {
                expected_state.gpr[0] =
                    HexU64((expected_state.gpr[0].get() & !0xffff) | u64::from(expected_fsw));
            }
            assert_eq!(value.state, expected_state, "{name}: full CPU state");
        }

        for suffix in ["invalid_tag", "invalid_status", "invalid_control"] {
            let name = format!("s2_31d_fistp_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::Unsupported { .. }),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) },
                "{name}"
            );
            assert_eq!(memory.snapshot(), initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            assert_eq!(value.state, fixture.fixture.initial_state, "{name}");
        }
    }

    #[test]
    fn fist_m16int_literal_fixtures_cover_no_pop_fnstsw_and_rejections() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let successful = [
            ("rc0_positive", 2_i16, true, true),
            ("fnstsw_tie", -2, true, false),
            ("exact_max", i16::MAX, false, false),
            ("exact_min", i16::MIN, false, false),
        ];
        for (suffix, expected, inexact, incremented) in successful {
            let name = format!("s2_31e_fist_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            let instruction_count = if suffix == "fnstsw_tie" { 2 } else { 1 };
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(instruction_count)
                },
                "{name}"
            );
            let regions = memory.snapshot();
            let left = regions.iter().find(|r| r.region_id == "data_left").unwrap();
            let right = regions
                .iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            let bytes = expected.to_le_bytes();
            assert_eq!(left.bytes_hex, format!("{:02x}", bytes[0]), "{name}");
            assert_eq!(
                right.bytes_hex,
                format!("{:02x}{}", bytes[1], "ccddeeff11223344556677889900"),
                "{name}"
            );
            assert_ne!(regions, initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let before_x87 = before.x87.as_ref().unwrap();
            let after = value.state.x87.as_ref().unwrap();
            let expected_fsw = (before_x87.status_word.get() & !(1 << 9))
                | if inexact { 0x20 } else { 0 }
                | if incremented { 0x200 } else { 0 };
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word, before_x87.tag_word, "{name}");
            assert_eq!(after.registers, before_x87.registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x100000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x300001), "{name}");
            let mut expected_state = before;
            expected_state.rip = HexU64(0x100000 + 2 * instruction_count);
            let expected_x87 = expected_state.x87.as_mut().unwrap();
            expected_x87.status_word = HexU64(expected_fsw);
            expected_x87.instruction_pointer = HexU64(0x100000);
            expected_x87.data_pointer = HexU64(0x300001);
            if suffix == "fnstsw_tie" {
                expected_state.gpr[0] =
                    HexU64((expected_state.gpr[0].get() & !0xffff) | u64::from(expected_fsw));
            }
            assert_eq!(value.state, expected_state, "{name}: complete CPU state");
        }

        for suffix in ["invalid_tag", "invalid_status", "invalid_control"] {
            let name = format!("s2_31e_fist_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::Unsupported { .. }),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) },
                "{name}"
            );
            assert_eq!(memory.snapshot(), initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            assert_eq!(value.state, fixture.fixture.initial_state, "{name}");
        }
    }

    #[test]
    fn fistp_m16int_signed_zero_fixture_stores_word_zero_and_observes_status() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bu_fistp_m16int_negative_zero_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31bu fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31bu fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d20));
        assert_eq!(after.status_word, HexU64(0x4d20));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "00ccddeeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_signed_zero_fixture_stores_qword_zero_and_observes_status() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bv_fistp_m64int_negative_zero_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31bv fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31bv fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d20));
        assert_eq!(after.status_word, HexU64(0x4d20));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000000000000003344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_quiet_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ce_fistp_m64int_negative_qnan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ce fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ce fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d21));
        assert_eq!(after.status_word, HexU64(0x4d21));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000000000000803344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_signaling_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ch_fistp_m64int_negative_snan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ch fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ch fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d21));
        assert_eq!(after.status_word, HexU64(0x4d21));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000000000000803344556677889900"
        );
    }

    #[test]
    fn fist_m32int_signaling_nan_fixture_observes_ie_with_fnstsw() {
        let bytes =
            include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ci_fist_snan_fnstsw.case.json");
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ci FIST fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ci FIST fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4501));
        assert_eq!(after.status_word, HexU64(0x4501));
        assert_eq!(after.tag_word, HexU64(0xfffe));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m32int_exact_fixture_preserves_incoming_ie_without_pe() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ci_fistp_exact_ie_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ci exact-store fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ci exact-store fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d01));
        assert_eq!(after.status_word, HexU64(0x4d01));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "01"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000000eeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m32int_positive_overflow_fixture_stores_indefinite_and_observes_ie() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cj_fistp_m32int_positive_overflow_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cj overflow fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cj overflow fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d01));
        assert_eq!(after.status_word, HexU64(0x4d01));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900"
        );
    }

    #[test]
    fn fist_m32int_positive_roundback_fixture_preserves_in_range_result_and_observes_pe() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cj_fist_m32int_positive_roundback_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cj roundback fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cj roundback fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4520));
        assert_eq!(after.status_word, HexU64(0x4520));
        assert_eq!(after.tag_word, HexU64(0xfffc));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "ff"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "ffff7feeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_infinity_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cb_fistp_m64int_negative_infinity_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cb fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cb fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4d21));
        assert_eq!(after.status_word, HexU64(0x4d21));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000000000000803344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_negative_fraction_fixture_rounds_and_observes_status() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31by_fistp_m64int_negative_fraction_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31by fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31by fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4f20));
        assert_eq!(after.status_word, HexU64(0x4f20));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "ff"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "ffffffffffffff3344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_subnormal_fixture_loads_stores_observes_and_clears_status() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cs_fld_fistp_m64int_negative_subnormal_fnstsw_fnclex.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cs fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31cs code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(5) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cs fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0220));
        assert_eq!(after.status_word, HexU64(0x0200));
        assert_eq!(after.tag_word, initial_x87.tag_word);
        assert_eq!((after.status_word.get() >> 11) & 7, 0);
        assert_eq!(after.registers[7], "ffffffffffffff7f0080");
        assert_ne!(after.registers[7], initial_x87.registers[7]);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_000c));
        assert_eq!(after.data_pointer, HexU64(0x0000_4001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "qword_output")
                .unwrap()
                .bytes_hex,
            "a5ffffffffffffffffa5a5a5a5a5a5a5"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "m80_input")
                .unwrap()
                .bytes_hex,
            "ffffffffffffff7f0080"
        );
    }

    #[test]
    fn fistp_m32int_negative_fraction_fixture_rounds_and_observes_status() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bw_fistp_m32int_negative_fraction_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31bw fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31bw fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4f20));
        assert_eq!(after.status_word, HexU64(0x4f20));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "ff"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "ffffffeeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m32int_infinity_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bz_fistp_m32int_negative_infinity_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31bz fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31bz fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m32int_pseudo_infinity_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ct_fistp_m32int_negative_pseudo_infinity_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ct fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31ct code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ct fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m16int_pseudo_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cx_fistp_m16int_negative_pseudo_nan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cx fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31cx code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cx fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "aa"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "0080ddeeff11223344556677889900aa"
        );
    }

    #[test]
    fn fistp_m32int_pseudo_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cw_fistp_m32int_negative_pseudo_nan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cw fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31cw code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cw fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m32int_unnormal_fixture_stores_indefinite_and_sets_ie() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cz_fistp_m32int_negative_unnormal.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cz fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31cz code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let initial = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cz fixture snapshot unavailable");
        };
        let initial_x87 = initial.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_0001));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900aa"
        );
    }

    #[test]
    fn fistp_m16int_pseudo_infinity_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cu_fistp_m16int_negative_pseudo_infinity_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cu fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31cu code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cu fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "80ccddeeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_pseudo_infinity_fixture_observes_ie_then_clears_it() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cv_fistp_m64int_negative_pseudo_infinity_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cv fixture schema");
        let code_hex = &fixture
            .fixture
            .memory
            .iter()
            .find(|region| region.region_id == "code")
            .expect("S2-31cv code region")
            .initial_bytes_hex;
        let code = code_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "bounded x87 remains JIT-denied"
        );
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(3) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cv fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4900));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000000000000803344556677889900"
        );
    }

    #[test]
    fn fistp_m32int_quiet_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cc_fistp_m32int_negative_qnan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cc fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cc fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m32int_signaling_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cf_fistp_m32int_negative_snan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cf fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cf fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "000080eeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m16int_quiet_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cd_fistp_m16int_negative_qnan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cd fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cd fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "80ccddeeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m16int_signaling_nan_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31cg_fistp_m16int_negative_snan_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31cg fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31cg fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "00"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "80ccddeeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m16int_infinity_fixture_observes_ie_with_fnstsw() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ca_fistp_m16int_negative_infinity_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31ca fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31ca fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4901));
        assert_eq!(after.status_word, HexU64(0x4901));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "0080"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "bbccddeeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m16int_negative_fraction_fixture_rounds_and_observes_status() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31bx_fistp_m16int_negative_fraction_fnstsw.case.json"
        );
        let fixture = v2::Fixture::from_json(bytes).expect("S2-31bx fixture schema");
        let before = fixture.fixture.initial_state.clone();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("S2-31bx fixture snapshot unavailable");
        };
        let initial_x87 = before.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(value.state.gpr[0], HexU64(0x0030_4f20));
        assert_eq!(after.status_word, HexU64(0x4f20));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, initial_x87.registers);
        assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
        assert_eq!(after.data_pointer, HexU64(0x0030_0001));
        let regions = memory.snapshot();
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap()
                .bytes_hex,
            "ff"
        );
        assert_eq!(
            regions
                .iter()
                .find(|region| region.region_id == "data_right")
                .unwrap()
                .bytes_hex,
            "ffccddeeff11223344556677889900"
        );
    }

    #[test]
    fn fistp_m64int_literal_fixtures_cover_qword_boundaries_and_overflow() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for (suffix, expected, count, rax_after, status_word) in [
            (
                "exact_max",
                0x7fff_ffff_ffff_ffff_u64,
                1_u64,
                0x0000_0000_0030_0001_u64,
                0x4d00_u64,
            ),
            (
                "negative_min_fnstsw",
                0x8000_0000_0000_0000_u64,
                2_u64,
                0x0000_0000_0030_4d00_u64,
                0x4d00_u64,
            ),
            (
                "positive_overflow",
                0x8000_0000_0000_0000_u64,
                2_u64,
                0x0000_0000_0030_4d01_u64,
                0x4d01_u64,
            ),
        ] {
            let name = format!("s2_31f_fistp_m64int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(count)
                },
                "{name}"
            );
            let regions = memory.snapshot();
            let left = regions.iter().find(|r| r.region_id == "data_left").unwrap();
            let right = regions
                .iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            let bytes = expected.to_le_bytes();
            assert_eq!(left.bytes_hex, format!("{:02x}", bytes[0]), "{name}");
            assert_eq!(
                right.bytes_hex,
                format!(
                    "{}3344556677889900",
                    bytes[1..]
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                ),
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let mut expected_state = before;
            expected_state.rip = HexU64(0x100000 + 2 * count);
            expected_state.gpr[0] = HexU64(rax_after);
            let expected_x87 = expected_state.x87.as_mut().unwrap();
            expected_x87.status_word = HexU64(status_word);
            expected_x87.tag_word = HexU64(0xffff);
            expected_x87.instruction_pointer = HexU64(0x100000);
            expected_x87.data_pointer = HexU64(0x300001);
            assert_eq!(value.state, expected_state, "{name}: full CPU state");
        }
    }

    #[test]
    fn fld_oe_project_fixtures_load_and_observe_sticky_status() {
        let fixtures: [(&[u8], u16, bool); 6] = [
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31du_fld_m32fp_seeded_oe_fnstsw.case.json"
                ),
                0x0008,
                false,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31du_fld_m64fp_seeded_oe_fnstsw.case.json"
                ),
                0x0008,
                false,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31du_fst_fld_m32fp_overflow_infinity_nearest_fnstsw_fnclex.case.json"
                ),
                0x0028,
                true,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31du_fst_fld_m32fp_overflow_infinity_upward_fnstsw_fnclex.case.json"
                ),
                0x0028,
                true,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31du_fst_fld_m64fp_overflow_infinity_nearest_fnstsw_fnclex.case.json"
                ),
                0x0028,
                true,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31du_fst_fld_m64fp_overflow_infinity_upward_fnstsw_fnclex.case.json"
                ),
                0x0028,
                true,
            ),
        ];
        for (bytes, observed_flags, cleared) in fixtures {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31du fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31du fixture snapshot unavailable");
            };
            let x87 = value.state.x87.as_ref().unwrap();
            if cleared {
                assert_eq!(x87.status_word.get() & 0x3f, 0);
                assert_eq!(
                    value.state.gpr[0].get() as u16 & observed_flags,
                    observed_flags
                );
                assert_eq!(value.state.gpr[0].get() as u16 & 0x0200, 0);
                let stack = value
                    .memory
                    .iter()
                    .find(|region| region.region_id == "stack")
                    .unwrap();
                assert!(
                    stack.bytes_hex.contains(
                        if fixture.fixture.memory[0]
                            .initial_bytes_hex
                            .starts_with("d9")
                        {
                            "0000807f"
                        } else {
                            "000000000000f07f"
                        }
                    )
                );
            } else {
                assert_eq!(x87.status_word.get() as u16 & 0x3f, observed_flags);
                assert_eq!(
                    value.state.gpr[0].get() as u16 & observed_flags,
                    observed_flags
                );
            }
        }
    }

    #[test]
    fn paddw_literal_fixture_runs_in_interpreter_and_jit_stays_denied() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_6_paddw_register.case.json"
        ))
        .expect("PADDW schema");
        fixture.fixture.validate().unwrap();
        let code = fixture
            .fixture
            .memory
            .iter()
            .find(|r| r.region_id == "code")
            .unwrap()
            .initial_bytes_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(od_jit::compile(&code).is_err(), "PADDW remains JIT-denied");
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("PADDW fixture snapshot unavailable")
        };
        assert_eq!(&value.state.sse.as_ref().unwrap().xmm[8][..4], "1013");
    }

    #[test]
    fn punpckhdq_literal_fixture_runs_in_interpreter_and_jit_stays_denied() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_29_punpckhdq_register.case.json"
        ))
        .expect("PUNPCKHDQ schema");
        fixture.fixture.validate().unwrap();
        let code = fixture
            .fixture
            .memory
            .iter()
            .find(|r| r.region_id == "code")
            .unwrap()
            .initial_bytes_hex
            .as_bytes()
            .chunks_exact(2)
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert!(
            od_jit::compile(&code).is_err(),
            "PUNPCKHDQ remains JIT-denied"
        );
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("PUNPCKHDQ fixture snapshot unavailable")
        };
        assert_eq!(
            value.state.sse.as_ref().unwrap().xmm[8],
            "08090a0b88898a8b0c0d0e0f8c8d8e8f"
        );
    }
    #[test]
    fn packsswb_literal_fixture_runs_in_interpreter_and_jit_stays_denied() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_32_packsswb_register.case.json"
        ))
        .expect("PACKSSWB schema");
        fixture.fixture.validate().unwrap();
        let code = [0x66, 0x45, 0x0f, 0x63, 0xc1];
        assert!(
            od_jit::compile(&code).is_err(),
            "PACKSSWB remains JIT-denied"
        );
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("PACKSSWB snapshot unavailable")
        };
        assert_eq!(
            value.state.sse.as_ref().unwrap().xmm[8],
            "80807f7f7f800000807f7f7f7f7fff80"
        );
        assert_eq!(
            value.state.sse.as_ref().unwrap().xmm[9],
            "0080ff7f8000ff008001ff00ffff0080"
        );
    }

    #[test]
    fn packuswb_literal_fixture_runs_in_interpreter_and_jit_stays_denied() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_33_packuswb_register.case.json"
        ))
        .expect("PACKUSWB schema");
        fixture.fixture.validate().unwrap();
        let code = [0x66, 0x45, 0x0f, 0x67, 0xc1];
        assert!(
            od_jit::compile(&code).is_err(),
            "PACKUSWB remains JIT-denied"
        );
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("PACKUSWB snapshot unavailable")
        };
        assert_eq!(
            value.state.sse.as_ref().unwrap().xmm[8],
            "00000000017f80ffffff008000000001"
        );
        assert_eq!(
            value.state.sse.as_ref().unwrap().xmm[9],
            "0001ff7f01ff80000000ffff00ff0100"
        );
    }

    #[test]
    fn packssdw_literal_fixture_runs_in_interpreter_and_jit_stays_denied() {
        let fixture = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_35_34_packssdw_register.case.json"
        ))
        .expect("PACKSSDW schema");
        fixture.fixture.validate().unwrap();
        let code = [0x66, 0x45, 0x0f, 0x6b, 0xc1];
        assert!(
            od_jit::compile(&code).is_err(),
            "PACKSSDW remains JIT-denied"
        );
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("PACKSSDW snapshot unavailable")
        };
        assert_eq!(
            value.state.sse.as_ref().unwrap().xmm[8],
            "ff7f0080ff7f0080fe7f0180ff7f0080"
        );
        assert_eq!(
            value.state.sse.as_ref().unwrap().xmm[9],
            "fe7f00000180ffff00800000ff7fffff"
        );
    }
}
