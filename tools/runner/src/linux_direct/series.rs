use super::{evaluate, parse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const SERIES_SCHEMA_VERSION: u32 = 2;
const RUN_PROFILE: &str = "smoke_not_acceptance";
const S5_08_RUN_PROFILE: &str = "stage5_functional";
const COM1_CAPTURE_SCOPE: &str = "linux_direct_run_boot_com1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeriesManifest {
    pub schema_version: u32,
    pub series_id: String,
    pub scenario_id: String,
    pub run_profile: String,
    pub requested_runs: usize,
    pub outcome: ScenarioStatus,
    pub runs: Vec<RunReference>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunReference {
    pub run_id: String,
    pub manifest: String,
    pub manifest_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunManifest {
    pub schema_version: u32,
    pub series_id: String,
    pub run_id: String,
    pub scenario_id: String,
    pub run_profile: String,
    pub nonce: String,
    pub hardware_status: String,
    pub scenario_status: ScenarioStatus,
    pub reason: Option<String>,
    pub runner: RunnerEvidence,
    /// Typed machine stop observation derived from the raw runner stop record. Missing fields in
    /// older raw JSON are represented as unobserved (`None`).
    #[serde(default)]
    pub machine_stop_reason: Option<MachineStopReasonEvidence>,
    pub host: HostEvidence,
    pub configuration: Value,
    pub artifact_hashes: BTreeMap<String, String>,
    pub guest_markers: MarkerEvidence,
    pub uart: UartEvidence,
    pub timing: TimingEvidence,
    pub expected_stop_kind: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MachineStopReasonEvidence {
    Reset,
    Shutdown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunnerEvidence {
    pub exit_code: u8,
    pub guest_started: bool,
    pub retired: Option<u64>,
    pub stop: Option<Value>,
    pub raw_json: String,
    pub raw_json_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostEvidence {
    pub os: String,
    pub arch: String,
    pub runner_version: String,
    pub source_revision: Option<String>,
    #[serde(default)]
    pub source_dirty: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarkerEvidence {
    pub valid: bool,
    pub stage4_nonce_line: Option<String>,
    pub ordered_s5_marker_keys: Vec<String>,
    pub ordered_s5_marker_lines: Vec<String>,
    pub expected_s5_marker_keys: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct UartEvidence {
    pub path: String,
    pub sha256: String,
    pub byte_count: usize,
    pub capture_scope: String,
    pub transcript_complete_for_run_boot: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimingEvidence {
    pub wall_elapsed_seconds: f64,
    pub user_cpu_seconds: Option<f64>,
    pub user_cpu_note: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScenarioStatus {
    Passed,
    Failed,
    NotRun,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Stage5Comparison {
    pub schema_version: u32,
    pub comparison: String,
    pub matches: bool,
    pub explained_differences: Vec<String>,
    pub unexplained_differences: Vec<String>,
}

#[derive(Debug)]
struct SeriesCli {
    requested_runs: usize,
    series_id: String,
    scenario_id: String,
    expected_stop_kind: String,
    expected_s5_marker_keys: Vec<String>,
    capture_execution_observations: bool,
    execution_observations_delay_call_summary: bool,
    execution_observations_elf_stack_copy: bool,
    execution_observations_quantum_trace: bool,
    execution_observations_rdtsc_rip: Option<u64>,
    execution_observations_rdtsc_stride: Option<u64>,
    execution_observations_rdtsc_entry_rip: Option<u64>,
    execution_observations_rdtsc_window: Option<od_machine::RdtscSampleWindow>,
    out: PathBuf,
    forwarded: Vec<String>,
}

#[derive(Debug)]
struct MarkerCheck {
    evidence: MarkerEvidence,
}

fn reserve_series<T>(requested: usize) -> Result<Vec<T>, String> {
    let mut values = Vec::new();
    values
        .try_reserve(requested)
        .map_err(|error| format!("requested series run count cannot be reserved: {error}"))?;
    Ok(values)
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let cli = match parse_series_args(args) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error),
    };
    let expected_out = PathBuf::from("work/stage5")
        .join(&cli.series_id)
        .join("series.json");
    if cli.out != expected_out {
        return invalid_input(&format!(
            "series --out must be exactly {}",
            expected_out.display()
        ));
    }
    let series_dir = expected_out.parent().expect("fixed series path");
    let run_profile = if cli.scenario_id == S5_08_SCENARIO {
        S5_08_RUN_PROFILE
    } else {
        RUN_PROFILE
    };
    let preflight_nonce = "S5PreflightNonce001".to_string();
    let mut preflight_args = cli.forwarded.clone();
    preflight_args.extend([
        "--nonce".into(),
        preflight_nonce,
        "--out".into(),
        series_dir
            .join("preflight.json")
            .to_string_lossy()
            .into_owned(),
    ]);
    if cli.capture_execution_observations {
        append_execution_observations_args(
            &mut preflight_args,
            &series_dir.join("preflight-execution-observations.json"),
            cli.execution_observations_rdtsc_rip,
            cli.execution_observations_rdtsc_stride,
            cli.execution_observations_rdtsc_entry_rip,
            cli.execution_observations_rdtsc_window,
            cli.execution_observations_quantum_trace,
            cli.execution_observations_elf_stack_copy,
            cli.execution_observations_delay_call_summary,
        );
    }
    if let Err(error) = parse(&preflight_args) {
        return invalid_input(&error);
    }
    if let Err(error) = fs::create_dir_all("work/stage5") {
        return invalid_input(&format!("could not create work/stage5: {error}"));
    }
    if let Err(error) = fs::create_dir(series_dir) {
        return invalid_input(&format!("could not create fresh series directory: {error}"));
    }

    let mut references = match reserve_series::<RunReference>(cli.requested_runs) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error),
    };
    let mut statuses = match reserve_series::<ScenarioStatus>(cli.requested_runs) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error),
    };
    for index in 0..cli.requested_runs {
        let run_id = format!("{}-run-{:04}", cli.series_id, index + 1);
        let nonce = match fresh_nonce(index) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error),
        };
        let run_dir = series_dir.join("runs").join(&run_id);
        if let Err(error) = fs::create_dir_all(run_dir.parent().expect("run parent")) {
            return invalid_input(&format!("could not create run parent: {error}"));
        }
        if let Err(error) = fs::create_dir(&run_dir) {
            return invalid_input(&format!("could not create fresh run directory: {error}"));
        }

        let raw_path = run_dir.join("runner.json");
        let observation_sidecar_path = run_dir.join("execution-observations.json");
        let mut run_args = cli.forwarded.clone();
        run_args.extend([
            "--nonce".into(),
            nonce.clone(),
            "--out".into(),
            raw_path.to_string_lossy().into_owned(),
        ]);
        if cli.capture_execution_observations {
            append_execution_observations_args(
                &mut run_args,
                &observation_sidecar_path,
                cli.execution_observations_rdtsc_rip,
                cli.execution_observations_rdtsc_stride,
                cli.execution_observations_rdtsc_entry_rip,
                cli.execution_observations_rdtsc_window,
                cli.execution_observations_quantum_trace,
                cli.execution_observations_elf_stack_copy,
                cli.execution_observations_delay_call_summary,
            );
        }
        let options = match parse(&run_args) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error),
        };
        let started = Instant::now();
        let (mut runner_value, exit_code) = evaluate(&options);
        if runner_value["execution_observations_sidecar"]["sha256"].is_string() {
            let relative = match relative_to_series(series_dir, &observation_sidecar_path) {
                Ok(value) => value,
                Err(error) => return invalid_input(&error),
            };
            runner_value["execution_observations_sidecar"]["path"] = json!(relative);
        }
        let elapsed = started.elapsed().as_secs_f64();
        let raw_bytes = format!("{runner_value:#}\n").into_bytes();
        let raw_sha256 = od_testkit::sha256_hex(&raw_bytes);
        if let Err(error) = write_new(&raw_path, &raw_bytes) {
            return invalid_input(&format!("could not save raw runner JSON: {error}"));
        }

        let uart_bytes = match runner_value["stop"]["com1_hex"].as_str() {
            Some(encoded) => match decode_hex(encoded) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return invalid_input(&format!("invalid stop.com1_hex: {error}"));
                }
            },
            None => Vec::new(),
        };
        let uart_path = run_dir.join("uart.bin");
        if let Err(error) = write_new(&uart_path, &uart_bytes) {
            return invalid_input(&format!(
                "could not save runner-recorded UART bytes: {error}"
            ));
        }
        let marker_check = validate_scenario_markers(
            &cli.scenario_id,
            &uart_bytes,
            &nonce,
            &cli.expected_s5_marker_keys,
            true,
        );
        let hardware_status = runner_value["status"]
            .as_str()
            .unwrap_or("invalid_runner_json")
            .to_string();
        let guest_started = runner_value["guest_started"].as_bool().unwrap_or(false);
        let stop = runner_value.get("stop").cloned();
        let machine_stop_reason = match machine_stop_reason_from_stop(stop.as_ref()) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error),
        };
        let scenario_status = scenario_status(
            &cli.scenario_id,
            &hardware_status,
            guest_started,
            &marker_check.evidence,
            stop.as_ref(),
            &cli.expected_stop_kind,
        );
        let reason = scenario_reason(
            &cli.scenario_id,
            &hardware_status,
            guest_started,
            &marker_check.evidence,
            stop.as_ref(),
            &cli.expected_stop_kind,
        );
        let raw_json_relative = match relative_to_series(series_dir, &raw_path) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error),
        };
        let uart_relative = match relative_to_series(series_dir, &uart_path) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error),
        };
        let run_manifest = RunManifest {
            schema_version: SERIES_SCHEMA_VERSION,
            series_id: cli.series_id.clone(),
            run_id: run_id.clone(),
            scenario_id: cli.scenario_id.clone(),
            run_profile: run_profile.to_string(),
            nonce: nonce.clone(),
            hardware_status,
            scenario_status,
            reason,
            runner: RunnerEvidence {
                exit_code,
                guest_started,
                retired: runner_value["retired"].as_u64(),
                stop,
                raw_json: raw_json_relative,
                raw_json_sha256: raw_sha256,
            },
            machine_stop_reason,
            host: HostEvidence {
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                runner_version: env!("CARGO_PKG_VERSION").to_string(),
                source_revision: build_source_revision(),
                source_dirty: build_source_dirty(),
            },
            configuration: configuration_from_runner(&runner_value),
            artifact_hashes: artifact_hashes(&runner_value),
            guest_markers: marker_check.evidence,
            uart: UartEvidence {
                path: uart_relative,
                sha256: od_testkit::sha256_hex(&uart_bytes),
                byte_count: uart_bytes.len(),
                capture_scope: COM1_CAPTURE_SCOPE.to_string(),
                transcript_complete_for_run_boot: runner_value["stop"].is_object(),
            },
            timing: TimingEvidence {
                wall_elapsed_seconds: elapsed,
                user_cpu_seconds: None,
                user_cpu_note: "not measured by linux-direct".to_string(),
            },
            expected_stop_kind: cli.expected_stop_kind.clone(),
        };
        let run_manifest_path = run_dir.join("manifest.json");
        let run_manifest_bytes = serde_json::to_vec_pretty(&run_manifest)
            .expect("serializing a fixed evidence structure cannot fail");
        if let Err(error) = write_new(&run_manifest_path, &run_manifest_bytes) {
            return invalid_input(&format!("could not save run manifest: {error}"));
        }
        references.push(RunReference {
            run_id,
            manifest: relative_to_series(series_dir, &run_manifest_path)
                .expect("run manifest is under series directory"),
            manifest_sha256: od_testkit::sha256_hex(&run_manifest_bytes),
        });
        statuses.push(scenario_status);
    }

    let outcome = aggregate_status(&statuses);
    let manifest = SeriesManifest {
        schema_version: SERIES_SCHEMA_VERSION,
        series_id: cli.series_id,
        scenario_id: cli.scenario_id,
        run_profile: run_profile.to_string(),
        requested_runs: cli.requested_runs,
        outcome,
        runs: references,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .expect("serializing a fixed evidence structure cannot fail");
    let out = Path::new("work/stage5")
        .join(&manifest.series_id)
        .join("series.json");
    if let Err(error) = write_new(&out, &manifest_bytes) {
        return invalid_input(&format!("could not save series manifest: {error}"));
    }
    let comparison_matches = if cli.requested_runs > 1 {
        match compare_series_runs(&out) {
            Ok(comparison) => {
                let comparison_path = series_dir.join("comparison.json");
                let comparison_bytes = match serde_json::to_vec_pretty(&comparison) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        return invalid_input(&format!(
                            "could not serialize series comparison: {error}"
                        ));
                    }
                };
                if let Err(error) = write_new(&comparison_path, &comparison_bytes) {
                    return invalid_input(&format!("could not save series comparison: {error}"));
                }
                eprintln!(
                    "within-series comparison: matches={} explained={} unexplained={} path={}",
                    comparison.matches,
                    comparison.explained_differences.len(),
                    comparison.unexplained_differences.len(),
                    comparison_path.display()
                );
                if !comparison.matches {
                    eprintln!(
                        "unexplained differences: {:?}",
                        comparison.unexplained_differences
                    );
                }
                comparison.matches
            }
            Err(error) => return invalid_input(&format!("could not compare series runs: {error}")),
        }
    } else {
        eprintln!("within-series comparison skipped: requires at least two runs");
        true
    };
    println!("{}", String::from_utf8_lossy(&manifest_bytes));
    match (outcome, comparison_matches) {
        (ScenarioStatus::Passed, true) => ExitCode::SUCCESS,
        (ScenarioStatus::Passed, false) => ExitCode::from(1),
        (ScenarioStatus::Failed, _) => ExitCode::from(1),
        (ScenarioStatus::NotRun | ScenarioStatus::Blocked, _) => ExitCode::from(78),
    }
}

fn build_source_revision() -> Option<String> {
    source_revision_from_env(option_env!("OD_SOURCE_REVISION"))
}

fn build_source_dirty() -> Option<bool> {
    source_dirty_from_env(option_env!("OD_SOURCE_DIRTY"))
}

fn source_revision_from_env(value: Option<&str>) -> Option<String> {
    value
        .filter(|revision| *revision != "unknown")
        .map(str::to_string)
}

fn source_dirty_from_env(value: Option<&str>) -> Option<bool> {
    match value {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn append_execution_observations_args(
    args: &mut Vec<String>,
    sidecar_path: &std::path::Path,
    selected_rip: Option<u64>,
    stride: Option<u64>,
    entry_rip: Option<u64>,
    window: Option<od_machine::RdtscSampleWindow>,
    quantum_trace: bool,
    elf_stack_copy: bool,
    delay_call_summary: bool,
) {
    args.extend([
        "--execution-observations-out".into(),
        sidecar_path.to_string_lossy().into_owned(),
    ]);
    if let Some(rip) = selected_rip {
        args.extend([
            "--execution-observations-rdtsc-rip".into(),
            format!("0x{rip:x}"),
        ]);
    }
    if let Some(stride) = stride {
        args.extend([
            "--execution-observations-rdtsc-stride".into(),
            stride.to_string(),
        ]);
    }
    if let Some(rip) = entry_rip {
        args.extend([
            "--execution-observations-rdtsc-entry-rip".into(),
            format!("0x{rip:x}"),
        ]);
    }
    if let Some(window) = window {
        args.extend([
            "--execution-observations-rdtsc-window".into(),
            format!("{}:{}", window.first_hit, window.last_hit),
        ]);
    }
    if quantum_trace {
        args.push("--execution-observations-quantum-trace".into());
    }
    if elf_stack_copy {
        args.push("--execution-observations-elf-stack-copy".into());
    }
    if delay_call_summary {
        args.push("--execution-observations-delay-call-summary".into());
    }
}

fn supported_schema_version(version: u32) -> bool {
    matches!(version, 1 | SERIES_SCHEMA_VERSION)
}

fn parse_series_args(args: &[String]) -> Result<SeriesCli, String> {
    let mut requested_runs = None;
    let mut series_id = None;
    let mut scenario_id = None;
    let mut expected_stop_kind = None;
    let mut out = None;
    let mut expected_s5_marker_keys = Vec::new();
    let mut capture_execution_observations = false;
    let mut execution_observations_delay_call_summary = false;
    let mut execution_observations_quantum_trace = false;
    let mut execution_observations_elf_stack_copy = false;
    let mut execution_observations_rdtsc_rip = None;
    let mut execution_observations_rdtsc_stride = None;
    let mut execution_observations_rdtsc_entry_rip = None;
    let mut execution_observations_rdtsc_window = None;
    let mut forwarded = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--execution-observations" {
            if std::mem::replace(&mut capture_execution_observations, true) {
                return Err("duplicate --execution-observations".into());
            }
            index += 1;
            continue;
        }
        if name == "--execution-observations-delay-call-summary" {
            if std::mem::replace(&mut execution_observations_delay_call_summary, true) {
                return Err("duplicate --execution-observations-delay-call-summary".into());
            }
            index += 1;
            continue;
        }
        if matches!(
            name,
            "--stack-context-out"
                | "--execution-observations-out"
                | "--execution-observations-post-iret-index"
                | "--rootfs-raw"
                | "--s6-compute"
                | "--parallel"
                | "--quantum"
                | "--timeout-s"
        ) {
            return Err(format!(
                "{name} is available only for a single linux-direct run"
            ));
        }
        if matches!(
            name,
            "--run"
                | "--set-cpl0"
                | "--diagnostic-past-c7"
                | "--user-mode"
                | "--nonce-bootconfig"
                | "--linux-norandmaps"
                | "--linux-no-randomize-kstack-offset"
        ) {
            forwarded.push(name.to_string());
            index += 1;
            continue;
        }
        if name == "--execution-observations-quantum-trace" {
            if std::mem::replace(&mut execution_observations_quantum_trace, true) {
                return Err("duplicate --execution-observations-quantum-trace".into());
            }
            index += 1;
            continue;
        }
        if name == "--execution-observations-elf-stack-copy" {
            if std::mem::replace(&mut execution_observations_elf_stack_copy, true) {
                return Err("duplicate --execution-observations-elf-stack-copy".into());
            }
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("{name} requires a value"))?;
        match name {
            "--execution-observations-rdtsc-rip" => {
                if execution_observations_rdtsc_rip.is_some() {
                    return Err("duplicate --execution-observations-rdtsc-rip".into());
                }
                let hex = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                    .filter(|digits| !digits.is_empty())
                    .ok_or_else(|| {
                        format!("invalid --execution-observations-rdtsc-rip '{value}': expected 0x-prefixed hexadecimal")
                    })?;
                execution_observations_rdtsc_rip =
                    Some(u64::from_str_radix(hex, 16).map_err(|_| {
                        format!("invalid --execution-observations-rdtsc-rip '{value}'")
                    })?);
            }
            "--execution-observations-rdtsc-entry-rip" => {
                if execution_observations_rdtsc_entry_rip.is_some() {
                    return Err("duplicate --execution-observations-rdtsc-entry-rip".into());
                }
                let hex = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                    .filter(|digits| !digits.is_empty())
                    .ok_or_else(|| {
                        format!("invalid --execution-observations-rdtsc-entry-rip '{value}': expected 0x-prefixed hexadecimal")
                    })?;
                execution_observations_rdtsc_entry_rip =
                    Some(u64::from_str_radix(hex, 16).map_err(|_| {
                        format!("invalid --execution-observations-rdtsc-entry-rip '{value}'")
                    })?);
            }
            "--execution-observations-rdtsc-stride" => {
                if execution_observations_rdtsc_stride.is_some() {
                    return Err("duplicate --execution-observations-rdtsc-stride".into());
                }
                execution_observations_rdtsc_stride = Some(
                    value.parse::<u64>().ok().filter(|stride| *stride > 0).ok_or_else(|| {
                        format!("invalid --execution-observations-rdtsc-stride '{value}': expected a positive integer")
                    })?,
                );
            }
            "--execution-observations-rdtsc-window" => {
                if execution_observations_rdtsc_window.is_some() {
                    return Err("duplicate --execution-observations-rdtsc-window".into());
                }
                execution_observations_rdtsc_window =
                    Some(super::parse_rdtsc_sample_window(value)?);
            }
            "--test-runs" => {
                if requested_runs.is_some() {
                    return Err("duplicate --test-runs".into());
                }
                requested_runs = Some(value.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(
                    || "--test-runs must be a positive explicit integer".to_string(),
                )?);
            }
            "--series-id" => set_unique(&mut series_id, value, name)?,
            "--scenario-id" => set_unique(&mut scenario_id, value, name)?,
            "--expected-stop-kind" => set_unique(&mut expected_stop_kind, value, name)?,
            "--expected-s5-marker" => expected_s5_marker_keys.push(value.clone()),
            "--out" => set_unique(&mut out, value, name)?,
            "--nonce" => return Err("--nonce is generated separately for each test run".into()),
            _ => {
                forwarded.push(name.to_string());
                forwarded.push(value.clone());
            }
        }
        index += 2;
    }
    let requested_runs = requested_runs.ok_or("series requires --test-runs N")?;
    if execution_observations_delay_call_summary && !capture_execution_observations {
        return Err(
            "--execution-observations-delay-call-summary needs --execution-observations".into(),
        );
    }
    if execution_observations_delay_call_summary
        && (execution_observations_rdtsc_rip
            .is_some_and(|rip| rip != od_machine::LINUX_DELAY_TSC_POLL_RIP)
            || execution_observations_rdtsc_entry_rip
                .is_some_and(|rip| rip != od_machine::LINUX_DELAY_TSC_ENTRY_RIP)
            || execution_observations_rdtsc_stride.is_some_and(|stride| stride != 1)
            || execution_observations_rdtsc_window.is_some())
    {
        return Err("--execution-observations-delay-call-summary conflicts with selected RIP, stride, or window options".into());
    }
    let series_id = required_id(series_id, "--series-id")?;
    let scenario_id = required_id(scenario_id, "--scenario-id")?;
    let expected_stop_kind = required_id(expected_stop_kind, "--expected-stop-kind")?;
    if !matches!(
        expected_stop_kind.as_str(),
        "budget_exhausted"
            | "stopped"
            | "halted"
            | "shutdown"
            | "rejected"
            | "integration_error"
            | "unsupported"
            | "fault"
            | "backend"
            | "port_io"
            | "cpuid_divergence"
            | "user_entry"
    ) {
        return Err(format!(
            "unsupported --expected-stop-kind '{expected_stop_kind}'"
        ));
    }
    let out =
        PathBuf::from(out.ok_or("series requires --out work/stage5/<series-id>/series.json")?);
    if !forwarded.iter().any(|value| value == "--run") {
        return Err("series requires --run --budget".into());
    }
    if execution_observations_rdtsc_rip.is_some() && !capture_execution_observations {
        return Err("--execution-observations-rdtsc-rip needs --execution-observations".into());
    }
    if execution_observations_rdtsc_entry_rip.is_some()
        && (execution_observations_rdtsc_rip.is_none() || !capture_execution_observations)
    {
        return Err("--execution-observations-rdtsc-entry-rip needs --execution-observations and --execution-observations-rdtsc-rip".into());
    }
    if execution_observations_rdtsc_entry_rip.is_some()
        && execution_observations_rdtsc_entry_rip == execution_observations_rdtsc_rip
    {
        return Err("entry and selected RDTSC RIPs must differ".into());
    }
    if execution_observations_rdtsc_stride.is_some()
        && (execution_observations_rdtsc_rip.is_none() || !capture_execution_observations)
    {
        return Err("--execution-observations-rdtsc-stride needs --execution-observations and --execution-observations-rdtsc-rip".into());
    }
    if execution_observations_rdtsc_window.is_some()
        && (execution_observations_rdtsc_rip.is_none() || !capture_execution_observations)
    {
        return Err("--execution-observations-rdtsc-window needs --execution-observations and --execution-observations-rdtsc-rip".into());
    }
    if execution_observations_quantum_trace && !capture_execution_observations {
        return Err("--execution-observations-quantum-trace needs --execution-observations".into());
    }
    if execution_observations_elf_stack_copy && !capture_execution_observations {
        return Err(
            "--execution-observations-elf-stack-copy needs --execution-observations".into(),
        );
    }
    if let Some(window) = execution_observations_rdtsc_window {
        super::validate_rdtsc_sample_window_capacity(
            window,
            execution_observations_rdtsc_stride.unwrap_or(1),
        )?;
    }
    for key in &expected_s5_marker_keys {
        if !valid_marker_key(key) {
            return Err(format!("invalid --expected-s5-marker key '{key}'"));
        }
    }
    let mut marker_counts = BTreeMap::new();
    for key in &expected_s5_marker_keys {
        *marker_counts.entry(key).or_insert(0usize) += 1;
    }
    if marker_counts
        .iter()
        .any(|(key, count)| *count > 1 && !key.ends_with("_FILE"))
    {
        return Err("only *_FILE marker keys may be expected more than once".into());
    }
    if scenario_id == S5_08_SCENARIO {
        if expected_stop_kind != "stopped" || requested_runs < 2 {
            return Err("s5-08 requires --expected-stop-kind stopped and at least two runs".into());
        }
        if expected_s5_marker_keys.is_empty() {
            expected_s5_marker_keys.push(S5_08_SHUTDOWN_MARKER.into());
        } else if expected_s5_marker_keys != [S5_08_SHUTDOWN_MARKER] {
            return Err("s5-08 expects only A_SHUTDOWN_REQUEST".into());
        }
    }
    if let Some(letter) = s5_scenario_letter(&scenario_id) {
        if expected_s5_marker_keys.is_empty()
            || expected_s5_marker_keys
                .iter()
                .any(|key| !key.starts_with(&format!("{letter}_")))
        {
            return Err(format!(
                "S5 scenario {scenario_id} needs expected marker keys beginning with {letter}_"
            ));
        }
    }
    Ok(SeriesCli {
        requested_runs,
        series_id,
        scenario_id,
        expected_stop_kind,
        expected_s5_marker_keys,
        capture_execution_observations,
        execution_observations_delay_call_summary,
        execution_observations_quantum_trace,
        execution_observations_elf_stack_copy,
        execution_observations_rdtsc_rip,
        execution_observations_rdtsc_stride,
        execution_observations_rdtsc_entry_rip,
        execution_observations_rdtsc_window,
        out,
        forwarded,
    })
}

fn set_unique(slot: &mut Option<String>, value: &str, name: &str) -> Result<(), String> {
    if slot.replace(value.to_string()).is_some() {
        return Err(format!("duplicate option {name}"));
    }
    Ok(())
}

fn required_id(value: Option<String>, name: &str) -> Result<String, String> {
    let value = value.ok_or_else(|| format!("series requires {name}"))?;
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!(
            "{name} must use 1..=64 ASCII alphanumeric, '.', '_' or '-' chars"
        ));
    }
    Ok(value)
}

fn valid_marker_key(key: &str) -> bool {
    let mut parts = key.splitn(2, '_');
    matches!(parts.next(), Some("A" | "B" | "C" | "D" | "E" | "F" | "G"))
        && parts.next().is_some_and(|tail| {
            !tail.is_empty()
                && tail
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
        })
}

fn s5_scenario_letter(scenario_id: &str) -> Option<char> {
    let normalized = scenario_id.to_ascii_uppercase();
    let suffix = normalized.strip_prefix("S5-")?;
    let mut chars = suffix.chars();
    let letter = chars.next()?;
    if matches!(letter, 'A'..='G') && chars.next().is_none_or(|value| value == '-') {
        Some(letter)
    } else {
        None
    }
}

fn fresh_nonce(index: usize) -> Result<String, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_nanos();
    let nonce = format!("OD5{nanos:016x}{:08x}{index:08x}", std::process::id());
    if !valid_nonce(&nonce) {
        return Err("generated nonce does not satisfy runner token constraints".into());
    }
    Ok(nonce)
}

fn valid_nonce(nonce: &str) -> bool {
    (16..=64).contains(&nonce.len()) && nonce.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn validate_markers(bytes: &[u8], nonce: &str, expected_s5: &[String]) -> MarkerCheck {
    validate_markers_with_crlf(bytes, nonce, expected_s5, true)
}

fn validate_markers_legacy(bytes: &[u8], nonce: &str, expected_s5: &[String]) -> MarkerCheck {
    validate_markers_with_crlf(bytes, nonce, expected_s5, false)
}

fn validate_markers_with_crlf(
    bytes: &[u8],
    nonce: &str,
    expected_s5: &[String],
    normalize_crlf: bool,
) -> MarkerCheck {
    let mut errors = Vec::new();
    let mut stage4_nonce_line = None;
    let mut s5_keys = Vec::new();
    let mut s5_lines = Vec::new();
    let mut seen_s5 = false;
    let mut exact_lines = BTreeMap::new();
    let text = match std::str::from_utf8(bytes) {
        Ok(value) => value,
        Err(_) => {
            errors.push("runner-recorded COM1 bytes are not UTF-8 marker text".to_string());
            ""
        }
    };
    for raw in text.split_inclusive('\n') {
        let terminated = raw.ends_with('\n');
        let raw_line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = if normalize_crlf {
            raw_line.strip_suffix('\r').unwrap_or(raw_line)
        } else {
            raw_line
        };
        if !line.starts_with("OD_NONCE=") && !line.starts_with("OD_S5_") {
            continue;
        }
        if !terminated {
            errors.push(format!("marker line is not newline terminated: {line:?}"));
        }
        if *exact_lines.entry(line.to_string()).or_insert(0usize) > 0 {
            errors.push(format!("duplicate marker line: {line}"));
        }
        *exact_lines.entry(line.to_string()).or_insert(0usize) += 1;
        if let Some(marker_nonce) = line.strip_prefix("OD_NONCE=") {
            if seen_s5 {
                errors.push("OD_NONCE marker must precede S5 markers".into());
            }
            if marker_nonce != nonce {
                errors.push(format!(
                    "OD_NONCE value does not match run nonce: {marker_nonce}"
                ));
            }
            if stage4_nonce_line.replace(line.to_string()).is_some() {
                errors.push("duplicate OD_NONCE marker".into());
            }
        } else if let Some(rest) = line.strip_prefix("OD_S5_") {
            seen_s5 = true;
            let key = rest.split_whitespace().next().unwrap_or_default();
            if !valid_marker_key(key) {
                errors.push(format!("invalid S5 marker key: {key}"));
            }
            let nonce_tokens = line
                .split_whitespace()
                .filter(|token| token.starts_with("nonce="))
                .collect::<Vec<_>>();
            if nonce_tokens.len() != 1 || nonce_tokens[0] != format!("nonce={nonce}") {
                errors.push(format!(
                    "S5 marker has missing, duplicate, or wrong nonce: {line}"
                ));
            }
            s5_keys.push(key.to_string());
            s5_lines.push(line.to_string());
        }
    }
    if stage4_nonce_line.is_none() {
        errors.push("missing exact OD_NONCE=<run nonce> line".into());
    }
    if s5_keys != expected_s5 {
        errors.push(format!(
            "ordered S5 marker keys differ: expected {expected_s5:?}, observed {s5_keys:?}"
        ));
    }
    MarkerCheck {
        evidence: MarkerEvidence {
            valid: errors.is_empty(),
            stage4_nonce_line,
            ordered_s5_marker_keys: s5_keys,
            ordered_s5_marker_lines: s5_lines,
            expected_s5_marker_keys: expected_s5.to_vec(),
            errors,
        },
    }
}

const S5_08_SCENARIO: &str = "s5-08";
const S5_08_SHUTDOWN_MARKER: &str = "A_SHUTDOWN_REQUEST";

fn validate_scenario_markers(
    scenario: &str,
    bytes: &[u8],
    nonce: &str,
    expected_s5: &[String],
    normalize_crlf: bool,
) -> MarkerCheck {
    let mut checked = if normalize_crlf {
        validate_markers(bytes, nonce, expected_s5)
    } else {
        validate_markers_legacy(bytes, nonce, expected_s5)
    };
    if scenario == S5_08_SCENARIO {
        let evidence = &mut checked.evidence;
        let expected_line = format!("OD_S5_{S5_08_SHUTDOWN_MARKER} nonce={nonce} method=poweroff");
        if evidence.ordered_s5_marker_lines != [expected_line] {
            evidence
                .errors
                .push("S5-08 requires one exact poweroff request marker".into());
        }
        if bytes
            .windows(b"Kernel panic - not syncing:".len())
            .any(|window| window == b"Kernel panic - not syncing:")
        {
            evidence
                .errors
                .push("kernel panic cannot pass S5-08".into());
        }
        evidence.valid = evidence.errors.is_empty();
    }
    checked
}

fn s5_08_shutdown_matches(stop: Option<&Value>, expected_stop_kind: &str) -> bool {
    expected_stop_kind == "stopped"
        && stop.is_some_and(|stop| {
            stop["exit_kind"] == "stopped" && stop["machine_stop_reason"] == "shutdown"
        })
}

fn scenario_status(
    scenario_id: &str,
    hardware_status: &str,
    guest_started: bool,
    markers: &MarkerEvidence,
    stop: Option<&Value>,
    expected_stop_kind: &str,
) -> ScenarioStatus {
    if !guest_started {
        return if hardware_status == "refused" {
            ScenarioStatus::Blocked
        } else {
            ScenarioStatus::NotRun
        };
    }
    if hardware_status != "loaded" {
        return ScenarioStatus::Failed;
    }
    if scenario_id == S5_08_SCENARIO {
        return if markers.valid && s5_08_shutdown_matches(stop, expected_stop_kind) {
            ScenarioStatus::Passed
        } else {
            ScenarioStatus::Failed
        };
    }
    if scenario_id.to_ascii_lowercase().starts_with("s5-") {
        if !markers.ordered_s5_marker_keys.is_empty() && !markers.valid {
            return ScenarioStatus::Failed;
        }
        return ScenarioStatus::NotRun;
    }
    if !markers.valid {
        return ScenarioStatus::Failed;
    }
    if scenario_id == "stage4-init-nonce"
        && markers.expected_s5_marker_keys.is_empty()
        && stop.and_then(|value| value["exit_kind"].as_str()) == Some(expected_stop_kind)
    {
        return ScenarioStatus::Passed;
    }
    ScenarioStatus::NotRun
}

fn scenario_reason(
    scenario_id: &str,
    hardware_status: &str,
    guest_started: bool,
    markers: &MarkerEvidence,
    stop: Option<&Value>,
    expected_stop_kind: &str,
) -> Option<String> {
    match scenario_status(
        scenario_id,
        hardware_status,
        guest_started,
        markers,
        stop,
        expected_stop_kind,
    ) {
        ScenarioStatus::Passed => None,
        ScenarioStatus::Failed if !markers.errors.is_empty() => Some(markers.errors.join("; ")),
        ScenarioStatus::Failed if scenario_id == S5_08_SCENARIO && hardware_status == "loaded" => {
            Some("expected an ACPI shutdown stop after the S5-08 markers".into())
        }
        ScenarioStatus::Failed => Some("runner execution failed after guest start".into()),
        ScenarioStatus::Blocked => Some("linux-direct refused before guest start".into()),
        ScenarioStatus::NotRun if !guest_started => Some("guest was not started".into()),
        ScenarioStatus::NotRun if scenario_id.to_ascii_lowercase().starts_with("s5-") => {
            Some("S5-08 guest scenario invocation is not integrated".into())
        }
        ScenarioStatus::NotRun if scenario_id == "stage4-init-nonce" => Some(format!(
            "expected stop kind {expected_stop_kind:?} was not observed"
        )),
        ScenarioStatus::NotRun => {
            Some("scenario has no executable integration in this profile".into())
        }
    }
}

fn configuration_from_runner(runner: &Value) -> Value {
    let mut configuration = json!({
        "cmdline": runner["cmdline"],
        "ram_mib": runner["ram_mib"],
        "budget": runner["stop"]["budget"],
        "cpu_path": runner["stop"]["path"],
        "cpu_profile": runner["stop"]["cpu_profile"],
        "diagnostic_past_c7": runner["diagnostic_past_c7"],
        "linux_norandmaps": runner["linux_norandmaps"],
        "linux_no_randomize_kstack_offset": runner["linux_no_randomize_kstack_offset"],
        "acceptance": runner["acceptance"],
    });
    if runner["inputs"]["initramfs"]
        .get("nonce_bootconfig")
        .is_some()
    {
        configuration["nonce_transport"] = json!("bootconfig");
    }
    if let Some(scheduler) = runner.get("interleaved_scheduler") {
        configuration["interleaved_scheduler"] = scheduler.clone();
    }
    configuration
}

fn configuration_matches_runner(configuration: &Value, runner: &Value) -> bool {
    let Some(configuration_object) = configuration.as_object() else {
        return false;
    };
    let mut expected = configuration_from_runner(runner);
    let Some(expected_object) = expected.as_object_mut() else {
        return false;
    };
    // These optional Linux diagnostics were added without a schema bump. Older manifests omit
    // their configuration keys even when the raw runner evidence predates or records the flags.
    // Ignore only absent keys; a present value and all other configuration (including cmdline)
    // must still match the raw JSON exactly.
    for key in ["linux_norandmaps", "linux_no_randomize_kstack_offset"] {
        if !configuration_object.contains_key(key) {
            expected_object.remove(key);
        }
    }
    configuration == &expected
}

fn enrich_legacy_diagnostic_configuration(configuration: &mut Value, runner: &Value) {
    let Some(configuration_object) = configuration.as_object_mut() else {
        return;
    };
    for key in ["linux_norandmaps", "linux_no_randomize_kstack_offset"] {
        if !configuration_object.contains_key(key) {
            if let Some(value) = runner.get(key) {
                configuration_object.insert(key.into(), value.clone());
            }
        }
    }
}

fn aggregate_status(statuses: &[ScenarioStatus]) -> ScenarioStatus {
    if statuses
        .iter()
        .all(|status| *status == ScenarioStatus::Passed)
    {
        ScenarioStatus::Passed
    } else if statuses.contains(&ScenarioStatus::Failed) {
        ScenarioStatus::Failed
    } else if statuses.contains(&ScenarioStatus::Blocked) {
        ScenarioStatus::Blocked
    } else {
        ScenarioStatus::NotRun
    }
}

const NONCE_INITRD_STOP_HASH_FIELDS: [&str; 2] = ["initramfs_sha256", "initramfs_sha256_in_memory"];

fn validate_nonce_bootconfig(runner: &Value, nonce: &str) -> Result<(), String> {
    let initramfs = &runner["inputs"]["initramfs"];
    let Some(bootconfig) = initramfs.get("nonce_bootconfig") else {
        if runner["cmdline"] == super::nonce_bootconfig_cmdline() {
            return Err("nonce bootconfig evidence is missing".into());
        }
        return Ok(());
    };
    let base_bytes = initramfs["bytes"]
        .as_u64()
        .ok_or("nonce bootconfig base initramfs size is missing")?;
    let trailer = super::nonce_bootconfig_trailer(nonce, base_bytes);
    let loaded_bytes = base_bytes
        .checked_add(trailer.len() as u64)
        .ok_or("nonce bootconfig loaded initrd size overflows")?;
    let loaded_hash = &bootconfig["loaded_sha256"];
    let is_sha256 = |value: &Value| {
        value.as_str().is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
        })
    };
    if runner["cmdline"] != super::nonce_bootconfig_cmdline()
        || !is_sha256(&initramfs["sha256"])
        || initramfs["expected_sha256"] != initramfs["sha256"]
        || bootconfig["trailer_sha256"] != od_testkit::sha256_hex(&trailer)
        || bootconfig["loaded_bytes"].as_u64() != Some(loaded_bytes)
        || !is_sha256(loaded_hash)
        || (runner["status"] == "loaded"
            && (runner["cmdline_in_memory_matches"] != true
                || runner["cmdline_in_memory"] != runner["cmdline"]
                || runner["memory_sha256"]["initrd"] != *loaded_hash
                || runner["memory_sha256"]["initrd_matches_file"] != true))
        || (runner["stop"].is_object()
            && NONCE_INITRD_STOP_HASH_FIELDS
                .iter()
                .any(|field| runner["stop"]["inputs"][*field] != *loaded_hash))
    {
        return Err("nonce bootconfig does not match its run input or memory evidence".into());
    }
    Ok(())
}

fn artifact_hashes(runner: &Value) -> BTreeMap<String, String> {
    let mut hashes = BTreeMap::new();
    for name in ["vmlinuz", "initramfs", "acpi_blob"] {
        if let Some(hash) = runner["inputs"][name]["sha256"].as_str() {
            hashes.insert(name.to_string(), hash.to_string());
        }
    }
    if let Some(hash) = runner["acpi"]["manifest_hash"].as_str() {
        hashes.insert("acpi_manifest".to_string(), hash.to_string());
    }
    hashes
}

fn relative_to_series(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|error| format!("artifact path escapes series directory: {error}"))
}

fn validate_execution_observations_sidecar(
    root: &Path,
    run_id: &str,
    runner: &Value,
) -> Result<(), String> {
    let Some(sidecar) = runner.get("execution_observations_sidecar") else {
        return Ok(());
    };
    let has_sha256 = sidecar.get("sha256").is_some();
    let has_byte_count = sidecar.get("byte_count").is_some();
    if !has_sha256 && !has_byte_count {
        // Older raw runner JSON recorded a path and write_error, but not integrity metadata.
        return Ok(());
    }
    if has_sha256 != has_byte_count
        || sidecar
            .get("write_error")
            .map_or(true, |write_error| !write_error.is_null())
    {
        return Err("execution-observations sidecar integrity metadata is incomplete".into());
    }
    let path = sidecar["path"]
        .as_str()
        .ok_or("execution-observations sidecar path is missing")?;
    if path != format!("runs/{run_id}/execution-observations.json") {
        return Err("execution-observations sidecar path does not match run directory".into());
    }
    let expected_sha256 = sidecar["sha256"]
        .as_str()
        .ok_or("execution-observations sidecar SHA-256 is missing")?;
    let expected_byte_count = sidecar["byte_count"]
        .as_u64()
        .ok_or("execution-observations sidecar byte count is invalid")?;
    let selected_rip_is_canonical = match sidecar.get("selected_rip") {
        Some(Value::Null) => true,
        Some(Value::String(rip)) => canonical_hex64(rip),
        _ => false,
    };
    if expected_sha256.len() != 64
        || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || sidecar["stride"].as_u64().unwrap_or(0) == 0
        || sidecar["selected_site_hits"].as_u64().is_none()
        || sidecar["sample_count"].as_u64().is_none()
        || sidecar["samples_omitted"].as_u64().is_none()
        || !selected_rip_is_canonical
    {
        return Err("execution-observations sidecar integrity metadata is invalid".into());
    }
    let path = safe_relative(root, path)?;
    let bytes = fs::read(path)
        .map_err(|error| format!("could not read execution-observations sidecar: {error}"))?;
    if bytes.len() as u64 != expected_byte_count
        || od_testkit::sha256_hex(&bytes) != expected_sha256
    {
        return Err("execution-observations sidecar byte count or SHA-256 mismatch".into());
    }
    let sidecar_json: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid execution-observations sidecar JSON: {error}"))?;
    let schema_version = sidecar_json["schema_version"]
        .as_u64()
        .filter(|version| supported_execution_observations_schema_version(*version))
        .ok_or_else(|| {
            "execution-observations sidecar schema_version is missing, invalid, or unsupported"
                .to_string()
        })?;
    let sample_count = sidecar_json["rdtsc_site_samples"]
        .as_array()
        .map(|samples| samples.len() as u64);
    if sidecar_json["rdtsc_sample_rip"] != sidecar["selected_rip"]
        || sidecar_json["rdtsc_sample_stride"] != sidecar["stride"]
        || sidecar_json["rdtsc_sample_site_hits"] != sidecar["selected_site_hits"]
        || sample_count != sidecar["sample_count"].as_u64()
        || sidecar_json["rdtsc_site_samples_omitted"] != sidecar["samples_omitted"]
    {
        return Err(
            "execution-observations sidecar summary does not match raw runner metadata".into(),
        );
    }
    if schema_version >= 8 {
        let entry_rip_is_canonical = match sidecar.get("entry_rip") {
            Some(Value::Null) => true,
            Some(Value::String(rip)) => canonical_hex64(rip),
            _ => false,
        };
        let raw_entry_rip_is_canonical = match sidecar_json.get("rdtsc_entry_sample_rip") {
            Some(Value::Null) => true,
            Some(Value::String(rip)) => canonical_hex64(rip),
            _ => false,
        };
        let entry_hits = sidecar.get("entry_hits").and_then(Value::as_u64);
        let entry_samples_omitted = sidecar.get("entry_samples_omitted").and_then(Value::as_u64);
        let entry_sample_count_metadata = sidecar.get("entry_sample_count").and_then(Value::as_u64);
        let entry_sample_count = sidecar_json["rdtsc_entry_samples"]
            .as_array()
            .map(|samples| samples.len() as u64);
        let raw_entry_hits = sidecar_json["rdtsc_entry_sample_hits"].as_u64();
        let raw_entry_samples_omitted = sidecar_json["rdtsc_entry_samples_omitted"].as_u64();
        if !entry_rip_is_canonical
            || !raw_entry_rip_is_canonical
            || entry_hits.is_none()
            || entry_samples_omitted.is_none()
            || entry_sample_count_metadata.is_none()
            || entry_sample_count.is_none()
            || raw_entry_hits.is_none()
            || raw_entry_samples_omitted.is_none()
        {
            return Err(
                "execution-observations entry sample metadata is missing or invalid".into(),
            );
        }
        if sidecar_json["rdtsc_entry_sample_rip"] != sidecar["entry_rip"]
            || raw_entry_hits != entry_hits
            || entry_sample_count != entry_sample_count_metadata
            || raw_entry_samples_omitted != entry_samples_omitted
        {
            return Err(
                "execution-observations entry sample summary does not match raw runner metadata"
                    .into(),
            );
        }
    }
    if schema_version >= 9 {
        let metadata_window = parse_sidecar_sample_window(sidecar.get("sample_window"))?;
        let raw_window = parse_sidecar_sample_window(sidecar_json.get("rdtsc_sample_window"))?;
        let stride = sidecar["stride"].as_u64().unwrap_or(0);
        let selected_hits = sidecar["selected_site_hits"].as_u64().unwrap_or(0);
        let sample_count = sidecar["sample_count"].as_u64().unwrap_or(0);
        let samples_omitted = sidecar["samples_omitted"].as_u64().unwrap_or(0);
        let accounted_samples = sample_count
            .checked_add(samples_omitted)
            .ok_or_else(|| "execution-observations sample window counts overflow".to_string())?;
        if metadata_window != raw_window {
            return Err(
                "execution-observations sample window does not match raw runner metadata".into(),
            );
        }
        if let Some(window) = raw_window {
            let candidates = window.last_hit / stride - (window.first_hit - 1) / stride;
            if !selected_rip_is_canonical_not_null(sidecar.get("selected_rip"))
                || candidates > od_machine::LINUX_RDTSC_SAMPLE_CAPACITY as u64
                || accounted_samples > candidates
                || (selected_hits >= window.last_hit && accounted_samples != candidates)
            {
                return Err("execution-observations sample window counts are invalid".into());
            }
        }
    }
    if schema_version >= 10 {
        let metadata_trace_count = sidecar
            .get("entry_clock_trace_sample_count")
            .and_then(Value::as_u64);
        let metadata_trace_missing = sidecar
            .get("entry_clock_trace_missing")
            .and_then(Value::as_u64);
        let raw_trace_samples = sidecar_json
            .get("rdtsc_entry_clock_trace_samples")
            .and_then(Value::as_array);
        let raw_trace_missing = sidecar_json
            .get("rdtsc_entry_clock_trace_missing")
            .and_then(Value::as_u64);
        let expected_trace_count = sidecar
            .get("entry_hits")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(od_machine::LINUX_RDTSC_CLOCK_TRACE_CAPACITY as u64);
        let Some(raw_trace_samples) = raw_trace_samples else {
            return Err("execution-observations clock trace is missing or invalid".into());
        };
        let Some(raw_trace_missing) = raw_trace_missing else {
            return Err("execution-observations clock trace is missing or invalid".into());
        };
        let accounted_trace_hits = (raw_trace_samples.len() as u64)
            .checked_add(raw_trace_missing)
            .ok_or_else(|| "execution-observations clock trace count overflow".to_string())?;
        if metadata_trace_count != Some(raw_trace_samples.len() as u64)
            || metadata_trace_missing != Some(raw_trace_missing)
            || raw_trace_samples.len() > od_machine::LINUX_RDTSC_CLOCK_TRACE_CAPACITY
            || accounted_trace_hits != expected_trace_count
        {
            return Err(
                "execution-observations clock trace summary does not match raw runner metadata"
                    .into(),
            );
        }
        let entry_rip = sidecar_json["rdtsc_entry_sample_rip"].as_str();
        let mut prior_hit = 0u64;
        for sample in raw_trace_samples {
            let valid_rip = sample
                .get("rip")
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64);
            let hit_ordinal = sample.get("hit_ordinal").and_then(Value::as_u64);
            let callback_tsc_is_canonical = sample
                .get("callback_tsc")
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64);
            let initial_ns = sample.get("initial_now_ns").and_then(Value::as_u64);
            let now_ns = sample.get("now_ns").and_then(Value::as_u64);
            let tsc_hz = sample.get("tsc_hz").and_then(Value::as_u64);
            let numeric_fields_valid = [
                "retired_instructions_charged",
                "pm_timer_ns_charged",
                "pit_ns_charged",
                "rdtsc_ns_charged",
                "idle_ns_advanced",
                "quantum_ordinal",
                "irqs_delivered",
            ]
            .into_iter()
            .all(|key| sample.get(key).and_then(Value::as_u64).is_some());
            let retired_ns_valid = sample
                .get("retired_ns_charged")
                .is_some_and(|value| value.is_null() || value.as_u64().is_some());
            let unaccounted_valid = matches!(
                sample.get("unaccounted_elapsed_ns"),
                Some(Value::Null) | Some(Value::Number(_))
            ) && sample
                .get("unaccounted_elapsed_ns")
                .is_some_and(|value| value.is_null() || value.as_u64().is_some());
            if !valid_rip
                || sample.get("rip").and_then(Value::as_str) != entry_rip
                || hit_ordinal
                    .is_none_or(|hit| hit == 0 || hit > expected_trace_count || hit <= prior_hit)
                || !callback_tsc_is_canonical
                || initial_ns
                    .zip(now_ns)
                    .is_none_or(|(initial, now)| now < initial)
                || tsc_hz.is_none_or(|frequency| frequency == 0)
                || !numeric_fields_valid
                || !retired_ns_valid
                || !unaccounted_valid
            {
                return Err("execution-observations clock trace sample is invalid".into());
            }
            prior_hit = hit_ordinal.unwrap_or(0);
        }
    }
    if schema_version >= 12 {
        let metadata_status = sidecar.get("rdtsc_prefix_status").and_then(Value::as_str);
        let raw_status = sidecar_json
            .get("rdtsc_prefix_status")
            .and_then(Value::as_str);
        let metadata_sentinel = sidecar.get("rdtsc_prefix_sentinel_rip");
        let raw_sentinel = sidecar_json.get("rdtsc_prefix_sentinel_rip");
        let sentinel_is_valid = |value: Option<&Value>| match value {
            Some(Value::Null) => true,
            Some(Value::String(rip)) => canonical_hex64(rip),
            _ => false,
        };
        let metadata_count = sidecar
            .get("rdtsc_prefix_sample_count")
            .and_then(Value::as_u64);
        let raw_samples = sidecar_json
            .get("rdtsc_prefix_samples")
            .and_then(Value::as_array);
        let metadata_events = sidecar
            .get("rdtsc_prefix_event_count")
            .and_then(Value::as_u64);
        let raw_events = sidecar_json
            .get("rdtsc_prefix_event_count")
            .and_then(Value::as_u64);
        let metadata_missing = sidecar.get("rdtsc_prefix_missing").and_then(Value::as_u64);
        let raw_missing = sidecar_json
            .get("rdtsc_prefix_missing")
            .and_then(Value::as_u64);
        let Some(raw_samples) = raw_samples else {
            return Err("execution-observations RDTSC prefix is missing or invalid".into());
        };
        if !sentinel_is_valid(metadata_sentinel)
            || !sentinel_is_valid(raw_sentinel)
            || metadata_sentinel != raw_sentinel
            || metadata_status.is_none()
            || raw_status != metadata_status
            || metadata_count != Some(raw_samples.len() as u64)
            || metadata_events.is_none()
            || raw_events != metadata_events
            || metadata_missing.is_none()
            || raw_missing != metadata_missing
            || raw_samples.len() > od_machine::LINUX_RDTSC_PREFIX_CAPACITY
            || raw_events.is_none_or(|count| count > od_machine::LINUX_RDTSC_PREFIX_CAPACITY as u64)
        {
            return Err("execution-observations RDTSC prefix summary is invalid".into());
        }
        let status = raw_status.unwrap_or_default();
        let sentinel_requested = raw_sentinel.and_then(Value::as_str) == Some("0xffffffff8271b31b");
        let event_count = raw_events.unwrap_or(0);
        let missing = raw_missing.unwrap_or(0);
        let retained_and_missing = (raw_samples.len() as u64)
            .checked_add(missing)
            .ok_or_else(|| "execution-observations RDTSC prefix count overflow".to_string())?;
        let mut previous = 0u64;
        let mut previous_page_faults = 0u64;
        let mut previous_general_protection_faults = 0u64;
        let mut clock_origin: Option<(u64, u64)> = None;
        for sample in raw_samples {
            let ordinal = sample.get("ordinal").and_then(Value::as_u64).unwrap_or(0);
            let rip = sample.get("rip").and_then(Value::as_str);
            let cpl_valid = matches!(sample.get("cpl"), Some(Value::Null))
                || sample.get("cpl").and_then(Value::as_u64) == Some(0);
            let context_valid = sample.get("cr3").is_some_and(|value| {
                value.is_null() || value.as_str().is_some_and(canonical_hex64)
            }) && ["rsp", "rbp"].into_iter().all(|key| {
                sample
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(canonical_hex64)
            });
            let page_faults = sample
                .get("page_faults_delivered_through")
                .and_then(Value::as_u64);
            let general_protection_faults = sample
                .get("general_protection_faults_delivered_through")
                .and_then(Value::as_u64);
            let faults_valid = page_faults.zip(general_protection_faults).is_some_and(
                |(page_faults, general_protection_faults)| {
                    page_faults >= previous_page_faults
                        && general_protection_faults >= previous_general_protection_faults
                },
            );
            let clock_valid = sample
                .get("clock")
                .and_then(Value::as_object)
                .is_some_and(|clock| {
                    let charged = clock
                        .get("retired_instructions_charged")
                        .and_then(Value::as_u64);
                    let quantum = clock
                        .get("quantum_instructions_executed")
                        .and_then(Value::as_u64);
                    let through = clock
                        .get("retired_instructions_through_rdtsc")
                        .and_then(Value::as_u64);
                    let initial_ns = clock.get("initial_now_ns").and_then(Value::as_u64);
                    let now_ns = clock.get("now_ns").and_then(Value::as_u64);
                    let tsc_hz = clock.get("tsc_hz").and_then(Value::as_u64);
                    let callback_tsc = clock
                        .get("callback_tsc")
                        .and_then(Value::as_str)
                        .is_some_and(canonical_hex64);
                    let numeric_fields_valid = [
                        "pm_timer_ns_charged",
                        "pit_ns_charged",
                        "rdtsc_ns_charged",
                        "idle_ns_advanced",
                        "quantum_ordinal",
                        "irqs_delivered",
                    ]
                    .into_iter()
                    .all(|key| clock.get(key).and_then(Value::as_u64).is_some());
                    let retired_ns_valid = clock
                        .get("retired_ns_charged")
                        .is_some_and(|value| value.is_null() || value.as_u64() == charged);
                    let unaccounted_valid = clock
                        .get("unaccounted_elapsed_ns")
                        .is_some_and(|value| value.is_null() || value.as_u64().is_some());
                    let now_origin = initial_ns.zip(tsc_hz);
                    let origin_valid = now_origin.is_some_and(|origin| {
                        clock_origin.is_none_or(|prior_origin| prior_origin == origin)
                    });
                    let retired_ns = clock.get("retired_ns_charged").and_then(Value::as_u64);
                    let elapsed_ns = initial_ns
                        .zip(now_ns)
                        .and_then(|(initial, now)| now.checked_sub(initial));
                    let charged_ns = retired_ns
                        .zip(clock.get("pm_timer_ns_charged").and_then(Value::as_u64))
                        .and_then(|(retired, pm)| retired.checked_add(pm))
                        .zip(clock.get("pit_ns_charged").and_then(Value::as_u64))
                        .and_then(|(charged, pit)| charged.checked_add(pit))
                        .zip(clock.get("rdtsc_ns_charged").and_then(Value::as_u64))
                        .and_then(|(charged, rdtsc)| charged.checked_add(rdtsc))
                        .zip(clock.get("idle_ns_advanced").and_then(Value::as_u64))
                        .and_then(|(charged, idle)| charged.checked_add(idle));
                    let expected_unaccounted = elapsed_ns
                        .zip(charged_ns)
                        .and_then(|(elapsed, charged)| elapsed.checked_sub(charged));
                    let unaccounted_matches = clock
                        .get("unaccounted_elapsed_ns")
                        .and_then(|value| {
                            if value.is_null() {
                                Some(None)
                            } else {
                                value.as_u64().map(Some)
                            }
                        })
                        .is_some_and(|actual| actual == expected_unaccounted);
                    let sum_valid = charged
                        .zip(quantum)
                        .and_then(|(charged, quantum)| charged.checked_add(quantum))
                        .zip(through)
                        .is_some_and(|(sum, through)| sum == through);
                    clock.get("rip").and_then(Value::as_str) == rip
                        && clock.get("hit_ordinal").and_then(Value::as_u64) == Some(ordinal)
                        && callback_tsc
                        && initial_ns
                            .zip(now_ns)
                            .is_some_and(|(initial, now)| now >= initial)
                        && origin_valid
                        && tsc_hz.is_some_and(|frequency| frequency > 0)
                        && numeric_fields_valid
                        && retired_ns_valid
                        && unaccounted_valid
                        && unaccounted_matches
                        && sum_valid
                });
            if ordinal <= previous
                || ordinal > event_count
                || rip.is_none_or(|rip| !canonical_hex64(rip))
                || !cpl_valid
                || !context_valid
                || !faults_valid
                || !clock_valid
            {
                return Err("execution-observations RDTSC prefix sample is invalid".into());
            }
            previous = ordinal;
            previous_page_faults = page_faults.unwrap_or(previous_page_faults);
            previous_general_protection_faults =
                general_protection_faults.unwrap_or(previous_general_protection_faults);
            if let (Some(initial), Some(frequency)) = (
                sample["clock"]
                    .get("initial_now_ns")
                    .and_then(Value::as_u64),
                sample["clock"].get("tsc_hz").and_then(Value::as_u64),
            ) {
                clock_origin = Some((initial, frequency));
            }
        }
        let status_valid = match status {
            "not_requested" => {
                matches!(metadata_sentinel, Some(Value::Null))
                    && event_count == 0
                    && missing == 0
                    && raw_samples.is_empty()
            }
            "sentinel_seen" => {
                let sentinel_samples = raw_samples
                    .iter()
                    .filter(|sample| {
                        sample.get("rip").and_then(Value::as_str) == Some("0xffffffff8271b31b")
                    })
                    .count();
                sentinel_requested
                    && event_count > 0
                    && retained_and_missing == event_count
                    && (sentinel_samples == 1
                        && raw_samples.last().is_some_and(|sample| {
                            sample.get("rip").and_then(Value::as_str) == Some("0xffffffff8271b31b")
                                && sample.get("ordinal").and_then(Value::as_u64)
                                    == Some(event_count)
                        })
                        || (sentinel_samples == 0
                            && missing > 0
                            && raw_samples.last().is_none_or(|sample| {
                                sample.get("ordinal").and_then(Value::as_u64) < Some(event_count)
                            })))
            }
            "capacity_exhausted" => {
                sentinel_requested
                    && event_count == od_machine::LINUX_RDTSC_PREFIX_CAPACITY as u64
                    && retained_and_missing >= event_count
                    && raw_samples.iter().all(|sample| {
                        sample.get("rip").and_then(Value::as_str) != Some("0xffffffff8271b31b")
                    })
            }
            "run_ended_before_sentinel" => {
                sentinel_requested
                    && event_count < od_machine::LINUX_RDTSC_PREFIX_CAPACITY as u64
                    && retained_and_missing == event_count
                    && raw_samples.iter().all(|sample| {
                        sample.get("rip").and_then(Value::as_str) != Some("0xffffffff8271b31b")
                    })
            }
            _ => false,
        };
        if !status_valid {
            return Err(
                "execution-observations RDTSC prefix termination status is inconsistent".into(),
            );
        }
    }
    if schema_version >= 11 {
        let samples = sidecar_json["rdtsc_entry_clock_trace_samples"]
            .as_array()
            .ok_or_else(|| {
                "execution-observations clock trace is missing or invalid".to_string()
            })?;
        for sample in samples {
            let charged = sample
                .get("retired_instructions_charged")
                .and_then(Value::as_u64);
            let quantum = sample
                .get("quantum_instructions_executed")
                .and_then(Value::as_u64);
            let through_rdtsc = sample
                .get("retired_instructions_through_rdtsc")
                .and_then(Value::as_u64);
            let sum = charged
                .zip(quantum)
                .and_then(|(charged, quantum)| charged.checked_add(quantum));
            if sum.is_none() || sum != through_rdtsc {
                return Err(
                    "execution-observations clock trace instruction counts are invalid".into(),
                );
            }
        }
    }
    if schema_version >= 13 {
        let raw_trace = sidecar_json.get("instruction_path_trace");
        let metadata_status = sidecar.get("instruction_path_trace_status");
        let metadata_count = sidecar.get("instruction_path_trace_record_count");
        let metadata_omitted = sidecar.get("instruction_path_trace_omitted_records");
        if matches!(raw_trace, Some(Value::Null)) {
            if metadata_status != Some(&Value::Null)
                || metadata_count != Some(&Value::Null)
                || metadata_omitted != Some(&Value::Null)
            {
                return Err(
                    "execution-observations instruction trace metadata is inconsistent".into(),
                );
            }
        } else {
            let trace = raw_trace.and_then(Value::as_object).ok_or_else(|| {
                "execution-observations instruction trace is missing or invalid".to_string()
            })?;
            let status = trace.get("status").and_then(Value::as_str);
            let records = trace.get("records").and_then(Value::as_array);
            let count = records.map(|records| records.len() as u64);
            let omitted = trace.get("omitted_records").and_then(Value::as_u64);
            let quantum = trace
                .get("quantum_instructions_executed")
                .and_then(Value::as_u64);
            let qordinal = trace.get("quantum_ordinal").and_then(Value::as_u64);
            let charged = trace
                .get("retired_instructions_charged")
                .and_then(Value::as_u64);
            let hits_before = trace
                .get("rdtsc_cpl0_hits_before_dispatch")
                .and_then(Value::as_u64);
            let hits_after = trace
                .get("rdtsc_cpl0_hits_after_quantum")
                .and_then(Value::as_u64);
            let requested = trace.get("requested").and_then(Value::as_bool);
            let header = trace.get("header");
            let valid_status = matches!(
                status,
                Some(
                    "captured"
                        | "gate_mismatch"
                        | "missed_start"
                        | "missed_end"
                        | "capacity_exhausted"
                )
            );
            let active_gate_matches = qordinal
                == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_QUANTUM)
                && charged == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE)
                && hits_before == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_START_HITS);
            let status_gate_valid = match status {
                Some("gate_mismatch") => {
                    qordinal.is_some_and(|value| {
                        value >= od_machine::LINUX_INSTRUCTION_PATH_TRACE_QUANTUM
                    }) && charged.is_some()
                        && hits_before.is_some()
                        && !active_gate_matches
                        && count == Some(0)
                        && omitted == Some(0)
                        && header.is_some_and(Value::is_null)
                        && quantum.is_none()
                        && hits_after.is_none()
                }
                Some("missed_start") => {
                    (active_gate_matches
                        || (qordinal.is_none()
                            && charged.is_none()
                            && hits_before.is_none()
                            && quantum.is_none()
                            && hits_after.is_none()))
                        && count == Some(0)
                        && omitted == Some(0)
                        && header.is_some_and(Value::is_null)
                }
                Some("captured" | "missed_end" | "capacity_exhausted") => active_gate_matches,
                _ => false,
            };
            let header_valid = header.is_some_and(|header| {
                header.is_null() || valid_instruction_path_trace_header(header)
            });
            let first_record_matches_header = header
                .and_then(|header| header.get("rip"))
                .and_then(Value::as_str)
                .zip(records.and_then(|records| records.first()))
                .is_none_or(|(header_rip, first)| {
                    first.get("rip").and_then(Value::as_str) == Some(header_rip)
                });
            if !valid_status
                || requested != Some(true)
                || count.is_none()
                || omitted.is_none()
                || count.unwrap_or(u64::MAX)
                    > od_machine::LINUX_INSTRUCTION_PATH_TRACE_CAPACITY as u64
                || metadata_status != trace.get("status")
                || metadata_count.and_then(Value::as_u64) != count
                || metadata_omitted.and_then(Value::as_u64) != omitted
                || !status_gate_valid
                || !header_valid
                || !first_record_matches_header
                || records
                    .unwrap()
                    .iter()
                    .any(|record| !valid_instruction_path_trace_record(record))
            {
                return Err(
                    "execution-observations instruction trace summary or record is invalid".into(),
                );
            }
            match status.unwrap() {
                "captured"
                    if header.is_some_and(valid_instruction_path_trace_header)
                        && quantum == count
                        && omitted == Some(0)
                        && qordinal == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_QUANTUM)
                        && charged
                            == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE)
                        && hits_before
                            == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_START_HITS)
                        && hits_after == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_END_HIT)
                        && records.is_some_and(|records| records.len() > 0) => {}
                "captured" => {
                    return Err(
                        "complete instruction trace does not satisfy its gate or end marker".into(),
                    );
                }
                "capacity_exhausted"
                    if count == Some(od_machine::LINUX_INSTRUCTION_PATH_TRACE_CAPACITY as u64)
                        && omitted.is_some_and(|value| value > 0) => {}
                "capacity_exhausted" => {
                    return Err("instruction trace capacity status is inconsistent".into());
                }
                "missed_start" if !header.is_some_and(Value::is_object) && count == Some(0) => {}
                "missed_start" => {
                    return Err("instruction trace missed-start status is inconsistent".into());
                }
                _ => {}
            }
        }
    }
    if schema_version >= 14 {
        let raw = sidecar_json.get("elf_stack_copy");
        let metadata_status = sidecar.get("elf_stack_copy_status");
        match raw {
            Some(Value::Null) => {
                if metadata_status != Some(&Value::Null) {
                    return Err(
                        "execution-observations ELF stack copy metadata is inconsistent".into(),
                    );
                }
            }
            Some(Value::Object(snapshot)) => {
                let status = snapshot.get("status").and_then(Value::as_str);
                let probes = snapshot.get("probes").and_then(Value::as_array);
                let mismatch_reasons = snapshot.get("mismatch_reasons").and_then(Value::as_array);
                let error = snapshot.get("error");
                let valid_status = matches!(
                    status,
                    Some("captured" | "gate_mismatch" | "read_error" | "run_ended_before_target")
                );
                let valid_hex = |value: Option<&Value>| match value {
                    Some(Value::Null) => true,
                    Some(Value::String(hex)) => canonical_hex64(hex),
                    _ => false,
                };
                let probes_valid = probes.is_some_and(|probes| {
                    elf_stack_copy_probe_count_valid(probes)
                        && probes.iter().all(|probe| {
                            let address = probe.get("address").and_then(Value::as_str);
                            let page_base = probe.get("page_base").and_then(Value::as_str);
                            let access = probe.get("access").and_then(Value::as_str);
                            let outcome = probe.get("outcome").and_then(Value::as_str);
                            let entries = probe.get("entries").and_then(Value::as_array);
                            let translated = probe.get("translated_physical");
                            let error = probe.get("error");
                            let access_ok = matches!(access, Some("Read" | "Write"));
                            let outcome_ok = match outcome {
                                Some("translated") => {
                                    valid_hex(translated)
                                        && translated.is_some_and(|v| v.is_string())
                                        && error.is_some_and(Value::is_null)
                                }
                                Some("page_fault" | "error") => {
                                    translated.is_some_and(Value::is_null)
                                        && error.is_some_and(Value::is_string)
                                }
                                _ => false,
                            };
                            address.is_some_and(canonical_hex64)
                                && page_base.is_some_and(canonical_hex64)
                                && address.zip(page_base).is_some_and(|(a, p)| {
                                    u64::from_str_radix(a.trim_start_matches("0x"), 16)
                                        .ok()
                                        .zip(
                                            u64::from_str_radix(p.trim_start_matches("0x"), 16)
                                                .ok(),
                                        )
                                        .is_some_and(|(a, p)| p == (a & !0xfff))
                                })
                                && access_ok
                                && probe
                                    .get("cpl")
                                    .and_then(Value::as_u64)
                                    .is_some_and(|cpl| cpl <= 3)
                                && entries.is_some_and(|entries| {
                                    entries.len()
                                        <= od_machine::LINUX_ELF_STACK_COPY_WALK_ENTRY_CAPACITY
                                        && entries.iter().all(valid_elf_stack_copy_walk_entry)
                                })
                                && outcome_ok
                        })
                });
                let named_probes_valid = probes.is_some_and(|probes| {
                    let parse_hex = |value: Option<&Value>| {
                        value
                            .and_then(Value::as_str)
                            .and_then(|hex| hex.strip_prefix("0x"))
                            .and_then(|digits| u64::from_str_radix(digits, 16).ok())
                    };
                    let registers = snapshot.get("registers").and_then(Value::as_object);
                    let cpl = snapshot.get("cpl").and_then(Value::as_u64);
                    let expected_addresses = [
                        parse_hex(snapshot.get("bprm_p")),
                        registers.and_then(|registers| parse_hex(registers.get("rsi"))),
                        registers.and_then(|registers| parse_hex(registers.get("rdi"))),
                    ];
                    probes.len() == 3
                        && [
                            ("stack_input", "Read"),
                            ("copy_source", "Read"),
                            ("copy_destination", "Write"),
                        ]
                        .iter()
                        .zip(probes.iter().zip(expected_addresses))
                        .all(
                            |((name, access), (probe, expected_address))| {
                                probe.get("name").and_then(Value::as_str) == Some(*name)
                                    && probe.get("access").and_then(Value::as_str) == Some(*access)
                                    && probe.get("cpl").and_then(Value::as_u64) == cpl
                                    && parse_hex(probe.get("address")) == expected_address
                            },
                        )
                });
                let snapshot_has_complete_capture =
                    [
                        "rip",
                        "cr3",
                        "bprm_p_address",
                        "bprm_p",
                        "aligned_stack_pointer",
                        "copy_destination",
                        "copy_length",
                    ]
                    .iter()
                    .all(|key| {
                        snapshot
                            .get(*key)
                            .and_then(Value::as_str)
                            .is_some_and(canonical_hex64)
                    }) && snapshot.get("cpl").and_then(Value::as_u64) == Some(0)
                        && snapshot
                            .get("cs_selector")
                            .and_then(Value::as_str)
                            .and_then(|selector| selector.strip_prefix("0x"))
                            .and_then(|digits| u16::from_str_radix(digits, 16).ok())
                            .is_some_and(|selector| selector & 3 == 0)
                        && snapshot.get("instruction_bytes").and_then(Value::as_str)
                            == Some("e847d93400")
                        && snapshot
                            .get("registers")
                            .and_then(Value::as_object)
                            .is_some_and(|registers| {
                                ["r15", "r13", "rdi", "rsi", "rdx"].iter().all(|key| {
                                    registers
                                        .get(*key)
                                        .and_then(Value::as_str)
                                        .is_some_and(canonical_hex64)
                                })
                            });
                let invocation_valid =
                    schema_version < 16
                        || (snapshot.get("platform_string_matches") == Some(&json!(true))
                            && snapshot
                                .get("return_rip")
                                .and_then(Value::as_str)
                                .is_some_and(canonical_hex64)
                            && snapshot.get("return_rip").and_then(Value::as_str).and_then(
                                |hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok(),
                            ) == snapshot
                                .get("rip")
                                .and_then(Value::as_str)
                                .and_then(|hex| {
                                    u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok()
                                })
                                .and_then(|rip| rip.checked_add(5))
                            && snapshot.get("invocation").is_some_and(|invocation| {
                                valid_elf_stack_copy_invocation(invocation, snapshot, runner)
                            }));
                let pre_copy_cr3_ledger_valid = schema_version < 17
                    || snapshot.get("pre_copy_cr3_ledger").is_some_and(|ledger| {
                        valid_elf_stack_copy_pre_copy_cr3_ledger_with_runner(
                            ledger,
                            snapshot,
                            Some(runner),
                        ) && (schema_version < 18 || valid_elf_stack_copy_target_cr3_event(ledger))
                            && (schema_version < 19
                                || valid_elf_mmu_gather_capture(
                                    ledger,
                                    runner,
                                    snapshot.get("invocation"),
                                    schema_version,
                                    snapshot.get("copy_destination"),
                                ))
                    });
                let alignment_valid = schema_version < 21
                    || snapshot.get("alignment").is_some_and(|alignment| {
                        valid_elf_stack_alignment_capture(alignment, snapshot, runner)
                    });
                let status_data_valid = match status {
                    Some("captured") => {
                        snapshot_has_complete_capture
                            && invocation_valid
                            && pre_copy_cr3_ledger_valid
                            && alignment_valid
                            && elf_stack_copy_gate_matches(snapshot)
                            && mismatch_reasons.is_some_and(Vec::is_empty)
                            && error.is_some_and(Value::is_null)
                            && named_probes_valid
                            && probes_valid
                    }
                    Some("gate_mismatch") => {
                        let invocation_gate_mismatch = schema_version >= 16
                            && mismatch_reasons.is_some_and(|reasons| {
                                reasons.iter().any(|reason| {
                                    matches!(
                                        reason.as_str(),
                                        Some(
                                            "platform_string_mismatch"
                                                | "platform_call_gate_incomplete"
                                        )
                                    )
                                })
                            });
                        mismatch_reasons.is_some_and(|reasons| !reasons.is_empty())
                            && pre_copy_cr3_ledger_valid
                            && alignment_valid
                            && (!elf_stack_copy_gate_matches(snapshot) || invocation_gate_mismatch)
                            && probes.is_some_and(Vec::is_empty)
                            && error.is_some_and(Value::is_null)
                    }
                    Some("read_error") => {
                        pre_copy_cr3_ledger_valid
                            && alignment_valid
                            && error.is_some_and(Value::is_string)
                            && probes.is_some_and(Vec::is_empty)
                    }
                    Some("run_ended_before_target") => {
                        pre_copy_cr3_ledger_valid
                            && alignment_valid
                            && probes.is_some_and(Vec::is_empty)
                            && mismatch_reasons.is_some_and(Vec::is_empty)
                            && error.is_some_and(Value::is_null)
                    }
                    _ => false,
                };
                if snapshot.get("profile").and_then(Value::as_str)
                    != Some(od_machine::LINUX_ELF_STACK_COPY_PROFILE)
                    || !valid_status
                    || !status_data_valid
                    || metadata_status.and_then(Value::as_str) != status
                {
                    return Err("execution-observations ELF stack copy snapshot is invalid".into());
                }
            }
            _ => {
                return Err(
                    "execution-observations ELF stack copy snapshot is missing or invalid".into(),
                );
            }
        }
    }
    if schema_version >= 15 && !valid_delay_tsc_call_summary(&sidecar_json, sidecar) {
        return Err("execution-observations delay_tsc call summary is inconsistent".into());
    }
    Ok(())
}

fn valid_elf_stack_alignment_capture(
    capture: &Value,
    copy: &serde_json::Map<String, Value>,
    runner: &Value,
) -> bool {
    let Some(capture) = capture.as_object() else {
        return false;
    };
    if capture.len() != 6 {
        return false;
    }
    let Some(status) = capture.get("status").and_then(Value::as_str) else {
        return false;
    };
    let before = capture.get("before_call");
    let after = capture.get("after_return");
    let (Some(before_omitted), Some(after_omitted), Some(matched)) = (
        capture.get("before_omitted").and_then(Value::as_u64),
        capture.get("after_omitted").and_then(Value::as_u64),
        capture.get("matched_copy"),
    ) else {
        return false;
    };
    if !(matched.is_null() || matched.is_boolean()) {
        return false;
    }
    let validate_point = |value: Option<&Value>, expected_rip: u64| {
        let Some(point) = value.and_then(Value::as_object) else {
            return value.is_some_and(Value::is_null);
        };
        let hex = |key: &str| {
            point
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| canonical_hex64(value))
                .and_then(|value| u64::from_str_radix(value.trim_start_matches("0x"), 16).ok())
        };
        let selector_cpl_coherent = match (point.get("cs_selector"), point.get("cpl")) {
            (Some(Value::Null), Some(Value::Null)) => true,
            (Some(Value::String(selector)), Some(cpl)) => {
                selector.len() == 6
                    && selector.starts_with("0x")
                    && u16::from_str_radix(&selector[2..], 16).is_ok_and(|selector_value| {
                        format!("0x{selector_value:04x}") == *selector
                            && cpl.as_u64() == Some((selector_value & 3) as u64)
                    })
            }
            _ => false,
        };
        point.len() == 11
            && hex("rip") == Some(expected_rip)
            && point
                .get("retired_instructions_before_current")
                .is_some_and(Value::is_u64)
            && hex("cr3").is_some()
            && selector_cpl_coherent
            && point
                .get("cpl")
                .is_some_and(|value| value.is_null() || value.as_u64().is_some_and(|cpl| cpl <= 3))
            && hex("r15").is_some()
            && hex("rbp").is_some()
            && hex("rsp").is_some()
            && hex("pointer").is_some()
            && point.get("instruction_bytes").is_some_and(|value| {
                value.is_null()
                    || value.as_str().is_some_and(|bytes| {
                        bytes.len() == 10
                            && bytes.bytes().all(|byte| byte.is_ascii_hexdigit())
                            && bytes == bytes.to_ascii_lowercase()
                    })
            })
            && point
                .get("error")
                .is_some_and(|value| value.is_null() || value.is_string())
    };
    if !validate_point(before, od_machine::LINUX_ELF_STACK_ALIGN_BEFORE_RIP)
        || !validate_point(after, od_machine::LINUX_ELF_STACK_ALIGN_AFTER_RIP)
    {
        return false;
    }
    let point_hex = |value: Option<&Value>, key: &str| {
        value
            .and_then(Value::as_object)
            .and_then(|point| point.get(key))
            .and_then(Value::as_str)
            .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
    };
    let point_selector = |value: Option<&Value>| {
        value
            .and_then(|point| point.get("cs_selector"))
            .and_then(Value::as_str)
            .and_then(|selector| u16::from_str_radix(selector.trim_start_matches("0x"), 16).ok())
    };
    let point_cpl = |value: Option<&Value>| {
        value
            .and_then(|point| point.get("cpl"))
            .and_then(Value::as_u64)
    };
    let before_present = before.is_some_and(Value::is_object);
    let after_present = after.is_some_and(Value::is_object);
    let both_present = before_present && after_present;
    let same_frame = both_present
        && point_hex(before, "cr3") == point_hex(after, "cr3")
        && before.and_then(|point| point.get("cs_selector"))
            == after.and_then(|point| point.get("cs_selector"))
        && before.and_then(|point| point.get("cpl")) == Some(&json!(0))
        && after.and_then(|point| point.get("cpl")) == Some(&json!(0))
        && point_selector(before).is_some_and(|selector| selector & 3 == 0)
        && point_selector(before) == point_selector(after)
        && point_cpl(before) == point_selector(before).map(|selector| (selector & 3) as u64)
        && point_cpl(after) == point_selector(after).map(|selector| (selector & 3) as u64)
        && ["r15", "rbp", "rsp"]
            .iter()
            .all(|key| point_hex(before, key) == point_hex(after, key))
        && after
            .and_then(|point| point.get("retired_instructions_before_current"))
            .and_then(Value::as_u64)
            > before
                .and_then(|point| point.get("retired_instructions_before_current"))
                .and_then(Value::as_u64);
    let registers = copy.get("registers").and_then(Value::as_object);
    let copy_status = copy.get("status").and_then(Value::as_str);
    let target_seen = copy_status != Some("run_ended_before_target");
    let copy_registers_canonical = registers.is_some_and(|registers| {
        ["r15", "rbp", "rsp", "r13"].iter().all(|key| {
            registers.get(*key).is_some_and(|value| {
                if target_seen {
                    value.as_str().is_some_and(canonical_hex64)
                } else {
                    value.is_null()
                }
            })
        })
    });
    let copy_selector_cpl_coherent = match (copy.get("cs_selector"), copy.get("cpl")) {
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::String(selector)), Some(cpl)) if target_seen => {
            selector.len() == 6
                && selector.starts_with("0x")
                && u16::from_str_radix(&selector[2..], 16).is_ok_and(|selector_value| {
                    format!("0x{selector_value:04x}") == *selector
                        && cpl.as_u64() == Some((selector_value & 3) as u64)
                })
        }
        _ => false,
    };
    if !copy_registers_canonical || !copy_selector_cpl_coherent {
        return false;
    }
    let copy_selector = copy
        .get("cs_selector")
        .and_then(Value::as_str)
        .and_then(|selector| u16::from_str_radix(selector.trim_start_matches("0x"), 16).ok());
    let copy_matches = copy_status == Some("captured")
        && copy.get("platform_string_matches") == Some(&json!(true))
        && copy_registers_canonical
        && copy_selector.is_some_and(|selector| selector & 3 == 0)
        && point_selector(before) == copy_selector
        && both_present
        && ["r15", "rbp", "rsp"].iter().all(|key| {
            point_hex(before, key)
                == registers
                    .and_then(|registers| registers.get(*key))
                    .and_then(Value::as_str)
                    .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
        })
        && point_hex(before, "cr3")
            == copy
                .get("cr3")
                .and_then(Value::as_str)
                .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
        && point_hex(before, "pointer")
            == copy
                .get("bprm_p")
                .and_then(Value::as_str)
                .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
        && point_hex(after, "pointer")
            == registers
                .and_then(|registers| registers.get("r13"))
                .and_then(Value::as_str)
                .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());
    let after_retired = after
        .and_then(|point| point.get("retired_instructions_before_current"))
        .and_then(Value::as_u64);
    let copy_entry_retired = copy
        .get("invocation")
        .and_then(|invocation| invocation.get("entry"))
        .and_then(|entry| entry.get("retired_instructions_through_current"))
        .and_then(Value::as_u64);
    let stop_retired = runner
        .get("stop")
        .and_then(|stop| stop.get("retired"))
        .and_then(Value::as_u64);
    let retired_precedes_copy = after_retired
        .zip(copy_entry_retired)
        .zip(stop_retired)
        .is_some_and(|((after, copy), stop)| after < copy && copy <= stop);
    let expected_bytes = before.and_then(|point| point.get("instruction_bytes"))
        == Some(&json!("e8f69aa1ff"))
        && after.and_then(|point| point.get("instruction_bytes")) == Some(&json!("ba07000000"));
    let has_read_error = [before, after]
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .any(|point| point.get("error").is_some_and(Value::is_string));
    let expected_status = if before_omitted > 0 || after_omitted > 0 {
        "ambiguous"
    } else if !both_present {
        if target_seen {
            "incomplete"
        } else {
            "run_ended_before_target"
        }
    } else if has_read_error {
        "read_error"
    } else if !expected_bytes {
        "anchor_mismatch"
    } else if !same_frame || (target_seen && (!copy_matches || !retired_precedes_copy)) {
        "invocation_mismatch"
    } else if !target_seen {
        "copy_target_not_seen"
    } else {
        "captured"
    };
    let expected_matched = if expected_status == "captured" {
        json!(true)
    } else if expected_status == "invocation_mismatch" && target_seen {
        json!(false)
    } else {
        Value::Null
    };
    status == expected_status && matched == &expected_matched
}

fn supported_execution_observations_schema_version(version: u64) -> bool {
    (1..=21).contains(&version)
}

fn valid_elf_mmu_gather_capture(
    ledger: &Value,
    runner: &Value,
    copy_invocation: Option<&Value>,
    schema_version: u64,
    outer_copy_destination: Option<&Value>,
) -> bool {
    let Some(capture) = ledger.get("mmu_gather") else {
        return false;
    };
    let Some(status) = capture.get("status").and_then(Value::as_str) else {
        return false;
    };
    let v20 = schema_version >= 20;
    if !matches!(status, "not_seen" | "captured" | "incomplete" | "truncated")
        || capture.get("association").and_then(Value::as_str) != Some("prefix_candidates")
        || !capture
            .as_object()
            .is_some_and(|object| object.len() == if v20 { 9 } else { 7 })
    {
        return false;
    }
    let (Some(setups), Some(unpaired)) = (
        capture.get("setups").and_then(Value::as_array),
        capture.get("unpaired").and_then(Value::as_array),
    ) else {
        return false;
    };
    let (Some(setup_count), Some(setups_omitted), Some(unpaired_omitted)) = (
        capture.get("setup_count").and_then(Value::as_u64),
        capture.get("setups_omitted").and_then(Value::as_u64),
        capture.get("unpaired_omitted").and_then(Value::as_u64),
    ) else {
        return false;
    };
    let Some(observed_plus_omitted) = (setups.len() as u64).checked_add(setups_omitted) else {
        return false;
    };
    if setup_count != observed_plus_omitted
        || setups.len() > od_machine::LINUX_ELF_MMU_GATHER_SETUP_CAPACITY
        || unpaired.len() > od_machine::LINUX_ELF_MMU_GATHER_SETUP_CAPACITY
        || (setups_omitted > 0 && setups.len() != od_machine::LINUX_ELF_MMU_GATHER_SETUP_CAPACITY)
        || (unpaired_omitted > 0
            && unpaired.len() != od_machine::LINUX_ELF_MMU_GATHER_SETUP_CAPACITY)
        || ((setups_omitted > 0 || unpaired_omitted > 0) != (status == "truncated"))
    {
        return false;
    }
    if status == "not_seen"
        && (setup_count != 0
            || setups_omitted != 0
            || !setups.is_empty()
            || unpaired_omitted != 0
            || !unpaired.is_empty())
    {
        return false;
    }
    if v20 {
        let Some(outer_copy_destination) = outer_copy_destination else {
            return false;
        };
        let (Some(destination), Some(destination_match)) = (
            capture.get("copy_destination"),
            capture.get("destination_in_candidate_pages"),
        ) else {
            return false;
        };
        if !valid_nullable_hex64(outer_copy_destination)
            || destination != outer_copy_destination
            || !valid_nullable_hex64(destination)
            || (!destination.is_null() && !destination_match.is_boolean())
            || (destination.is_null() && !destination_match.is_null())
        {
            return false;
        }
    }
    if matches!(status, "captured" | "incomplete") && setup_count == 0 && unpaired.is_empty() {
        return false;
    }

    let stop = runner.get("stop").unwrap_or(&Value::Null);
    let stop_counters = (
        stop.get("retired").and_then(Value::as_u64),
        stop.get("cr3_changes").and_then(Value::as_u64),
        stop.get("page_faults_delivered").and_then(Value::as_u64),
    );
    if stop_counters.0.is_none() || stop_counters.1.is_none() || stop_counters.2.is_none() {
        return false;
    }
    let Some(ledger_event_count) = ledger.get("event_count").and_then(Value::as_u64) else {
        return false;
    };
    let entry = copy_invocation
        .and_then(|invocation| invocation.get("entry"))
        .filter(|entry| !entry.is_null());
    let entry_counters = if let Some(entry) = entry {
        let (Some(retired), Some(cr3_changes), Some(page_faults)) = (
            entry
                .get("retired_instructions_through_current")
                .and_then(Value::as_u64),
            entry.get("cr3_changes").and_then(Value::as_u64),
            entry.get("delivered_page_faults").and_then(Value::as_u64),
        ) else {
            return false;
        };
        Some((retired, cr3_changes, page_faults))
    } else {
        None
    };
    let within_runner_bounds = |context: &Value| {
        let counters = (
            context.get("retired_instructions").and_then(Value::as_u64),
            context.get("cr3_changes").and_then(Value::as_u64),
            context.get("page_faults").and_then(Value::as_u64),
        );
        let (Some(retired), Some(cr3_changes), Some(page_faults)) = counters else {
            return false;
        };
        retired <= stop_counters.0.unwrap()
            && cr3_changes <= stop_counters.1.unwrap()
            && page_faults <= stop_counters.2.unwrap()
            && cr3_changes <= ledger_event_count
            && entry_counters.is_none_or(|(entry_retired, entry_cr3, entry_faults)| {
                retired <= entry_retired && cr3_changes <= entry_cr3 && page_faults <= entry_faults
            })
    };

    for (index, invocation) in setups.iter().enumerate() {
        if !invocation
            .as_object()
            .is_some_and(|object| object.len() == if v20 { 11 } else { 8 })
        {
            return false;
        }
        let Some(sequence) = invocation.get("sequence").and_then(Value::as_u64) else {
            return false;
        };
        if sequence != index as u64 + 1 {
            return false;
        }
        let Some(closure) = invocation.get("closure").and_then(Value::as_str) else {
            return false;
        };
        if !matches!(closure, "active" | "returned" | "superseded" | "run_ended") {
            return false;
        }
        let setup_phase = invocation.get("setup");
        let initial_phase = invocation.get("initial");
        let actual_phase = invocation.get("actual_post");
        let returned_phase = invocation.get("returned");
        if !valid_mmu_phase(
            setup_phase,
            0xffff_ffff_818f_24d5,
            "value",
            "partial",
            |value| {
                if v20 {
                    valid_mmu_setup_data_v20(value)
                } else {
                    valid_mmu_setup_data(value)
                }
            },
        ) || !valid_mmu_phase(
            initial_phase,
            0xffff_ffff_8182_bfdb,
            "value",
            "partial",
            valid_mmu_gather_raw,
        ) || !valid_mmu_phase(
            actual_phase,
            0xffff_ffff_8180_9fa1,
            "value",
            "partial",
            valid_mmu_gather_raw,
        ) || !valid_mmu_phase(
            returned_phase,
            0xffff_ffff_8182_bfe0,
            "value",
            "partial",
            |v| v == &json!(true),
        ) {
            return false;
        }
        let Some(setup_context) = mmu_phase_context(setup_phase) else {
            return false;
        };
        let setup_status = setup_phase
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
            .unwrap();
        if setup_status == "captured" {
            if !setup_context
                .get("saved_rbp")
                .is_some_and(|v| v.as_str().is_some_and(canonical_hex64))
                || !setup_context
                    .get("saved_return_rip")
                    .is_some_and(|v| v.as_str().is_some_and(canonical_hex64))
                || setup_phase
                    .and_then(|v| v.get("value"))
                    .and_then(|v| v.get("vm_start"))
                    .and_then(Value::as_str)
                    .is_none_or(|v| !canonical_hex64(v))
                || setup_phase
                    .and_then(|v| v.get("value"))
                    .and_then(|v| v.get("vm_end"))
                    .and_then(Value::as_str)
                    .is_none_or(|v| !canonical_hex64(v))
            {
                return false;
            }
        }
        let setup_candidates = if v20 {
            let Some(addresses) = serialized_mmu_candidate_pages(setup_phase) else {
                return false;
            };
            if !valid_mmu_gather_page_walks(
                &invocation["setup_page_walks"],
                addresses,
                mmu_phase_cr3(setup_phase),
            ) {
                return false;
            }
            Some(addresses)
        } else {
            None
        };
        let initial_status = initial_phase
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
            .unwrap();
        let initial_context = mmu_phase_context(initial_phase);
        if initial_status != "not_observed" {
            let Some(context) = initial_context else {
                return false;
            };
            if context.get("saved_rbp") != setup_context.get("rbp")
                || context.get("saved_return_rip") != Some(&json!("0xffffffff818f24e5"))
                || context
                    .get("tlb_pointer")
                    .and_then(Value::as_str)
                    .is_none_or(|v| !canonical_hex64(v))
            {
                return false;
            }
            if let Some(raw) = mmu_phase_raw(initial_phase) {
                if context.get("tlb_pointer") != raw.get("tlb") {
                    return false;
                }
            }
        }
        let initial_walks_complete = if v20 {
            let Some(addresses) = setup_candidates else {
                return false;
            };
            let Some(walks) = invocation.get("initial_page_walks") else {
                return false;
            };
            if initial_status == "not_observed" {
                if !walks.is_null() {
                    return false;
                }
                false
            } else if !valid_mmu_gather_page_walks(walks, addresses, mmu_phase_cr3(initial_phase)) {
                return false;
            } else {
                mmu_walks_are_complete(walks, addresses, mmu_phase_cr3(initial_phase))
            }
        } else {
            false
        };
        let actual_status = actual_phase
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
            .unwrap();
        let actual_context = mmu_phase_context(actual_phase);
        if actual_status != "not_observed" {
            let (Some(context), Some(initial_context)) = (actual_context, initial_context) else {
                return false;
            };
            if context.get("saved_rbp") != initial_context.get("rbp")
                || context.get("saved_return_rip") != Some(&json!("0xffffffff8182bfe0"))
                || context.get("tlb_pointer") != initial_context.get("tlb_pointer")
            {
                return false;
            }
            if let Some(raw) = mmu_phase_raw(actual_phase) {
                if context.get("tlb_pointer") != raw.get("tlb") {
                    return false;
                }
            }
        }
        let Some(association_mismatch) = invocation
            .get("association_mismatch")
            .and_then(Value::as_bool)
        else {
            return false;
        };
        let captured_mm_mismatch = if initial_status == "captured" && actual_status == "captured" {
            let (Some(initial_raw), Some(actual_raw)) =
                (mmu_phase_raw(initial_phase), mmu_phase_raw(actual_phase))
            else {
                return false;
            };
            initial_raw.get("tlb") == actual_raw.get("tlb")
                && initial_raw.get("mm") != actual_raw.get("mm")
        } else {
            false
        };
        if association_mismatch != captured_mm_mismatch {
            return false;
        }
        let returned_status = returned_phase
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
            .unwrap();
        if returned_status == "captured" {
            let (Some(return_context), Some(initial_context)) =
                (mmu_phase_context(returned_phase), initial_context)
            else {
                return false;
            };
            if return_context.get("rbp") != initial_context.get("rbp")
                || return_context.get("rsp") != initial_context.get("rsp")
                || return_context.get("saved_rbp") != initial_context.get("saved_rbp")
                || return_context.get("saved_return_rip") != initial_context.get("saved_return_rip")
                || closure != "returned"
            {
                return false;
            }
        } else if closure == "returned" {
            return false;
        }
        let return_walks_complete = if v20 {
            let Some(addresses) = setup_candidates else {
                return false;
            };
            let Some(walks) = invocation.get("return_page_walks") else {
                return false;
            };
            if returned_status == "not_observed" {
                if !walks.is_null() {
                    return false;
                }
                false
            } else if !valid_mmu_gather_page_walks(walks, addresses, mmu_phase_cr3(returned_phase))
            {
                return false;
            } else {
                mmu_walks_are_complete(walks, addresses, mmu_phase_cr3(returned_phase))
            }
        } else {
            false
        };

        let contexts = [
            mmu_phase_context(setup_phase),
            initial_context,
            actual_context,
            mmu_phase_context(returned_phase),
        ];
        let mut previous = (0, 0, 0);
        for context in contexts.into_iter().flatten() {
            let counters = (
                context.get("retired_instructions").and_then(Value::as_u64),
                context.get("cr3_changes").and_then(Value::as_u64),
                context.get("page_faults").and_then(Value::as_u64),
            );
            let (Some(retired), Some(cr3_changes), Some(page_faults)) = counters else {
                return false;
            };
            if retired < previous.0
                || cr3_changes < previous.1
                || page_faults < previous.2
                || retired > stop_counters.0.unwrap()
                || cr3_changes > stop_counters.1.unwrap()
                || page_faults > stop_counters.2.unwrap()
                || cr3_changes > ledger_event_count
                || !within_runner_bounds(context)
            {
                return false;
            }
            previous = (retired, cr3_changes, page_faults);
        }
        let derived_needs_flush = mmu_phase_raw(actual_phase).map(|raw| {
            raw.get("flags")
                .and_then(Value::as_str)
                .and_then(parse_hex32)
                .is_some_and(|flags| flags & 0x08f4 != 0)
        });
        let Some(predicate) = invocation.get("needs_flush") else {
            return false;
        };
        if match derived_needs_flush {
            Some(expected) => predicate.as_bool() != Some(expected),
            None => !predicate.is_null(),
        } {
            return false;
        }
        if v20 {
            let Some(addresses) = setup_candidates else {
                return false;
            };
            if let Some(walks) = invocation.get("initial_page_walks")
                && !walks.is_null()
                && !valid_mmu_gather_page_walks(walks, addresses, mmu_phase_cr3(initial_phase))
            {
                return false;
            }
            let _ = initial_walks_complete;
            let _ = return_walks_complete;
        }
    }

    let unpaired_valid = unpaired.iter().all(|event| {
        if !event
            .as_object()
            .is_some_and(|object| object.len() == if v20 { 6 } else { 5 })
        {
            return false;
        }
        let phase = event.get("phase").and_then(Value::as_str);
        let expected_rip = match phase {
            Some("relocate_vma_down") => 0xffff_ffff_8182_bfdb,
            Some("actual_post") => 0xffff_ffff_8180_9fa1,
            Some("return") => 0xffff_ffff_8182_bfe0,
            _ => return false,
        };
        matches!(
            event.get("status").and_then(Value::as_str),
            Some("unpaired" | "ambiguous" | "mismatch" | "read_error")
        ) && valid_mmu_gather_context(event.get("context"), expected_rip)
            && event
                .get("context")
                .is_some_and(|context| within_runner_bounds(context))
            && event
                .get("raw")
                .is_some_and(|v| v.is_null() || valid_mmu_gather_raw(v))
            && (!v20 || event.get("page_walks") == Some(&Value::Null))
            && event
                .get("error")
                .is_some_and(|v| v.is_null() || v.is_string())
    });
    if !unpaired_valid {
        return false;
    }
    if v20 {
        let destination = capture
            .get("copy_destination")
            .and_then(Value::as_str)
            .and_then(parse_hex64);
        let expected = destination.map(|dest| {
            setups.iter().any(|invocation| {
                serialized_mmu_candidate_pages(invocation.get("setup")).is_some_and(|pages| {
                    pages
                        .into_iter()
                        .flatten()
                        .any(|page| page == (dest & !0xfff))
                })
            })
        });
        if capture.get("destination_in_candidate_pages")
            != Some(&expected.map_or(Value::Null, Value::Bool))
        {
            return false;
        }
    }
    let complete = unpaired.is_empty()
        && !setups.is_empty()
        && setups.iter().all(|invocation| {
            invocation.get("closure").and_then(Value::as_str) == Some("returned")
                && invocation
                    .get("association_mismatch")
                    .and_then(Value::as_bool)
                    == Some(false)
                && ["setup", "initial", "actual_post", "returned"]
                    .iter()
                    .all(|phase| {
                        invocation
                            .get(*phase)
                            .and_then(|value| value.get("status"))
                            .and_then(Value::as_str)
                            == Some("captured")
                    })
                && (!v20
                    || serialized_mmu_candidate_pages(invocation.get("setup")).is_some_and(
                        |addresses| {
                            invocation["setup"]["value"]["candidate_error"].is_null()
                                && mmu_walks_are_complete(
                                    &invocation["setup_page_walks"],
                                    addresses,
                                    mmu_phase_cr3(invocation.get("setup")),
                                )
                                && invocation.get("initial_page_walks").is_some_and(|walks| {
                                    mmu_walks_are_complete(
                                        walks,
                                        addresses,
                                        mmu_phase_cr3(invocation.get("initial")),
                                    )
                                })
                                && invocation.get("return_page_walks").is_some_and(|walks| {
                                    mmu_walks_are_complete(
                                        walks,
                                        addresses,
                                        mmu_phase_cr3(invocation.get("returned")),
                                    )
                                })
                        },
                    ))
        });
    let expected_status = if setups_omitted != 0 || unpaired_omitted != 0 {
        "truncated"
    } else if setups.is_empty() && unpaired.is_empty() {
        "not_seen"
    } else if complete {
        "captured"
    } else {
        "incomplete"
    };
    status == expected_status
}

fn valid_mmu_phase(
    value: Option<&Value>,
    expected_rip: u64,
    value_key: &str,
    partial_key: &str,
    valid_payload: impl Fn(&Value) -> bool,
) -> bool {
    let Some(phase) = value else {
        return false;
    };
    let Some(status) = phase.get("status").and_then(Value::as_str) else {
        return false;
    };
    match status {
        "not_observed" => phase.as_object().is_some_and(|o| o.len() == 1),
        "captured" => {
            phase.as_object().is_some_and(|o| o.len() == 3)
                && phase
                    .get("context")
                    .is_some_and(|c| valid_mmu_gather_context(Some(c), expected_rip))
                && phase.get(value_key).is_some_and(&valid_payload)
                && !phase
                    .as_object()
                    .is_some_and(|o| o.contains_key(partial_key) || o.contains_key("error"))
        }
        "read_error" => {
            phase.as_object().is_some_and(|o| o.len() == 4)
                && phase
                    .get("context")
                    .is_some_and(|c| valid_mmu_gather_context(Some(c), expected_rip))
                && phase
                    .get(partial_key)
                    .is_some_and(|v| v.is_null() || valid_payload(v))
                && phase
                    .get("error")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                && !phase.as_object().is_some_and(|o| o.contains_key(value_key))
        }
        _ => false,
    }
}

fn mmu_phase_context(phase: Option<&Value>) -> Option<&Value> {
    match phase?.get("status")?.as_str()? {
        "captured" | "read_error" => phase?.get("context"),
        _ => None,
    }
}

fn mmu_phase_raw(phase: Option<&Value>) -> Option<&Value> {
    match phase?.get("status")?.as_str()? {
        "captured" => phase?.get("value"),
        "read_error" => phase?.get("partial").filter(|v| !v.is_null()),
        _ => None,
    }
}

fn valid_mmu_gather_context(value: Option<&Value>, expected_rip: u64) -> bool {
    let Some(context) = value else {
        return false;
    };
    context.as_object().is_some_and(|o| o.len() == 10)
        && context
            .get("rip")
            .and_then(Value::as_str)
            .and_then(parse_hex64)
            == Some(expected_rip)
        && ["cr3", "rbp", "rsp"].iter().all(|key| {
            context
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64)
        })
        && context
            .get("tlb_pointer")
            .is_some_and(|v| v.is_null() || v.as_str().is_some_and(canonical_hex64))
        && ["retired_instructions", "cr3_changes", "page_faults"]
            .iter()
            .all(|key| context.get(*key).and_then(Value::as_u64).is_some())
        && ["saved_rbp", "saved_return_rip"]
            .iter()
            .all(|key| context.get(*key).is_some_and(valid_nullable_hex64))
        && (context.get("saved_rbp") == Some(&Value::Null))
            == (context.get("saved_return_rip") == Some(&Value::Null))
}

fn valid_mmu_setup_data(value: &Value) -> bool {
    value.as_object().is_some_and(|o| o.len() == 4)
        && value
            .get("stack_shift")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        && value
            .get("vma")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        && value.get("vm_start").is_some_and(valid_nullable_hex64)
        && value.get("vm_end").is_some_and(valid_nullable_hex64)
}

fn valid_mmu_setup_data_v20(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.len() != 8
        || !value
            .get("stack_shift")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        || !value
            .get("vma")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        || !value.get("vm_start").is_some_and(valid_nullable_hex64)
        || !value.get("vm_end").is_some_and(valid_nullable_hex64)
    {
        return false;
    }
    let stack_shift = value["stack_shift"].as_str().and_then(parse_hex64).unwrap();
    let vm_start = value["vm_start"].as_str().and_then(parse_hex64);
    let expected_relocated = vm_start.and_then(|start| start.checked_sub(stack_shift));
    let expected_aligned = expected_relocated.map(|start| start & !0xfff);
    let expected_previous = expected_aligned.and_then(|start| start.checked_sub(4096));
    let candidates_valid = expected_relocated.is_some_and(canonical_guest_address)
        && expected_aligned.is_some_and(canonical_guest_address)
        && expected_previous.is_some_and(canonical_guest_address);
    let expected_pages = if candidates_valid {
        [expected_previous, expected_aligned]
    } else {
        [None, None]
    };
    let Some(pages) = value.get("candidate_pages").and_then(Value::as_array) else {
        return false;
    };
    pages.len() == 2
        && value
            .get("relocated_start")
            .is_some_and(valid_nullable_hex64)
        && value["relocated_start"].as_str().and_then(parse_hex64) == expected_relocated
        && value.get("aligned_start").is_some_and(valid_nullable_hex64)
        && value["aligned_start"].as_str().and_then(parse_hex64) == expected_aligned
        && pages.iter().zip(expected_pages).all(|(actual, expected)| {
            valid_nullable_hex64(actual) && actual.as_str().and_then(parse_hex64) == expected
        })
        && if candidates_valid {
            value.get("candidate_error") == Some(&Value::Null)
        } else {
            value
                .get("candidate_error")
                .and_then(Value::as_str)
                .is_some_and(|error| !error.is_empty())
        }
}

fn serialized_mmu_candidate_pages(phase: Option<&Value>) -> Option<[Option<u64>; 2]> {
    let phase = phase?;
    let data = match phase.get("status")?.as_str()? {
        "captured" => phase.get("value")?,
        "read_error" => phase.get("partial").filter(|value| !value.is_null())?,
        _ => return None,
    };
    let pages = data.get("candidate_pages")?.as_array()?;
    Some([
        pages.first()?.as_str().and_then(parse_hex64),
        pages.get(1)?.as_str().and_then(parse_hex64),
    ])
}

fn mmu_phase_cr3(phase: Option<&Value>) -> Option<u64> {
    phase?
        .get("context")?
        .get("cr3")?
        .as_str()
        .and_then(parse_hex64)
}
fn valid_mmu_gather_page_walk(
    value: &Value,
    expected_address: Option<u64>,
    cr3: Option<u64>,
) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.len() != 13
        || !value.get("address").is_some_and(valid_nullable_hex64)
        || value["address"].as_str().and_then(parse_hex64) != expected_address
        || value.get("access").and_then(Value::as_str) != Some("Write")
        || value.get("cpl").and_then(Value::as_u64) != Some(0)
        || value.get("origin").and_then(Value::as_str) != Some("Explicit")
        || value.get("profile").and_then(Value::as_str) != Some("long64_4k_2m")
        || !value.get("cr0_wp").is_some_and(Value::is_boolean)
        || !value.get("efer_nxe").is_some_and(Value::is_boolean)
    {
        return false;
    }
    let Some(status) = value.get("status").and_then(Value::as_str) else {
        return false;
    };
    let Some(entries_json) = value.get("entries").and_then(Value::as_array) else {
        return false;
    };
    if entries_json.len() > 4 {
        return false;
    }
    let mut entries = Vec::with_capacity(entries_json.len());
    for entry in entries_json {
        if !entry.as_object().is_some_and(|fields| fields.len() == 4) {
            return false;
        }
        let level = match entry.get("level").and_then(Value::as_str) {
            Some("Pml4") => od_machine::PageTableLevel::Pml4,
            Some("Pdpt") => od_machine::PageTableLevel::Pdpt,
            Some("Pd") => od_machine::PageTableLevel::Pd,
            Some("Pt") => od_machine::PageTableLevel::Pt,
            _ => return false,
        };
        let (Some(index), Some(entry_address), Some(entry_value)) = (
            entry.get("index").and_then(Value::as_u64),
            entry
                .get("entry_address")
                .and_then(Value::as_str)
                .and_then(parse_hex64),
            entry
                .get("entry_value")
                .and_then(Value::as_str)
                .and_then(parse_hex64),
        ) else {
            return false;
        };
        if index >= 512
            || !entry
                .get("entry_address")
                .is_some_and(|v| v.as_str().is_some_and(canonical_hex64))
            || !entry
                .get("entry_value")
                .is_some_and(|v| v.as_str().is_some_and(canonical_hex64))
        {
            return false;
        }
        entries.push(od_machine::WalkEntry {
            level,
            index: index as u16,
            entry_address: od_machine::GuestPhysicalAddress::new(entry_address),
            entry_value,
        });
    }
    let terminal = match value.get("terminal_level") {
        Some(Value::Null) => None,
        Some(Value::String(level)) => match level.as_str() {
            "Pml4" => Some(od_machine::PageTableLevel::Pml4),
            "Pdpt" => Some(od_machine::PageTableLevel::Pdpt),
            "Pd" => Some(od_machine::PageTableLevel::Pd),
            "Pt" => Some(od_machine::PageTableLevel::Pt),
            _ => return false,
        },
        _ => return false,
    };
    let leaf_size = match value.get("leaf_size") {
        Some(Value::Null) => None,
        Some(v) => match v.as_u64() {
            Some(size) => Some(size),
            None => return false,
        },
        None => return false,
    };
    let physical = match value.get("translated_physical") {
        Some(Value::Null) => None,
        Some(Value::String(v)) => match parse_hex64(v) {
            Some(address) if canonical_hex64(v) => Some(address),
            _ => return false,
        },
        _ => return false,
    };
    let error = match value.get("error") {
        Some(Value::Null) => None,
        Some(Value::String(v)) if !v.is_empty() => Some(v.as_str()),
        _ => return false,
    };
    match status {
        "translated" | "non_present" | "protection" if error.is_none() => {}
        "read_error" | "invalid_address" if error.is_some() => {}
        _ => return false,
    }
    match expected_address {
        Some(address) => cr3.is_some_and(|cr3| {
            value["address"].as_str().is_some_and(canonical_hex64)
                && od_machine::validate_long64_read_only_walk_observation(
                    address,
                    cr3,
                    &entries,
                    status,
                    terminal,
                    leaf_size,
                    physical,
                    value["cr0_wp"].as_bool().unwrap_or(false),
                    value["efer_nxe"].as_bool().unwrap_or(false),
                )
        }),
        None => {
            status == "invalid_address"
                && value["address"].is_null()
                && entries.is_empty()
                && terminal.is_none()
                && leaf_size.is_none()
                && physical.is_none()
                && cr3.is_some()
        }
    }
}
fn valid_mmu_gather_page_walks(
    value: &Value,
    addresses: [Option<u64>; 2],
    cr3: Option<u64>,
) -> bool {
    value.as_array().is_some_and(|walks| {
        walks.len() == 2
            && walks[0].get("cr0_wp") == walks[1].get("cr0_wp")
            && walks[0].get("efer_nxe") == walks[1].get("efer_nxe")
            && walks
                .iter()
                .zip(addresses)
                .all(|(walk, address)| valid_mmu_gather_page_walk(walk, address, cr3))
    })
}
fn mmu_walks_are_complete(value: &Value, addresses: [Option<u64>; 2], cr3: Option<u64>) -> bool {
    value.as_array().is_some_and(|walks| {
        walks.len() == 2
            && walks.iter().zip(addresses).all(|(walk, address)| {
                valid_mmu_gather_page_walk(walk, address, cr3)
                    && matches!(
                        walk.get("status").and_then(Value::as_str),
                        Some("translated" | "non_present" | "protection")
                    )
            })
    })
}
fn valid_mmu_gather_raw(raw: &Value) -> bool {
    raw.as_object().is_some_and(|o| o.len() == 6)
        && ["tlb", "mm", "start", "end"].iter().all(|key| {
            raw.get(*key)
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64)
        })
        && raw
            .get("flags")
            .and_then(Value::as_str)
            .and_then(parse_hex32)
            .is_some()
        && raw
            .get("mm_tlb_flush_nested")
            .and_then(Value::as_i64)
            .is_some_and(|value| i32::try_from(value).is_ok())
}

fn valid_nullable_hex64(value: &Value) -> bool {
    value.is_null() || value.as_str().is_some_and(canonical_hex64)
}

fn parse_hex32(value: &str) -> Option<u32> {
    value
        .strip_prefix("0x")
        .and_then(|digits| u32::from_str_radix(digits, 16).ok())
}

fn valid_elf_stack_copy_target_cr3_event(ledger: &Value) -> bool {
    let Some(target) = ledger.get("target_event") else {
        return false;
    };
    let Some(status) = target.get("status").and_then(Value::as_str) else {
        return false;
    };
    let Some(frames) = target.get("frames").and_then(Value::as_array) else {
        return false;
    };
    if frames.len() > od_machine::LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY {
        return false;
    }
    if status == "not_seen" {
        let target_event_was_seen =
            ledger
                .get("events")
                .and_then(Value::as_array)
                .is_some_and(|events| {
                    events.iter().any(|event| {
                        event.get("sequence").and_then(Value::as_u64)
                            == Some(od_machine::LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_ORDINAL)
                            && event.get("rip").and_then(Value::as_str)
                                == Some("0xffffffff8141d753")
                    })
                });
        return !target_event_was_seen
            && target.get("sequence") == Some(&Value::Null)
            && target.get("rip") == Some(&Value::Null)
            && target.get("cr3") == Some(&Value::Null)
            && target.get("retired_instructions") == Some(&Value::Null)
            && target.get("rsp") == Some(&Value::Null)
            && target.get("rbp") == Some(&Value::Null)
            && target.get("chain_status") == Some(&Value::Null)
            && frames.is_empty()
            && target.get("error") == Some(&Value::Null);
    }
    if status != "captured"
        || target.get("sequence").and_then(Value::as_u64)
            != Some(od_machine::LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_ORDINAL)
        || target.get("rip").and_then(Value::as_str) != Some("0xffffffff8141d753")
        || !target
            .get("cr3")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        || !target
            .get("rsp")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        || !target
            .get("rbp")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        || target
            .get("retired_instructions")
            .and_then(Value::as_u64)
            .is_none()
    {
        return false;
    }
    let matching_event = ledger
        .get("events")
        .and_then(Value::as_array)
        .and_then(|events| {
            events.iter().find(|event| {
                event.get("sequence").and_then(Value::as_u64)
                    == Some(od_machine::LINUX_ELF_STACK_COPY_TARGET_CR3_EVENT_ORDINAL)
            })
        });
    let Some(matching_event) = matching_event else {
        return false;
    };
    if matching_event.get("rip") != target.get("rip")
        || matching_event.get("new_cr3") != target.get("cr3")
        || matching_event.get("retired_instructions_through_current")
            != target.get("retired_instructions")
    {
        return false;
    }
    let chain_status = target.get("chain_status").and_then(Value::as_str);
    if !matches!(
        chain_status,
        Some("complete" | "unreadable" | "malformed" | "frame_limit")
    ) {
        return false;
    }
    let error = target.get("error");
    if (chain_status == Some("complete") && error != Some(&Value::Null))
        || (chain_status != Some("complete")
            && !error
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty()))
        || (chain_status == Some("frame_limit")
            && frames.len() != od_machine::LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY)
        || (chain_status == Some("unreadable")
            && frames.len() == od_machine::LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY)
    {
        return false;
    }
    let initial_rbp = target
        .get("rbp")
        .and_then(Value::as_str)
        .and_then(parse_hex64);
    let Some(initial_rbp) = initial_rbp else {
        return false;
    };
    if (initial_rbp == 0 && chain_status != Some("complete"))
        || (matches!(chain_status, Some("unreadable" | "frame_limit"))
            && (initial_rbp == 0
                || initial_rbp & 7 != 0
                || !canonical_guest_address(initial_rbp)
                || initial_rbp.checked_add(8).is_none()))
        || (chain_status == Some("malformed")
            && frames.is_empty()
            && (initial_rbp == 0
                || (initial_rbp & 7 == 0
                    && canonical_guest_address(initial_rbp)
                    && initial_rbp.checked_add(8).is_some())))
    {
        return false;
    }
    let mut previous_rbp = target
        .get("rbp")
        .and_then(Value::as_str)
        .and_then(parse_hex64);
    for (index, frame) in frames.iter().enumerate() {
        let (Some(rbp), Some(saved_slot), Some(saved_rbp), Some(return_slot), Some(return_address)) = (
            frame
                .get("rbp")
                .and_then(Value::as_str)
                .and_then(parse_hex64),
            frame
                .get("saved_rbp_slot")
                .and_then(Value::as_str)
                .and_then(parse_hex64),
            frame
                .get("saved_rbp")
                .and_then(Value::as_str)
                .and_then(parse_hex64),
            frame
                .get("return_address_slot")
                .and_then(Value::as_str)
                .and_then(parse_hex64),
            frame
                .get("return_address")
                .and_then(Value::as_str)
                .and_then(parse_hex64),
        ) else {
            return false;
        };
        let terminal_malformed_frame =
            chain_status == Some("malformed") && index + 1 == frames.len();
        let bad_saved_rbp = saved_rbp != 0
            && (saved_rbp & 7 != 0 || !canonical_guest_address(saved_rbp) || saved_rbp <= rbp);
        let bad_return_address = !canonical_guest_address(return_address);
        let next_frame_slot_overflow = saved_rbp != 0 && saved_rbp.checked_add(8).is_none();
        if Some(rbp) != previous_rbp
            || rbp & 7 != 0
            || !canonical_guest_address(rbp)
            || saved_slot != rbp
            || rbp.checked_add(8) != Some(return_slot)
            || (!terminal_malformed_frame && (bad_saved_rbp || bad_return_address))
            || (!terminal_malformed_frame && saved_rbp == 0 && index + 1 != frames.len())
            || (terminal_malformed_frame
                && !bad_saved_rbp
                && !bad_return_address
                && (!next_frame_slot_overflow
                    || frames.len() == od_machine::LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY))
        {
            return false;
        }
        previous_rbp = (saved_rbp != 0).then_some(saved_rbp);
    }
    if chain_status == Some("complete") {
        if frames.is_empty() {
            if target
                .get("rbp")
                .and_then(Value::as_str)
                .and_then(parse_hex64)
                != Some(0)
            {
                return false;
            }
        } else if frames
            .last()
            .and_then(|frame| frame.get("saved_rbp"))
            .and_then(Value::as_str)
            .and_then(parse_hex64)
            != Some(0)
        {
            return false;
        }
    } else if chain_status == Some("unreadable")
        && frames
            .last()
            .and_then(|frame| frame.get("saved_rbp"))
            .and_then(Value::as_str)
            .and_then(parse_hex64)
            .is_some_and(|saved_rbp| saved_rbp == 0 || saved_rbp.checked_add(8).is_none())
    {
        return false;
    } else if chain_status == Some("frame_limit")
        && frames
            .last()
            .and_then(|frame| frame.get("saved_rbp"))
            .and_then(Value::as_str)
            .and_then(parse_hex64)
            == Some(0)
    {
        return false;
    }
    true
}

fn parse_hex64(value: &str) -> Option<u64> {
    value
        .strip_prefix("0x")
        .and_then(|digits| u64::from_str_radix(digits, 16).ok())
}

fn canonical_guest_address(address: u64) -> bool {
    let sign = (address >> 47) & 1;
    (sign == 0 && address >> 48 == 0) || (sign == 1 && address >> 48 == 0xffff)
}

#[cfg(test)]
fn valid_elf_stack_copy_pre_copy_cr3_ledger(
    value: &Value,
    snapshot: &serde_json::Map<String, Value>,
) -> bool {
    valid_elf_stack_copy_pre_copy_cr3_ledger_with_runner(value, snapshot, None)
}

fn valid_elf_stack_copy_pre_copy_cr3_ledger_with_runner(
    value: &Value,
    snapshot: &serde_json::Map<String, Value>,
    runner: Option<&Value>,
) -> bool {
    let Some(ledger) = value.as_object() else {
        return false;
    };
    let status = ledger.get("status").and_then(Value::as_str);
    let Some(event_count) = ledger.get("event_count").and_then(Value::as_u64) else {
        return false;
    };
    let Some(events_omitted) = ledger.get("events_omitted").and_then(Value::as_u64) else {
        return false;
    };
    let Some(events) = ledger.get("events").and_then(Value::as_array) else {
        return false;
    };
    let Some(markers) = ledger.get("boundary_markers").and_then(Value::as_array) else {
        return false;
    };
    let Some(retained_and_omitted) = (events.len() as u64).checked_add(events_omitted) else {
        return false;
    };
    if events.len() > od_machine::LINUX_ELF_STACK_COPY_PRE_COPY_CR3_EVENT_CAPACITY
        || markers.len() > od_machine::LINUX_ELF_STACK_COPY_BOUNDARIES.len()
        || retained_and_omitted != event_count
        || !matches!(
            status,
            Some("complete" | "incomplete_overflow" | "run_ended_before_target")
        )
        || (status == Some("complete") && events_omitted != 0)
        || (status == Some("run_ended_before_target") && events_omitted != 0)
        || (status == Some("incomplete_overflow")
            && (events_omitted == 0
                || events.len() != od_machine::LINUX_ELF_STACK_COPY_PRE_COPY_CR3_EVENT_CAPACITY))
    {
        return false;
    }
    let captured_entry_retired =
        if snapshot.get("status").and_then(Value::as_str) == Some("captured") {
            let Some(retired) = snapshot
                .get("invocation")
                .and_then(|invocation| invocation.get("entry"))
                .and_then(|entry| entry.get("retired_instructions_through_current"))
                .and_then(Value::as_u64)
            else {
                return false;
            };
            if status == Some("run_ended_before_target") {
                return false;
            }
            Some(retired)
        } else {
            None
        };
    let mut previous_marker_index = None;
    let mut previous_marker_retired = None;
    for marker in markers {
        let (Some(name), Some(rip), Some(retired)) = (
            marker.get("name").and_then(Value::as_str),
            marker.get("rip").and_then(Value::as_str),
            marker
                .get("retired_instructions_before_current")
                .and_then(Value::as_u64),
        ) else {
            return false;
        };
        let Some(index) = od_machine::LINUX_ELF_STACK_COPY_BOUNDARIES.iter().position(
            |(expected_rip, expected_name)| {
                name == *expected_name && rip == format!("0x{expected_rip:016x}")
            },
        ) else {
            return false;
        };
        if previous_marker_index.is_some_and(|previous| index <= previous)
            || previous_marker_retired.is_some_and(|previous| retired < previous)
            || captured_entry_retired.is_some_and(|entry_retired| retired > entry_retired)
        {
            return false;
        }
        previous_marker_index = Some(index);
        previous_marker_retired = Some(retired);
    }
    let mut previous_retired = 0;
    let mut previous_new_cr3 = None;
    for (index, event) in events.iter().enumerate() {
        let Some(sequence) = event.get("sequence").and_then(Value::as_u64) else {
            return false;
        };
        let (Some(rip), Some(old_cr3), Some(new_cr3)) = (
            event.get("rip").and_then(Value::as_str),
            event.get("old_cr3").and_then(Value::as_str),
            event.get("new_cr3").and_then(Value::as_str),
        ) else {
            return false;
        };
        let Some(retired) = event
            .get("retired_instructions_through_current")
            .and_then(Value::as_u64)
        else {
            return false;
        };
        let cpl_valid = event.get("cpl").and_then(Value::as_u64) == Some(0);
        if sequence != index as u64 + 1
            || !canonical_hex64(rip)
            || !canonical_hex64(old_cr3)
            || !canonical_hex64(new_cr3)
            || !cpl_valid
            || retired < previous_retired
            || captured_entry_retired.is_some_and(|entry_retired| retired > entry_retired)
            || previous_new_cr3.is_some_and(|expected| old_cr3 != expected)
        {
            return false;
        }
        previous_retired = retired;
        previous_new_cr3 = Some(new_cr3);
    }
    if captured_entry_retired.is_some() {
        let Some(invocation) = snapshot.get("invocation") else {
            return false;
        };
        let Some(entry_changes) = invocation
            .get("entry")
            .and_then(|entry| entry.get("cr3_changes"))
            .and_then(Value::as_u64)
        else {
            return false;
        };
        if event_count != entry_changes {
            return false;
        }
        let entry_cr3 = invocation
            .get("entry")
            .and_then(|entry| entry.get("cr3"))
            .and_then(Value::as_str);
        if status == Some("complete")
            && previous_new_cr3.is_some_and(|new_cr3| Some(new_cr3) != entry_cr3)
        {
            return false;
        }
    }
    if snapshot.get("status").and_then(Value::as_str) == Some("run_ended_before_target") {
        let Some(run_stop_cr3_changes) = runner
            .and_then(|runner| runner.get("stop"))
            .and_then(|stop| stop.get("cr3_changes"))
            .and_then(Value::as_u64)
        else {
            return false;
        };
        if event_count != run_stop_cr3_changes {
            return false;
        }
    }
    true
}

fn valid_elf_stack_copy_invocation(
    value: &Value,
    snapshot: &serde_json::Map<String, Value>,
    runner: &Value,
) -> bool {
    let Some(invocation) = value.as_object() else {
        return false;
    };
    let status = invocation.get("status").and_then(Value::as_str);
    if !matches!(
        status,
        Some("complete" | "incomplete_gate" | "incomplete_overflow" | "incomplete_run_ended")
    ) {
        return false;
    }
    let valid_snapshot = |snapshot: Option<&Value>| {
        snapshot.is_some_and(|snapshot| {
            snapshot
                .get("cr3")
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64)
                && [
                    "delivered_page_faults",
                    "delivered_exceptions",
                    "cr3_changes",
                    "rdtsc_reads",
                    "retired_instructions_through_current",
                ]
                .iter()
                .all(|key| snapshot.get(*key).and_then(Value::as_u64).is_some())
        })
    };
    let entry = invocation.get("entry");
    let exit = match invocation.get("return_boundary") {
        None | Some(Value::Null) => None,
        Some(boundary) if valid_snapshot(Some(boundary)) => Some(boundary),
        Some(_) => return false,
    };
    let stop = invocation.get("stop");
    if !valid_snapshot(entry) || !valid_snapshot(stop) {
        return false;
    }
    let Some(entry) = entry else { return false };
    let (Some(cr3_events), Some(pf_events), Some(rdtsc_sites)) = (
        invocation.get("cr3_events").and_then(Value::as_array),
        invocation
            .get("page_fault_events")
            .and_then(Value::as_array),
        invocation.get("rdtsc_sites").and_then(Value::as_array),
    ) else {
        return false;
    };
    let cr3_omitted = invocation.get("cr3_events_omitted").and_then(Value::as_u64);
    let pf_omitted = invocation
        .get("page_fault_events_omitted")
        .and_then(Value::as_u64);
    let rdtsc_overflow = invocation
        .get("rdtsc_site_overflow_reads")
        .and_then(Value::as_u64);
    let (Some(duration), Some(cap)) = (
        invocation.get("retired_duration").and_then(Value::as_u64),
        invocation
            .get("retired_duration_cap")
            .and_then(Value::as_u64),
    ) else {
        return false;
    };
    if cap != od_machine::LINUX_ELF_STACK_COPY_RETIRED_CAPACITY {
        return false;
    }
    if cr3_events.len() > od_machine::LINUX_ELF_STACK_COPY_CR3_EVENT_CAPACITY
        || pf_events.len() > od_machine::LINUX_ELF_STACK_COPY_PF_EVENT_CAPACITY
        || rdtsc_sites.len() > od_machine::LINUX_ELF_STACK_COPY_RDTSC_SITE_CAPACITY
        || cr3_omitted.is_none()
        || pf_omitted.is_none()
        || rdtsc_overflow.is_none()
        || (duration > cap && status != Some("incomplete_overflow"))
    {
        return false;
    }
    let valid_retired = |event: &Value| {
        event
            .get("retired_instructions_through_current")
            .and_then(Value::as_u64)
            .is_some()
    };
    if !cr3_events.iter().all(|event| {
        ["rip", "old_cr3", "new_cr3"].iter().all(|key| {
            event
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64)
        }) && valid_retired(event)
    }) || !pf_events.iter().all(|event| {
        ["rip", "cr2"].iter().all(|key| {
            event
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64)
        }) && event.get("error_code").and_then(Value::as_u64).is_some()
            && matches!(
                event.get("access").and_then(Value::as_str),
                Some("Read" | "Write" | "Fetch" | "unknown")
            )
            && event
                .get("cpl")
                .and_then(Value::as_u64)
                .is_some_and(|cpl| cpl <= 3)
            && valid_retired(event)
    }) || !rdtsc_sites.iter().all(|site| {
        site.get("rip")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
            && site
                .get("reads")
                .and_then(Value::as_u64)
                .is_some_and(|reads| reads > 0)
    }) {
        return false;
    }
    let count =
        |snapshot: &Value, key: &str| snapshot.get(key).and_then(Value::as_u64).unwrap_or(0);
    let stop = stop.unwrap();
    let nested_return_rip = invocation.get("return_rip");
    if nested_return_rip != snapshot.get("return_rip") {
        return false;
    }
    let monotonic = |first: &Value, second: &Value| {
        [
            "delivered_page_faults",
            "delivered_exceptions",
            "cr3_changes",
            "rdtsc_reads",
            "retired_instructions_through_current",
        ]
        .iter()
        .all(|key| count(second, key) >= count(first, key))
    };
    let interval_end = exit.unwrap_or(stop);
    let entry_retired = count(entry, "retired_instructions_through_current");
    let interval_end_retired = count(interval_end, "retired_instructions_through_current");
    let ordered_in_interval = |events: &[Value]| {
        let mut previous = entry_retired;
        events.iter().all(|event| {
            let Some(retired) = event
                .get("retired_instructions_through_current")
                .and_then(Value::as_u64)
            else {
                return false;
            };
            let ordered = retired >= previous && retired <= interval_end_retired;
            previous = retired;
            ordered
        })
    };
    if !ordered_in_interval(cr3_events) || !ordered_in_interval(pf_events) {
        return false;
    }
    let run_stop = runner.get("stop");
    let stop_matches_run = run_stop.is_some_and(|run_stop| {
        let stop_rdtsc = run_stop
            .get("rdtsc_reads_by_cpl")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_u64);
        count(stop, "delivered_page_faults")
            == run_stop
                .get("page_faults_delivered")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
            && count(stop, "delivered_exceptions")
                == run_stop
                    .get("exceptions_delivered")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX)
            && count(stop, "cr3_changes")
                == run_stop
                    .get("cr3_changes")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX)
            && Some(count(stop, "rdtsc_reads")) == stop_rdtsc
            && count(stop, "retired_instructions_through_current")
                == run_stop
                    .get("retired")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX)
            && run_stop
                .get("cr3")
                .and_then(Value::as_str)
                .is_some_and(|cr3| stop.get("cr3").and_then(Value::as_str) == Some(cr3))
    });
    let parse_cr3 = |snapshot: &Value| {
        snapshot
            .get("cr3")
            .and_then(Value::as_str)
            .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok())
    };
    let mut expected_cr3 = parse_cr3(entry);
    for event in cr3_events {
        let old_cr3 = event
            .get("old_cr3")
            .and_then(Value::as_str)
            .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());
        let new_cr3 = event
            .get("new_cr3")
            .and_then(Value::as_str)
            .and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok());
        if old_cr3 != expected_cr3 {
            return false;
        }
        expected_cr3 = new_cr3;
    }
    if status == Some("complete") {
        let Some(exit) = exit else {
            return false;
        };
        let Some(site_reads) = rdtsc_sites.iter().try_fold(0u64, |sum, site| {
            sum.checked_add(site.get("reads")?.as_u64()?)
        }) else {
            return false;
        };
        let Some(site_reads_with_overflow) =
            site_reads.checked_add(rdtsc_overflow.unwrap_or(u64::MAX))
        else {
            return false;
        };
        let Some(cr3_events_total) =
            (cr3_events.len() as u64).checked_add(cr3_omitted.unwrap_or(u64::MAX))
        else {
            return false;
        };
        let Some(pf_events_total) =
            (pf_events.len() as u64).checked_add(pf_omitted.unwrap_or(u64::MAX))
        else {
            return false;
        };
        if expected_cr3 != parse_cr3(exit) {
            return false;
        }
        stop_matches_run
            && monotonic(entry, exit)
            && monotonic(exit, stop)
            && count(exit, "cr3_changes").checked_sub(count(entry, "cr3_changes"))
                == Some(cr3_events_total)
            && count(exit, "delivered_page_faults")
                .checked_sub(count(entry, "delivered_page_faults"))
                == Some(pf_events_total)
            && count(exit, "delivered_exceptions")
                .checked_sub(count(entry, "delivered_exceptions"))
                .is_some_and(|delta| delta >= pf_events_total)
            && count(exit, "retired_instructions_through_current")
                .checked_sub(count(entry, "retired_instructions_through_current"))
                == Some(duration)
            && count(exit, "rdtsc_reads").checked_sub(count(entry, "rdtsc_reads"))
                == Some(site_reads_with_overflow)
            && cr3_omitted == Some(0)
            && pf_omitted == Some(0)
            && rdtsc_overflow == Some(0)
    } else {
        let has_return_boundary = exit.is_some();
        let boundary_status_valid = match status {
            Some("incomplete_gate" | "incomplete_run_ended") => !has_return_boundary,
            Some("incomplete_overflow") => true,
            _ => false,
        };
        let latest = exit.unwrap_or(entry);
        let monotonic_to_stop = [
            "delivered_page_faults",
            "delivered_exceptions",
            "cr3_changes",
            "rdtsc_reads",
            "retired_instructions_through_current",
        ]
        .iter()
        .all(|key| count(stop, key) >= count(latest, key));
        let overflow_evidence = duration > cap
            || cr3_omitted.unwrap_or(0) > 0
            || pf_omitted.unwrap_or(0) > 0
            || rdtsc_overflow.unwrap_or(0) > 0;
        boundary_status_valid
            && (!has_return_boundary || status == Some("incomplete_overflow"))
            && monotonic(entry, latest)
            && monotonic_to_stop
            && stop_matches_run
            && ((status == Some("incomplete_overflow")) == overflow_evidence)
    }
}

fn valid_delay_tsc_call_summary(raw: &Value, metadata: &Value) -> bool {
    let Some(summary) = raw.get("delay_tsc_call_summary") else {
        return false;
    };
    let metadata_keys = [
        "delay_tsc_call_summary_entry_count",
        "delay_tsc_call_summary_retained_count",
        "delay_tsc_call_summary_entries_omitted",
        "delay_tsc_call_summary_poll_count",
        "delay_tsc_call_summary_unmatched_polls",
        "delay_tsc_call_summary_ambiguous_polls",
    ];
    if summary.is_null() {
        return metadata_keys
            .iter()
            .all(|key| metadata.get(*key).is_some_and(Value::is_null));
    }
    let Some(summary) = summary.as_object() else {
        return false;
    };
    let Some(entries) = summary.get("entries").and_then(Value::as_array) else {
        return false;
    };
    let (Some(entry_count), Some(omitted), Some(poll_count), Some(unmatched), Some(ambiguous)) = (
        summary.get("entry_count").and_then(Value::as_u64),
        summary.get("entries_omitted").and_then(Value::as_u64),
        summary.get("poll_count").and_then(Value::as_u64),
        summary.get("unmatched_polls").and_then(Value::as_u64),
        summary.get("ambiguous_polls").and_then(Value::as_u64),
    ) else {
        return false;
    };
    let expected_entry_rip = format!("0x{:x}", od_machine::LINUX_DELAY_TSC_ENTRY_RIP);
    let expected_poll_rip = format!("0x{:x}", od_machine::LINUX_DELAY_TSC_POLL_RIP);
    let Some(sites) = raw.get("rdtsc_cpl0_sites").and_then(Value::as_array) else {
        return false;
    };
    let mut histogram = BTreeMap::new();
    for site in sites {
        let (Some(rip), Some(reads)) = (
            site.get("rip").and_then(Value::as_str),
            site.get("reads").and_then(Value::as_u64),
        ) else {
            return false;
        };
        if !canonical_hex64(rip) || reads == 0 || histogram.insert(rip, reads).is_some() {
            return false;
        }
    }
    let histogram_count = |rip: &str| histogram.get(rip).copied().unwrap_or(0);
    let metadata_match = [
        entry_count,
        entries.len() as u64,
        omitted,
        poll_count,
        unmatched,
        ambiguous,
    ]
    .into_iter()
    .zip(metadata_keys)
    .all(|(value, key)| metadata.get(key).and_then(Value::as_u64) == Some(value));
    let Some(entry_sum) = (entries.len() as u64).checked_add(omitted) else {
        return false;
    };
    let Some(poll_sum) = entries.iter().try_fold(0u64, |sum, entry| {
        sum.checked_add(entry.get("poll_count")?.as_u64()?)
    }) else {
        return false;
    };
    let recognized_chain_end = |value: &str| {
        matches!(
            value,
            "null_frame_pointer"
                | "non_canonical_frame_pointer"
                | "unaligned_frame_pointer"
                | "outside_stack_window"
                | "repeated_frame_pointer"
                | "non_monotonic_frame_pointer"
                | "read_failed"
                | "non_canonical_return_address"
                | "frame_limit"
        )
    };
    let valid_entry = |entry: &Value, index: usize| {
        let Some(entry) = entry.as_object() else {
            return false;
        };
        let Some(request) = entry
            .get("requested_cycles")
            .and_then(Value::as_str)
            .filter(|value| canonical_hex64(value))
            .and_then(|value| u64::from_str_radix(value.trim_start_matches("0x"), 16).ok())
        else {
            return false;
        };
        let elapsed = entry.get("last_elapsed_cycles");
        let elapsed_value = match elapsed {
            Some(Value::Null) => None,
            Some(Value::String(value)) if canonical_hex64(value) => {
                u64::from_str_radix(value.trim_start_matches("0x"), 16).ok()
            }
            _ => return false,
        };
        let count = entry.get("poll_count").and_then(Value::as_u64);
        let first = entry.get("first_poll_ordinal");
        let last = entry.get("last_poll_ordinal");
        let ordinals_valid = match (count, first, last, elapsed_value) {
            (Some(0), Some(Value::Null), Some(Value::Null), None) => true,
            (Some(count @ 1..), Some(first), Some(last), Some(_)) => first
                .as_u64()
                .zip(last.as_u64())
                .is_some_and(|(first, last)| {
                    first > 0
                        && first <= last
                        && last <= poll_count
                        && count <= poll_count
                        && last
                            .checked_sub(first)
                            .and_then(|span| span.checked_add(1))
                            .is_some_and(|span| count <= span)
                        && (count != 1 || first == last)
                }),
            _ => false,
        };
        let links_valid = entry
            .get("frame_links")
            .and_then(Value::as_array)
            .is_some_and(|links| {
                links.len() <= 4
                    && links.iter().all(|link| {
                        ["frame_pointer", "return_address"].iter().all(|key| {
                            link.get(*key)
                                .and_then(Value::as_str)
                                .is_some_and(canonical_hex64)
                        })
                    })
            });
        let cr3_valid = matches!(entry.get("cr3"), Some(Value::Null))
            || entry
                .get("cr3")
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64);
        let registers_valid = ["entry_tsc", "rbp", "rsp"].iter().all(|key| {
            entry
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(canonical_hex64)
        });
        let terminal = entry
            .get("terminal_threshold_seen")
            .and_then(Value::as_bool);
        let status = entry.get("poll_sequence_status").and_then(Value::as_str);
        let chain_end = entry.get("frame_chain_end").and_then(Value::as_str);
        let chain_incomplete = entry.get("frame_chain_incomplete").and_then(Value::as_bool);
        let terminal_valid = match (terminal, status, elapsed_value) {
            (Some(true), Some("threshold_seen"), Some(elapsed)) => elapsed >= request,
            (Some(false), Some("partial"), Some(elapsed)) => elapsed < request,
            (Some(false), Some("partial"), None) => true,
            _ => false,
        };
        entry.get("ordinal").and_then(Value::as_u64) == Some(index as u64 + 1)
            && entry
                .get("frame_chain_end")
                .and_then(Value::as_str)
                .is_some_and(recognized_chain_end)
            && chain_incomplete == chain_end.map(|end| end != "null_frame_pointer")
            && ordinals_valid
            && links_valid
            && cr3_valid
            && registers_valid
            && terminal_valid
    };
    summary.get("entry_rip").and_then(Value::as_str) == Some(expected_entry_rip.as_str())
        && summary.get("poll_rip").and_then(Value::as_str) == Some(expected_poll_rip.as_str())
        && entries.len() <= od_machine::LINUX_DELAY_TSC_CALL_CAPACITY
        && entry_count == entry_sum
        && histogram_count(&expected_entry_rip) == entry_count
        && histogram_count(&expected_poll_rip) == poll_count
        && poll_sum.checked_add(unmatched) == Some(poll_count)
        && ambiguous <= unmatched
        && entries
            .iter()
            .enumerate()
            .all(|(index, entry)| valid_entry(entry, index))
        && raw
            .get("rdtsc_site_samples")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        && metadata_match
}

fn valid_elf_stack_copy_walk_entry(entry: &Value) -> bool {
    matches!(
        entry.get("level").and_then(Value::as_str),
        Some("Pml4" | "Pdpt" | "Pd" | "Pt")
    ) && entry
        .get("index")
        .and_then(Value::as_u64)
        .is_some_and(|index| index <= 0x1ff)
        && entry
            .get("entry_address")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        && entry
            .get("entry_value")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
}

fn elf_stack_copy_gate_matches(snapshot: &serde_json::Map<String, Value>) -> bool {
    let read_hex = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .and_then(|hex| hex.strip_prefix("0x"))
            .and_then(|digits| u64::from_str_radix(digits, 16).ok())
    };
    let registers = snapshot.get("registers").and_then(Value::as_object);
    let (Some(r15), Some(r13), Some(rdi), Some(rsi), Some(rdx), Some(bprm_address), Some(bprm_p)) = (
        registers.and_then(|registers| read_hex(registers.get("r15"))),
        registers.and_then(|registers| read_hex(registers.get("r13"))),
        registers.and_then(|registers| read_hex(registers.get("rdi"))),
        registers.and_then(|registers| read_hex(registers.get("rsi"))),
        registers.and_then(|registers| read_hex(registers.get("rdx"))),
        read_hex(snapshot.get("bprm_p_address")),
        read_hex(snapshot.get("bprm_p")),
    ) else {
        return false;
    };
    read_hex(snapshot.get("rip")) == Some(od_machine::LINUX_ELF_STACK_COPY_RIP)
        && snapshot.get("cpl").and_then(Value::as_u64) == Some(0)
        && snapshot
            .get("platform_string_matches")
            .is_none_or(|value| value == &json!(true))
        && snapshot.get("instruction_bytes").and_then(Value::as_str) == Some("e847d93400")
        && snapshot
            .get("cs_selector")
            .and_then(Value::as_str)
            .and_then(|selector| selector.strip_prefix("0x"))
            .and_then(|digits| u16::from_str_radix(digits, 16).ok())
            .is_some_and(|selector| selector & 3 == 0)
        && r15.checked_add(0x20) == Some(bprm_address)
        && r13 & 0xf == 0
        && r13.checked_sub(7) == Some(rdi)
        && rdx == 7
        && rsi == 0xffff_ffff_82fd_eb8f
        && bprm_p >= r13
        && bprm_p - r13 <= 8206
        && read_hex(snapshot.get("aligned_stack_pointer")) == Some(r13)
        && read_hex(snapshot.get("copy_destination")) == Some(rdi)
        && read_hex(snapshot.get("copy_length")) == Some(rdx)
}

fn elf_stack_copy_probe_count_valid(probes: &[Value]) -> bool {
    probes.len() <= od_machine::LINUX_ELF_STACK_COPY_PROBE_CAPACITY
}

fn valid_instruction_path_trace_header(value: &Value) -> bool {
    let Some(header) = value.as_object() else {
        return false;
    };
    let valid_cr3 = header
        .get("cr3")
        .is_some_and(|value| value.is_null() || value.as_str().is_some_and(canonical_hex64));
    let valid_cs = header.get("cs_selector").is_some_and(|value| {
        if value.is_null() {
            return true;
        }
        value.as_str().is_some_and(|selector| {
            selector.len() == 6
                && selector.starts_with("0x")
                && u16::from_str_radix(&selector[2..], 16)
                    .is_ok_and(|parsed| format!("0x{parsed:04x}") == selector)
        })
    });
    let valid_gpr = header
        .get("gpr")
        .and_then(Value::as_array)
        .is_some_and(|registers| {
            registers.len() == 16
                && registers
                    .iter()
                    .all(|value| value.as_str().is_some_and(canonical_hex64))
        });
    let valid_frames = header
        .get("frame_chain")
        .and_then(Value::as_array)
        .is_some_and(|frames| {
            frames.len() <= od_machine::LINUX_PAGE_FAULT_FRAME_LINK_CAPACITY
                && frames.iter().all(|frame| {
                    frame
                        .get("frame_pointer")
                        .and_then(Value::as_str)
                        .is_some_and(canonical_hex64)
                        && frame
                            .get("return_address")
                            .and_then(Value::as_str)
                            .is_some_and(canonical_hex64)
                })
        });
    let valid_frame_end = matches!(
        header.get("frame_chain_end").and_then(Value::as_str),
        Some(
            "NullFramePointer"
                | "NonCanonicalFramePointer"
                | "UnalignedFramePointer"
                | "OutsideStackWindow"
                | "RepeatedFramePointer"
                | "NonMonotonicFramePointer"
                | "ReadFailed"
                | "NonCanonicalReturnAddress"
                | "FrameLimit"
        )
    );
    header
        .get("rip")
        .and_then(Value::as_str)
        .is_some_and(canonical_hex64)
        && header
            .get("rflags")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        && valid_gpr
        && valid_cr3
        && valid_cs
        && valid_frames
        && valid_frame_end
}

fn valid_instruction_path_trace_record(record: &Value) -> bool {
    let Some(record_object) = record.as_object() else {
        return false;
    };
    let rip = record_object
        .get("rip")
        .and_then(Value::as_str)
        .filter(|rip| canonical_hex64(rip));
    let next_rip = record_object
        .get("next_rip")
        .and_then(Value::as_str)
        .filter(|rip| canonical_hex64(rip));
    let Some(flow) = record_object.get("control_flow") else {
        return false;
    };
    if flow.is_null() {
        return rip.is_some() && next_rip.is_some();
    }
    let Some(flow) = flow.as_object() else {
        return false;
    };
    let kind = flow.get("kind").and_then(Value::as_str);
    let condition = flow.get("condition");
    let destination = flow
        .get("destination")
        .and_then(Value::as_str)
        .filter(|address| canonical_hex64(address));
    let registers = flow.get("destination_registers").and_then(Value::as_array);
    let registers_valid = registers.is_some_and(|registers| {
        registers.len() <= 2
            && registers.iter().all(|register| {
                register
                    .get("register")
                    .and_then(Value::as_u64)
                    .is_some_and(|index| index < 16)
                    && register
                        .get("value")
                        .and_then(Value::as_str)
                        .is_some_and(canonical_hex64)
            })
            && registers.iter().enumerate().all(|(index, register)| {
                registers[index + 1..]
                    .iter()
                    .all(|other| register.get("register") != other.get("register"))
            })
    });
    let condition_valid = match kind {
        Some("branch") => condition
            .and_then(Value::as_u64)
            .is_some_and(|value| value <= 15),
        Some(
            "jump" | "call" | "return" | "syscall" | "sysret" | "indirect_jump" | "indirect_call",
        ) => condition.is_some_and(Value::is_null),
        _ => false,
    };
    rip.is_some()
        && next_rip.is_some()
        && condition_valid
        && flow.get("taken").and_then(Value::as_bool).is_some()
        && flow
            .get("rflags_before")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        && flow
            .get("rflags_after")
            .and_then(Value::as_str)
            .is_some_and(canonical_hex64)
        && registers_valid
        && destination.is_some()
        && destination == next_rip
}

fn selected_rip_is_canonical_not_null(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(rip)) if canonical_hex64(rip))
}

fn parse_sidecar_sample_window(
    value: Option<&Value>,
) -> Result<Option<od_machine::RdtscSampleWindow>, String> {
    match value {
        Some(Value::Null) => Ok(None),
        Some(Value::Object(fields)) if fields.len() == 2 => {
            let first_hit = fields
                .get("first_hit")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    "execution-observations sample window first_hit is invalid".to_string()
                })?;
            let last_hit = fields
                .get("last_hit")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    "execution-observations sample window last_hit is invalid".to_string()
                })?;
            if first_hit == 0 || last_hit < first_hit {
                return Err("execution-observations sample window bounds are invalid".into());
            }
            Ok(Some(od_machine::RdtscSampleWindow {
                first_hit,
                last_hit,
            }))
        }
        _ => Err("execution-observations sample window is missing or invalid".into()),
    }
}

fn canonical_hex64(value: &str) -> bool {
    let Some(digits) = value.strip_prefix("0x") else {
        return false;
    };
    let Ok(parsed) = u64::from_str_radix(digits, 16) else {
        return false;
    };
    format!("0x{parsed:x}") == value
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("expected an even number of hexadecimal digits".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16).map_err(|error| error.to_string())
        })
        .collect()
}

fn fresh_manifest(path: &Path) -> Result<SeriesManifest, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let manifest: SeriesManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Stage 5 series JSON: {error}"))?;
    if !supported_schema_version(manifest.schema_version)
        || manifest.requested_runs == 0
        || manifest.runs.len() != manifest.requested_runs
        || manifest.series_id.is_empty()
        || manifest.scenario_id.is_empty()
    {
        return Err("invalid Stage 5 series schema version or run count".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut references = std::collections::HashSet::new();
    if manifest.runs.iter().any(|run| {
        run.run_id.is_empty()
            || !ids.insert(run.run_id.as_str())
            || !references.insert(run.manifest.as_str())
    }) {
        return Err("series contains an empty or duplicate run reference".into());
    }
    Ok(manifest)
}

pub(crate) fn compare_series_paths(
    left_path: &Path,
    right_path: &Path,
) -> Result<Stage5Comparison, String> {
    let left = fresh_manifest(left_path)?;
    let right = fresh_manifest(right_path)?;
    let mut unexplained = Vec::new();
    let mut explained = Vec::new();
    let left_runs = load_series_runs(left_path, &left)?;
    let right_runs = load_series_runs(right_path, &right)?;
    explained.push("series_id, run_id, nonce, raw runner JSON hash, UART hash/path, and wall time are run-specific metadata".into());
    if left.run_profile != right.run_profile {
        unexplained.push("run_profile differs".into());
    }
    if left.scenario_id != right.scenario_id {
        unexplained.push("scenario_id differs".into());
    }
    if left.requested_runs != right.requested_runs {
        unexplained.push("requested run count differs".into());
    }
    if left.outcome != right.outcome {
        unexplained.push("series outcome differs".into());
    }
    if left.runs.len() != right.runs.len() {
        unexplained.push("series lengths differ".into());
    }
    for (index, (left_run, right_run)) in left_runs.iter().zip(&right_runs).enumerate() {
        compare_run(
            index,
            left_path,
            right_path,
            &left_run,
            &right_run,
            &mut explained,
            &mut unexplained,
        )?;
    }
    Ok(Stage5Comparison {
        schema_version: SERIES_SCHEMA_VERSION,
        comparison: "linux_stage5".into(),
        matches: unexplained.is_empty(),
        explained_differences: explained,
        unexplained_differences: unexplained,
    })
}

fn compare_series_runs(series_path: &Path) -> Result<Stage5Comparison, String> {
    let series = fresh_manifest(series_path)?;
    let runs = load_series_runs(series_path, &series)?;
    compare_run_sequence(series_path, &runs)
}

fn compare_run_sequence(
    series_path: &Path,
    runs: &[RunManifest],
) -> Result<Stage5Comparison, String> {
    if runs.len() < 2 {
        return Err("within-series comparison requires at least two runs".into());
    }
    let mut unexplained = Vec::new();
    let mut explained = Vec::new();
    explained.push("run identity, nonce, raw runner JSON hash, UART hash/path, and wall time are run-specific metadata".into());
    for (index, run) in runs.iter().enumerate().skip(1) {
        compare_run(
            index,
            series_path,
            series_path,
            &runs[0],
            run,
            &mut explained,
            &mut unexplained,
        )?;
    }
    Ok(Stage5Comparison {
        schema_version: SERIES_SCHEMA_VERSION,
        comparison: "linux_stage5_within_series".into(),
        matches: unexplained.is_empty(),
        explained_differences: explained,
        unexplained_differences: unexplained,
    })
}

fn load_series_runs(
    series_path: &Path,
    series: &SeriesManifest,
) -> Result<Vec<RunManifest>, String> {
    if series.scenario_id == S5_08_SCENARIO && series.run_profile != S5_08_RUN_PROFILE {
        return Err("s5-08 requires the stage5_functional run profile".into());
    }
    let mut runs = Vec::new();
    runs.try_reserve(series.runs.len())
        .map_err(|error| format!("series run count cannot be reserved: {error}"))?;
    let mut nonces = std::collections::HashSet::new();
    for reference in &series.runs {
        let run = load_run_manifest(series_path, reference)?;
        if run.series_id != series.series_id
            || run.scenario_id != series.scenario_id
            || run.run_profile != series.run_profile
        {
            return Err("run manifest does not match series identity/profile".into());
        }
        if !nonces.insert(run.nonce.clone()) {
            return Err("series contains duplicate run nonce".into());
        }
        runs.push(run);
    }
    if aggregate_status(
        &runs
            .iter()
            .map(|run| run.scenario_status)
            .collect::<Vec<_>>(),
    ) != series.outcome
    {
        return Err("series outcome does not match run scenario statuses".into());
    }
    Ok(runs)
}

fn load_run_manifest(series_path: &Path, reference: &RunReference) -> Result<RunManifest, String> {
    let root = series_path
        .parent()
        .ok_or("series manifest has no parent")?;
    let relative = Path::new(&reference.manifest);
    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("run manifest path escapes series directory".into());
    }
    let path = safe_relative(root, &reference.manifest)?;
    let bytes = fs::read(&path).map_err(|error| format!("could not read run manifest: {error}"))?;
    if od_testkit::sha256_hex(&bytes) != reference.manifest_sha256 {
        return Err("run manifest hash mismatch".into());
    }
    let mut manifest: RunManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid run manifest JSON: {error}"))?;
    if !supported_schema_version(manifest.schema_version) || manifest.run_id != reference.run_id {
        return Err("run manifest identity or schema mismatch".into());
    }
    if !valid_nonce(&manifest.nonce) {
        return Err("run manifest nonce must be 16..=64 ASCII alphanumeric characters".into());
    }
    let raw_path = safe_relative(root, &manifest.runner.raw_json)?;
    let raw =
        fs::read(raw_path).map_err(|error| format!("could not read raw runner JSON: {error}"))?;
    if od_testkit::sha256_hex(&raw) != manifest.runner.raw_json_sha256 {
        return Err("raw runner JSON hash mismatch".into());
    }
    let raw_value: Value = serde_json::from_slice(&raw)
        .map_err(|error| format!("invalid raw runner JSON: {error}"))?;
    validate_execution_observations_sidecar(root, &manifest.run_id, &raw_value)?;
    validate_nonce_bootconfig(&raw_value, &manifest.nonce)?;
    if raw_value["status"].as_str() != Some(manifest.hardware_status.as_str())
        || raw_value["nonce"].as_str() != Some(manifest.nonce.as_str())
        || raw_value["guest_started"].as_bool() != Some(manifest.runner.guest_started)
        || raw_value["retired"].as_u64() != manifest.runner.retired
        || raw_value.get("stop") != manifest.runner.stop.as_ref()
    {
        return Err("run manifest runner summary does not match raw JSON".into());
    }
    if !configuration_matches_runner(&manifest.configuration, &raw_value)
        || manifest.artifact_hashes != artifact_hashes(&raw_value)
    {
        return Err("run manifest configuration or artifact hashes do not match raw JSON".into());
    }
    enrich_legacy_diagnostic_configuration(&mut manifest.configuration, &raw_value);
    if manifest.machine_stop_reason != machine_stop_reason_from_stop(raw_value.get("stop"))? {
        return Err("run manifest machine stop reason does not match raw runner JSON".into());
    }
    let uart_path = safe_relative(root, &manifest.uart.path)?;
    let uart =
        fs::read(&uart_path).map_err(|error| format!("could not read UART capture: {error}"))?;
    if uart.len() != manifest.uart.byte_count
        || od_testkit::sha256_hex(&uart) != manifest.uart.sha256
    {
        return Err("UART byte count or SHA-256 mismatch".into());
    }
    if manifest.uart.capture_scope != COM1_CAPTURE_SCOPE
        || manifest.uart.transcript_complete_for_run_boot != raw_value["stop"].is_object()
    {
        return Err("UART capture scope/completeness does not match runner stop record".into());
    }
    let raw_com1 = match raw_value["stop"]["com1_hex"].as_str() {
        Some(encoded) => decode_hex(encoded)?,
        None => Vec::new(),
    };
    if uart != raw_com1 {
        return Err("saved UART bytes do not match runner stop record".into());
    }
    let check = |normalize_crlf| {
        validate_scenario_markers(
            &manifest.scenario_id,
            &uart,
            &manifest.nonce,
            &manifest.guest_markers.expected_s5_marker_keys,
            normalize_crlf,
        )
    };
    let canonical = check(true);
    let checked = if canonical.evidence == manifest.guest_markers {
        canonical
    } else {
        let legacy = check(false);
        if legacy.evidence != manifest.guest_markers {
            return Err("recorded guest marker validation does not match UART bytes".into());
        }
        legacy
    };
    let derived_status = scenario_status(
        &manifest.scenario_id,
        &manifest.hardware_status,
        manifest.runner.guest_started,
        &checked.evidence,
        manifest.runner.stop.as_ref(),
        &manifest.expected_stop_kind,
    );
    let derived_reason = scenario_reason(
        &manifest.scenario_id,
        &manifest.hardware_status,
        manifest.runner.guest_started,
        &checked.evidence,
        manifest.runner.stop.as_ref(),
        &manifest.expected_stop_kind,
    );
    if manifest.scenario_status != derived_status || manifest.reason != derived_reason {
        return Err(
            "run scenario status/reason does not match raw runner and marker evidence".into(),
        );
    }
    Ok(manifest)
}

fn safe_relative(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("artifact path escapes series directory".into());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("could not resolve series directory: {error}"))?;
    let resolved = root
        .join(path)
        .canonicalize()
        .map_err(|error| format!("could not resolve series artifact: {error}"))?;
    if !resolved.starts_with(&root) {
        return Err("artifact path resolves outside series directory".into());
    }
    Ok(resolved)
}

fn compare_run(
    index: usize,
    left_series_path: &Path,
    right_series_path: &Path,
    left: &RunManifest,
    right: &RunManifest,
    explained: &mut Vec<String>,
    unexplained: &mut Vec<String>,
) -> Result<(), String> {
    let prefix = format!("run[{index}]");
    if left.host != right.host {
        unexplained.push(format!("{prefix}.host_environment differs"));
    }
    for (field, equal) in [
        ("scenario_id", left.scenario_id == right.scenario_id),
        ("run_profile", left.run_profile == right.run_profile),
        (
            "hardware_status",
            left.hardware_status == right.hardware_status,
        ),
        (
            "scenario_status",
            left.scenario_status == right.scenario_status,
        ),
        (
            "expected_stop_kind",
            left.expected_stop_kind == right.expected_stop_kind,
        ),
        (
            "machine_stop_reason",
            left.machine_stop_reason == right.machine_stop_reason,
        ),
        (
            "artifact_hashes",
            left.artifact_hashes == right.artifact_hashes,
        ),
        (
            "expected_s5_markers",
            left.guest_markers.expected_s5_marker_keys
                == right.guest_markers.expected_s5_marker_keys,
        ),
        (
            "ordered_s5_marker_keys",
            left.guest_markers.ordered_s5_marker_keys == right.guest_markers.ordered_s5_marker_keys,
        ),
        (
            "marker_validation",
            left.guest_markers.valid == right.guest_markers.valid
                && left.guest_markers.errors == right.guest_markers.errors,
        ),
    ] {
        if !equal {
            unexplained.push(format!("{prefix}.{field} differs"));
        }
    }
    let left_config =
        normalize_optional_linux_diagnostics(&normalize_nonce(&left.configuration, &left.nonce));
    let right_config =
        normalize_optional_linux_diagnostics(&normalize_nonce(&right.configuration, &right.nonce));
    if left_config != right_config {
        unexplained.push(format!("{prefix}.configuration differs"));
    }
    let left_uart_path = safe_relative(
        left_series_path
            .parent()
            .ok_or("left series has no parent")?,
        &left.uart.path,
    )?;
    let right_uart_path = safe_relative(
        right_series_path
            .parent()
            .ok_or("right series has no parent")?,
        &right.uart.path,
    )?;
    let left_uart = fs::read(left_uart_path).map_err(|error| error.to_string())?;
    let right_uart = fs::read(right_uart_path).map_err(|error| error.to_string())?;
    let bootconfig_pair = left_config["nonce_transport"] == "bootconfig"
        && right_config["nonce_transport"] == "bootconfig";
    let left_stop = comparable_stop(left, bootconfig_pair);
    let right_stop = comparable_stop(right, bootconfig_pair);
    if bootconfig_pair && left.nonce != right.nonce {
        explained.push(format!(
            "{prefix}.initrd bootconfig trailer carries the per-run nonce"
        ));
    }
    let stage4_nonce_scenario = left_config["nonce_transport"].is_null()
        && stage4_nonce_variation_eligible(left, &left_uart)
        && stage4_nonce_variation_eligible(right, &right_uart)
        && left.host == right.host
        && left.run_profile == right.run_profile
        && left_config == right_config;
    let comparable_left_stop = if stage4_nonce_scenario {
        without_stage4_nonce_volatile_observations(&left_stop)
    } else {
        left_stop.clone()
    };
    let comparable_right_stop = if stage4_nonce_scenario {
        without_stage4_nonce_volatile_observations(&right_stop)
    } else {
        right_stop.clone()
    };
    if comparable_left_stop != comparable_right_stop {
        unexplained.push(format!("{prefix}.machine_observations differs"));
    } else if stage4_nonce_scenario && left_stop != right_stop {
        explained.push(format!(
            "{prefix}.stage4-init-nonce terminal CPU snapshot, timing, IRQ, or quantum observations differ"
        ));
    }
    let left_lines = left
        .guest_markers
        .ordered_s5_marker_lines
        .iter()
        .map(|line| line.replace(&left.nonce, "<nonce>"))
        .collect::<Vec<_>>();
    let right_lines = right
        .guest_markers
        .ordered_s5_marker_lines
        .iter()
        .map(|line| line.replace(&right.nonce, "<nonce>"))
        .collect::<Vec<_>>();
    if left_lines != right_lines {
        unexplained.push(format!("{prefix}.ordered_marker_payloads differs"));
    }
    let left_comparable_uart = if stage4_nonce_scenario {
        normalize_stage4_init_nonce_uart(&left_uart, &left.nonce)
    } else {
        normalize_uart(&left_uart, &left.nonce)
    };
    let right_comparable_uart = if stage4_nonce_scenario {
        normalize_stage4_init_nonce_uart(&right_uart, &right.nonce)
    } else {
        normalize_uart(&right_uart, &right.nonce)
    };
    if left_comparable_uart != right_comparable_uart {
        unexplained.push(format!(
            "{prefix}.ordered_uart_bytes differs after nonce normalization"
        ));
    } else if stage4_nonce_scenario
        && normalize_uart(&left_uart, &left.nonce) != normalize_uart(&right_uart, &right.nonce)
    {
        explained.push(format!(
            "{prefix}.stage4-init-nonce UART timestamps, memory availability, sched_clock calibration, init/nonce console order, audit initialization timestamp, or terminal panic snapshot differ"
        ));
    }
    if left.runner.guest_started != right.runner.guest_started
        || left.runner.retired != right.runner.retired
        || left.runner.exit_code != right.runner.exit_code
    {
        unexplained.push(format!("{prefix}.runner_execution differs"));
    }
    Ok(())
}

fn stage4_nonce_variation_eligible(run: &RunManifest, uart: &[u8]) -> bool {
    let Some(stop) = run.runner.stop.as_ref() else {
        return false;
    };
    let nonce_line = format!("OD_NONCE={}", run.nonce);
    let profile = stop["cpu_profile"].as_str().unwrap_or_default();
    run.scenario_id == "stage4-init-nonce"
        && run.hardware_status == "loaded"
        && run.scenario_status == ScenarioStatus::Passed
        && run.runner.guest_started
        && run.expected_stop_kind == "budget_exhausted"
        && stop["exit_kind"] == "budget_exhausted"
        && stop["user_mode"] == true
        && stop["path"] == "cpl0_set"
        && profile.starts_with("Cpl0ProfileSet (S2-45)")
        && run.configuration["cpu_path"] == "cpl0_set"
        && run.configuration["cpu_profile"] == profile
        && run.configuration["acceptance"] == "counted"
        && run.configuration["diagnostic_past_c7"] == false
        && run.configuration["budget"] == 3_000_000_000u64
        && run.guest_markers.valid
        && run.guest_markers.stage4_nonce_line.as_deref() == Some(nonce_line.as_str())
        && run.guest_markers.expected_s5_marker_keys.is_empty()
        && run.guest_markers.ordered_s5_marker_keys.is_empty()
        && run.guest_markers.errors.is_empty()
        && has_expected_stage4_nonce_terminal_panic(uart, &run.nonce)
        && has_s4_39j_syscall_journal(stop)
}

fn has_s4_39j_syscall_journal(stop: &Value) -> bool {
    const EXPECTED_SYSCALLS: [u64; 9] = [83, 165, 257, 0, 3, 1, 39, 35, 60];
    let Some(journal) = stop.get("syscall_journal") else {
        return false;
    };
    let Some(entries) = journal.get("entries").and_then(Value::as_array) else {
        return false;
    };
    journal["entered"] == 9
        && journal["returned"] == 8
        && entries.len() == EXPECTED_SYSCALLS.len()
        && entries
            .iter()
            .zip(EXPECTED_SYSCALLS)
            .all(|(entry, expected)| entry["nr"] == expected)
        && entries[8].get("rax_return").is_some_and(Value::is_null)
}

fn has_expected_stage4_nonce_terminal_panic(bytes: &[u8], nonce: &str) -> bool {
    let normalized = normalize_uart(bytes, nonce);
    let Ok(text) = String::from_utf8(normalized) else {
        return false;
    };
    let lines = text
        .split_inclusive('\n')
        .map(normalize_stage4_init_nonce_uart_line)
        .collect::<Vec<_>>();
    terminal_stage4_panic_snapshot(&lines).is_some()
}

fn comparable_stop(run: &RunManifest, bootconfig_pair: bool) -> Value {
    let mut value = run.runner.stop.clone().unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("com1_hex");
        object.remove("com1_text");
        object.remove("machine_stop_reason");
        // The loader has verified these duplicate, nonce-bearing initrd digests against
        // the raw bootconfig evidence. The original initramfs hash is compared separately.
        if bootconfig_pair {
            if let Some(inputs) = object.get_mut("inputs").and_then(Value::as_object_mut) {
                for field in NONCE_INITRD_STOP_HASH_FIELDS {
                    inputs.remove(field);
                }
            }
        }
    }
    normalize_nonce(&value, &run.nonce)
}

fn machine_stop_reason_from_stop(
    stop: Option<&Value>,
) -> Result<Option<MachineStopReasonEvidence>, String> {
    let Some(value) = stop.and_then(|stop| stop.get("machine_stop_reason")) else {
        // Old linux-direct JSON did not expose the typed PM request. Keep it unobserved.
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::String(reason) if reason == "reset" => Ok(Some(MachineStopReasonEvidence::Reset)),
        Value::String(reason) if reason == "shutdown" => {
            Ok(Some(MachineStopReasonEvidence::Shutdown))
        }
        _ => Err("stop.machine_stop_reason must be reset, shutdown, or null".into()),
    }
}

fn normalize_nonce(value: &Value, nonce: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(nonce, "<nonce>")),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_nonce(item, nonce))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (key.clone(), normalize_nonce(item, nonce)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn normalize_optional_linux_diagnostics(value: &Value) -> Value {
    let mut normalized = value.clone();
    let Some(configuration) = normalized.as_object_mut() else {
        return normalized;
    };
    for key in ["linux_norandmaps", "linux_no_randomize_kstack_offset"] {
        if matches!(
            configuration.get(key),
            None | Some(Value::Null | Value::Bool(false))
        ) {
            configuration.remove(key);
        }
    }
    normalized
}

fn normalize_uart(bytes: &[u8], nonce: &str) -> Vec<u8> {
    let needle = nonce.as_bytes();
    if needle.is_empty() {
        return bytes.to_vec();
    }
    let replacement = b"<nonce>";
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while let Some(relative) = bytes[cursor..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        let start = cursor + relative;
        normalized.extend_from_slice(&bytes[cursor..start]);
        normalized.extend_from_slice(replacement);
        cursor = start + needle.len();
    }
    normalized.extend_from_slice(&bytes[cursor..]);
    normalized
}

fn without_stage4_nonce_volatile_observations(stop: &Value) -> Value {
    let mut value = stop.clone();
    if let Some(object) = value.as_object_mut() {
        for field in [
            "cr3",
            "last_cr3_change",
            "gpr",
            "rflags",
            "rip",
            "rip_bytes",
            "rip_gpa",
            "halt_idle_ns",
            "virtual_ns",
            "irq0_edges",
            "irq_refused_masked",
            "irq_refused_shadow",
            "irqs_delivered",
            "quanta",
            "quantum_cut_by_deadline",
        ] {
            object.remove(field);
        }
    }
    value
}

fn normalize_stage4_init_nonce_uart(bytes: &[u8], nonce: &str) -> Vec<u8> {
    let nonce_normalized = normalize_uart(bytes, nonce);
    let Ok(text) = String::from_utf8(nonce_normalized.clone()) else {
        return nonce_normalized;
    };
    let mut lines = text
        .split_inclusive('\n')
        .map(normalize_stage4_init_nonce_uart_line)
        .collect::<Vec<_>>();
    normalize_stage4_init_nonce_marker_order(&mut lines);
    if let Some((start, end)) = terminal_stage4_panic_snapshot(&lines)
        && let Some(normalized) = normalize_stage4_panic_snapshot(&lines[start..=end])
    {
        lines.splice(start..end + 1, normalized);
    }
    lines.concat().into_bytes()
}

fn normalize_stage4_init_nonce_marker_order(lines: &mut [String]) {
    const NONCE_MARKER: &str = "OD_NONCE=<nonce>";
    const INIT_LOG: &str = "Run /init as init process";
    let marker = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (uart_line_message(line) == NONCE_MARKER).then_some(index))
        .collect::<Vec<_>>();
    let init_log = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (uart_line_message(line) == INIT_LOG).then_some(index))
        .collect::<Vec<_>>();
    let ([marker], [init_log]) = (marker.as_slice(), init_log.as_slice()) else {
        return;
    };
    // Linux may defer a printk console write while another context holds the console lock.
    // Normalize only this unique adjacent pair; the guest nonce and all other line ordering
    // remain part of the strict comparison.
    if marker.abs_diff(*init_log) == 1 && marker < init_log {
        lines.swap(*marker, *init_log);
    }
}

fn terminal_stage4_panic_snapshot(lines: &[String]) -> Option<(usize, usize)> {
    const PANIC_START: &str =
        "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000";
    const PANIC_END: &str =
        "---[ end Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000 ]---";
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (uart_line_message(line) == PANIC_START).then_some(index))
        .collect::<Vec<_>>();
    let ends = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (uart_line_message(line) == PANIC_END).then_some(index))
        .collect::<Vec<_>>();
    let ([start], [end]) = (starts.as_slice(), ends.as_slice()) else {
        return None;
    };
    (*start < *end
        && *end + 1 == lines.len()
        && *end - *start < 64
        && lines[*start..=*end].iter().map(String::len).sum::<usize>() <= 8192
        && normalize_stage4_panic_snapshot(&lines[*start..=*end]).is_some())
    .then_some((*start, *end))
}

fn normalize_stage4_init_nonce_uart_line(line: &str) -> String {
    let (body, ending) = if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    };
    let (prefix, message) = if let Some(message) = kernel_timestamp_message(body) {
        ("[<kernel-time>] ", message)
    } else {
        ("", body)
    };
    let message = normalize_memory_available(message);
    let message = normalize_sched_clock_calibration(&message);
    let message = normalize_stage4_audit_init_timestamp(&message);
    format!("{prefix}{message}{ending}")
}

fn normalize_stage4_audit_init_timestamp(line: &str) -> String {
    const PREFIX: &str = "audit: type=2000 audit(";
    const SUFFIX: &str = "): state=initialized audit_enabled=0 res=1";
    let Some(timestamp) = line
        .strip_prefix(PREFIX)
        .and_then(|value| value.strip_suffix(SUFFIX))
    else {
        return line.to_string();
    };
    let Some((time, serial)) = timestamp.split_once(':') else {
        return line.to_string();
    };
    let Some((seconds, fraction)) = time.split_once('.') else {
        return line.to_string();
    };
    if serial != "1"
        || seconds.is_empty()
        || fraction.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return line.to_string();
    }
    format!("{PREFIX}<event-time>:1{SUFFIX}")
}

fn kernel_timestamp_message(line: &str) -> Option<&str> {
    let close = line.strip_prefix('[')?.find("] ")? + 1;
    let timestamp = line.get(1..close)?;
    let (seconds, fraction) = timestamp.trim().split_once('.')?;
    if seconds.is_empty()
        || fraction.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    line.get(close + 2..)
}

fn normalize_memory_available(line: &str) -> String {
    let Some(memory) = line.strip_prefix("Memory: ") else {
        return line.to_string();
    };
    let Some(k_slash) = memory.find("K/") else {
        return line.to_string();
    };
    let count = &memory[..k_slash];
    if count.is_empty() || !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return line.to_string();
    }
    format!("Memory: <available>K/{}", &memory[k_slash + 2..])
}

fn normalize_sched_clock_calibration(line: &str) -> String {
    const PREFIX: &str = "sched_clock: Marking stable (";
    let Some(calibration) = line.strip_prefix(PREFIX) else {
        return line.to_string();
    };
    let Some(calibration) = calibration.strip_suffix(')') else {
        return line.to_string();
    };
    let Some((first, second)) = calibration.split_once(")->(") else {
        return line.to_string();
    };
    if !is_integer_pair(first) || !is_integer_pair(second) {
        return line.to_string();
    }
    format!("{PREFIX}<calibration>)")
}

fn is_integer_pair(pair: &str) -> bool {
    let Some((first, second)) = pair.split_once(", ") else {
        return false;
    };
    fn integer(value: &str) -> bool {
        let digits = value.strip_prefix('-').unwrap_or(value);
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    }
    integer(first) && integer(second) && !second.contains(", ")
}

fn normalize_stage4_panic_snapshot(lines: &[String]) -> Option<Vec<String>> {
    const PANIC_START: &str =
        "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000";
    const PANIC_END: &str =
        "---[ end Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000 ]---";
    if lines.len() < 2
        || uart_line_message(lines.first()?) != PANIC_START
        || uart_line_message(lines.last()?) != PANIC_END
    {
        return None;
    }
    let mut normalized = Vec::new();
    for line in lines {
        let message = uart_line_message(line);
        if let Some((symbol, true)) = panic_frame(message) {
            if optional_panic_frame(symbol) {
                continue;
            }
            return None;
        }
        if !panic_snapshot_line_allowed(message) {
            return None;
        }
        if let Some(value) = normalize_panic_rsp(message) {
            normalized.push(line.replace(message, &value));
        } else {
            normalized.push(line.clone());
        }
    }
    Some(normalized)
}

fn panic_snapshot_line_allowed(line: &str) -> bool {
    const PANIC_START: &str =
        "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000";
    const PANIC_END: &str =
        "---[ end Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000 ]---";
    const SHARED_FRAMES: &[&str] = &[
        "show_stack",
        "dump_stack_lvl",
        "dump_stack",
        "vpanic",
        "panic",
        "do_exit.cold",
        "__x64_sys_exit",
        "x64_sys_call",
        "do_syscall_64",
        "entry_SYSCALL_64_after_hwframe",
    ];
    let content = line.strip_prefix(' ').unwrap_or(line);
    if matches!(
        content,
        PANIC_START | PANIC_END | "Call Trace:" | "<TASK>" | "</TASK>" | "Kernel Offset: disabled"
    ) {
        return true;
    }
    if content.starts_with(
        "CPU: 0 UID: 0 PID: 1 Comm: init Not tainted 7.0.0-38-generic #38-Ubuntu PREEMPT(lazy)",
    ) {
        return true;
    }
    if let Some((symbol, optional)) = panic_frame(line) {
        return if optional {
            optional_panic_frame(symbol)
        } else {
            SHARED_FRAMES.contains(&symbol)
        };
    }
    if let Some(address) = content.strip_prefix("RIP: 0033:0x") {
        return is_hex_digits(address);
    }
    if let Some(code) = content.strip_prefix("Code: ") {
        return !code.is_empty()
            && code
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b' ' | b'<' | b'>'));
    }
    if normalize_panic_rsp(content).is_some() {
        return true;
    }
    valid_panic_register_line(content)
}

fn panic_frame(line: &str) -> Option<(&str, bool)> {
    let line = line.strip_prefix(' ')?;
    if line.starts_with(' ') {
        return None;
    }
    let (optional, frame) = if let Some(frame) = line.strip_prefix("? ") {
        (true, frame)
    } else {
        (false, line)
    };
    let (symbol, offsets) = frame.split_once('+')?;
    let (instruction, function) = offsets.split_once('/')?;
    let valid_offset = |value: &str| value.strip_prefix("0x").is_some_and(is_hex_digits);
    if symbol.is_empty() || !valid_offset(instruction) || !valid_offset(function) {
        return None;
    }
    Some((symbol, optional))
}

fn optional_panic_frame(symbol: &str) -> bool {
    matches!(
        symbol,
        "__task_pid_nr_ns"
            | "acct_update_integrals"
            | "ksys_write"
            | "arch_exit_to_user_mode_prepare.isra.0"
            | "do_syscall_64"
            | "exc_page_fault"
            | "handle_pte_fault"
            | "__pmd_alloc"
            | "__handle_mm_fault"
            | "count_memcg_events"
            | "handle_mm_fault"
            | "irqentry_exit"
    )
}

fn normalize_panic_rsp(line: &str) -> Option<String> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 6
        || fields[0] != "RSP:"
        || !fields[1].starts_with("002b:")
        || !is_hex_digits(fields[1].strip_prefix("002b:")?)
        || fields[2] != "EFLAGS:"
        || !is_hex_digits(fields[3])
        || fields[4] != "ORIG_RAX:"
        || !is_hex_digits(fields[5])
    {
        return None;
    }
    Some(format!(
        "RSP: 002b:<stack> EFLAGS: {} ORIG_RAX: {}",
        fields[3], fields[5]
    ))
}

fn valid_panic_register_line(line: &str) -> bool {
    const REGISTERS: &[&[&str]] = &[
        &["RAX:", "RBX:", "RCX:"],
        &["RDX:", "RSI:", "RDI:"],
        &["RBP:", "R08:", "R09:"],
        &["R10:", "R11:", "R12:"],
        &["R13:", "R14:", "R15:"],
    ];
    let fields = line.split_whitespace().collect::<Vec<_>>();
    REGISTERS.iter().any(|registers| {
        fields.len() == registers.len() * 2
            && registers.iter().enumerate().all(|(index, register)| {
                fields[index * 2] == *register && is_hex_digits(fields[index * 2 + 1])
            })
    })
}

fn is_hex_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn uart_line_message(line: &str) -> &str {
    let body = line
        .strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line);
    body.strip_prefix("[<kernel-time>] ").unwrap_or(body)
}

pub(crate) fn compare_stage5_cli(args: &[String]) -> ExitCode {
    if args
        .iter()
        .filter(|argument| argument.as_str() == "--linux-stage5")
        .count()
        != 1
        || args
            .iter()
            .filter(|argument| argument.as_str() == "--json")
            .count()
            != 1
        || args.iter().any(|argument| argument.as_str() == "--case")
        || args.iter().any(|argument| {
            argument.starts_with("--") && !matches!(argument.as_str(), "--linux-stage5" | "--json")
        })
    {
        eprintln!("invalid input: --compare-results A.json [B.json] --linux-stage5 --json");
        return ExitCode::from(3);
    }
    let paths = args
        .iter()
        .filter(|argument| !argument.starts_with("--"))
        .collect::<Vec<_>>();
    let report = match paths.as_slice() {
        [series] => compare_series_runs(Path::new(series)),
        [left, right] => compare_series_paths(Path::new(left), Path::new(right)),
        _ => {
            eprintln!(
                "invalid input: --linux-stage5 comparison requires one or two series.json paths"
            );
            return ExitCode::from(3);
        }
    };
    let report = match report {
        Ok(value) => value,
        Err(error) => {
            eprintln!("invalid Stage 5 series comparison input: {error}");
            return ExitCode::from(3);
        }
    };
    let exit = if report.matches { 0 } else { 1 };
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serializable comparison")
    );
    ExitCode::from(exit)
}

fn invalid_input(detail: &str) -> ExitCode {
    eprintln!("invalid input: {detail}");
    ExitCode::from(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_tsc_summary_validator_checks_histograms_and_partial_call_status() {
        let raw = json!({
            "rdtsc_cpl0_sites": [
                {"rip": format!("0x{:x}", od_machine::LINUX_DELAY_TSC_ENTRY_RIP), "reads": 1},
                {"rip": format!("0x{:x}", od_machine::LINUX_DELAY_TSC_POLL_RIP), "reads": 1}
            ],
            "rdtsc_site_samples": [],
            "delay_tsc_call_summary": {
                "entry_rip": format!("0x{:x}", od_machine::LINUX_DELAY_TSC_ENTRY_RIP),
                "poll_rip": format!("0x{:x}", od_machine::LINUX_DELAY_TSC_POLL_RIP),
                "entry_count": 1,
                "entries_omitted": 0,
                "poll_count": 1,
                "unmatched_polls": 0,
                "ambiguous_polls": 0,
                "entries": [{
                    "ordinal": 1,
                    "requested_cycles": "0x64",
                    "entry_tsc": "0x1000",
                    "cr3": null,
                    "rbp": "0x8000",
                    "rsp": "0x7ff0",
                    "frame_links": [],
                    "frame_chain_end": "read_failed",
                    "frame_chain_incomplete": true,
                    "poll_count": 1,
                    "first_poll_ordinal": 1,
                    "last_poll_ordinal": 1,
                    "last_elapsed_cycles": "0x64",
                    "terminal_threshold_seen": true,
                    "poll_sequence_status": "threshold_seen"
                }]
            }
        });
        let metadata = json!({
            "delay_tsc_call_summary_entry_count": 1,
            "delay_tsc_call_summary_retained_count": 1,
            "delay_tsc_call_summary_entries_omitted": 0,
            "delay_tsc_call_summary_poll_count": 1,
            "delay_tsc_call_summary_unmatched_polls": 0,
            "delay_tsc_call_summary_ambiguous_polls": 0
        });
        assert!(valid_delay_tsc_call_summary(&raw, &metadata));

        let mut malformed = raw.clone();
        malformed["delay_tsc_call_summary"]["unmatched_polls"] = json!(1);
        assert!(!valid_delay_tsc_call_summary(&malformed, &metadata));

        let mut duplicate_histogram_site = raw.clone();
        let duplicate_row = duplicate_histogram_site["rdtsc_cpl0_sites"][0].clone();
        duplicate_histogram_site["rdtsc_cpl0_sites"]
            .as_array_mut()
            .unwrap()
            .push(duplicate_row);
        assert!(!valid_delay_tsc_call_summary(
            &duplicate_histogram_site,
            &metadata
        ));

        let mut malformed_histogram_site = raw.clone();
        malformed_histogram_site["rdtsc_cpl0_sites"][0]["reads"] = json!("1");
        assert!(!valid_delay_tsc_call_summary(
            &malformed_histogram_site,
            &metadata
        ));

        let mut impossible_span = raw.clone();
        impossible_span["delay_tsc_call_summary"]["poll_count"] = json!(3);
        impossible_span["rdtsc_cpl0_sites"][1]["reads"] = json!(3);
        impossible_span["delay_tsc_call_summary"]["entries"][0]["poll_count"] = json!(3);
        impossible_span["delay_tsc_call_summary"]["entries"][0]["last_poll_ordinal"] = json!(2);
        let mut impossible_span_metadata = metadata.clone();
        impossible_span_metadata["delay_tsc_call_summary_poll_count"] = json!(3);
        assert!(!valid_delay_tsc_call_summary(
            &impossible_span,
            &impossible_span_metadata
        ));

        let mut single_poll_span = raw.clone();
        single_poll_span["delay_tsc_call_summary"]["poll_count"] = json!(2);
        single_poll_span["delay_tsc_call_summary"]["unmatched_polls"] = json!(1);
        single_poll_span["rdtsc_cpl0_sites"][1]["reads"] = json!(2);
        single_poll_span["delay_tsc_call_summary"]["entries"][0]["last_poll_ordinal"] = json!(2);
        let mut single_poll_span_metadata = metadata.clone();
        single_poll_span_metadata["delay_tsc_call_summary_poll_count"] = json!(2);
        single_poll_span_metadata["delay_tsc_call_summary_unmatched_polls"] = json!(1);
        assert!(!valid_delay_tsc_call_summary(
            &single_poll_span,
            &single_poll_span_metadata
        ));

        let mut partial = raw;
        partial["delay_tsc_call_summary"]["poll_count"] = json!(0);
        partial["delay_tsc_call_summary"]["unmatched_polls"] = json!(0);
        partial["rdtsc_cpl0_sites"]
            .as_array_mut()
            .unwrap()
            .remove(1);
        partial["delay_tsc_call_summary"]["entries"][0]["poll_count"] = json!(0);
        partial["delay_tsc_call_summary"]["entries"][0]["first_poll_ordinal"] = Value::Null;
        partial["delay_tsc_call_summary"]["entries"][0]["last_poll_ordinal"] = Value::Null;
        partial["delay_tsc_call_summary"]["entries"][0]["last_elapsed_cycles"] = Value::Null;
        partial["delay_tsc_call_summary"]["entries"][0]["terminal_threshold_seen"] = json!(false);
        partial["delay_tsc_call_summary"]["entries"][0]["poll_sequence_status"] = json!("partial");
        let mut partial_metadata = metadata;
        partial_metadata["delay_tsc_call_summary_poll_count"] = json!(0);
        assert!(valid_delay_tsc_call_summary(&partial, &partial_metadata));
    }

    #[test]
    fn delay_tsc_summary_serialization_round_trips_into_strict_validator() {
        let path = std::env::temp_dir().join(format!(
            "od-runner-delay-call-summary-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let observations = od_machine::LinuxExecutionObservations {
            rdtsc_cpl0_sites: vec![
                od_machine::RdtscSiteCount {
                    rip: od_machine::LINUX_DELAY_TSC_ENTRY_RIP,
                    reads: 1,
                },
                od_machine::RdtscSiteCount {
                    rip: od_machine::LINUX_DELAY_TSC_POLL_RIP,
                    reads: 1,
                },
            ],
            rdtsc_sample_rip: Some(od_machine::LINUX_DELAY_TSC_POLL_RIP),
            rdtsc_sample_stride: 1,
            rdtsc_sample_site_hits: 1,
            rdtsc_entry_sample_rip: Some(od_machine::LINUX_DELAY_TSC_ENTRY_RIP),
            rdtsc_entry_sample_hits: 1,
            delay_tsc_call_summary: Some(od_machine::DelayTscCallSummaryCollection {
                entry_rip: od_machine::LINUX_DELAY_TSC_ENTRY_RIP,
                poll_rip: od_machine::LINUX_DELAY_TSC_POLL_RIP,
                entry_count: 1,
                entries_omitted: 0,
                poll_count: 1,
                unmatched_polls: 0,
                ambiguous_polls: 0,
                entries: vec![od_machine::DelayTscCallSummary {
                    ordinal: 1,
                    requested_cycles: 100,
                    entry_tsc: 1_000,
                    cr3: Some(0x5000),
                    rbp: 0x8000,
                    rsp: 0x7ff0,
                    frame_links: vec![od_machine::ConsoleContextFrame {
                        frame_pointer: 0x8000,
                        return_address: 0xffff_ffff_8100_1234,
                    }],
                    frame_chain_end: od_machine::FrameChainEnd::ReadFailed,
                    poll_count: 1,
                    first_poll_ordinal: Some(1),
                    last_poll_ordinal: Some(1),
                    last_elapsed_cycles: Some(25),
                    terminal_threshold_seen: false,
                    poll_sequence_status: od_machine::DelayTscPollSequenceStatus::Partial,
                }],
            }),
            ..Default::default()
        };
        let (byte_count, sha256) = super::super::write_execution_observations_sidecar(
            &mut file,
            path.to_str().unwrap(),
            &observations,
        )
        .unwrap();
        let bytes = fs::read(&path).unwrap();
        let raw: Value = serde_json::from_slice(&bytes).unwrap();
        let metadata = super::super::execution_observations_sidecar_metadata(
            path.to_str().unwrap(),
            &observations,
            byte_count,
            sha256,
        );
        assert_eq!(raw["schema_version"], 21);
        assert_eq!(
            raw["delay_tsc_call_summary"]["entries"][0]["frame_chain_end"],
            "read_failed"
        );
        assert_eq!(
            raw["delay_tsc_call_summary"]["entries"][0]["frame_chain_incomplete"],
            true
        );
        assert!(valid_delay_tsc_call_summary(&raw, &metadata));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn delay_tsc_summary_loader_accepts_zero_entries_and_entry_without_polls() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-delay-call-empty-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let series_path = write_test_series(&temp, "delay-empty", "nonceDelayEmpty001", "PASS");
        let root = series_path.parent().unwrap();
        let run_id = "delay-empty-run-0001";
        let sidecar_path = root
            .join("runs")
            .join(run_id)
            .join("execution-observations.json");
        let relative_sidecar = format!("runs/{run_id}/execution-observations.json");

        let observations_for = |include_entry: bool| {
            let entries = if include_entry {
                vec![od_machine::DelayTscCallSummary {
                    ordinal: 1,
                    requested_cycles: 100,
                    entry_tsc: 1_000,
                    cr3: Some(0x5000),
                    rbp: 0x8000,
                    rsp: 0x7ff0,
                    frame_links: vec![],
                    frame_chain_end: od_machine::FrameChainEnd::NullFramePointer,
                    poll_count: 0,
                    first_poll_ordinal: None,
                    last_poll_ordinal: None,
                    last_elapsed_cycles: None,
                    terminal_threshold_seen: false,
                    poll_sequence_status: od_machine::DelayTscPollSequenceStatus::Partial,
                }]
            } else {
                vec![]
            };
            let entry_samples = if include_entry {
                vec![od_machine::RdtscEntrySample {
                    rip: od_machine::LINUX_DELAY_TSC_ENTRY_RIP,
                    hit_ordinal: 1,
                    requested_cycles: 100,
                    observed_tsc: 1_000,
                    rbp: 0x8000,
                    rsp: 0x7ff0,
                    cr3: Some(0x5000),
                }]
            } else {
                vec![]
            };
            od_machine::LinuxExecutionObservations {
                rdtsc_cpl0_sites: if include_entry {
                    vec![od_machine::RdtscSiteCount {
                        rip: od_machine::LINUX_DELAY_TSC_ENTRY_RIP,
                        reads: 1,
                    }]
                } else {
                    vec![]
                },
                rdtsc_sample_rip: Some(od_machine::LINUX_DELAY_TSC_POLL_RIP),
                rdtsc_sample_stride: 1,
                rdtsc_entry_sample_rip: Some(od_machine::LINUX_DELAY_TSC_ENTRY_RIP),
                rdtsc_entry_sample_hits: u64::from(include_entry),
                rdtsc_entry_samples: entry_samples,
                rdtsc_entry_clock_trace_missing: if include_entry { 1 } else { 0 },
                delay_tsc_call_summary: Some(od_machine::DelayTscCallSummaryCollection {
                    entry_rip: od_machine::LINUX_DELAY_TSC_ENTRY_RIP,
                    poll_rip: od_machine::LINUX_DELAY_TSC_POLL_RIP,
                    entry_count: u64::from(include_entry),
                    entries_omitted: 0,
                    poll_count: 0,
                    unmatched_polls: 0,
                    ambiguous_polls: 0,
                    entries,
                }),
                ..Default::default()
            }
        };

        let write_and_load = |observations: &od_machine::LinuxExecutionObservations,
                              create: bool| {
            let mut options = OpenOptions::new();
            options.write(true);
            if create {
                options.create_new(true);
            } else {
                options.truncate(true);
            }
            let mut file = options.open(&sidecar_path).unwrap();
            let (byte_count, sha256) = super::super::write_execution_observations_sidecar(
                &mut file,
                sidecar_path.to_str().unwrap(),
                observations,
            )
            .unwrap();
            drop(file);
            let metadata = super::super::execution_observations_sidecar_metadata(
                &relative_sidecar,
                observations,
                byte_count,
                sha256,
            );
            rewrite_raw_and_series(&series_path, |raw| {
                raw["execution_observations_sidecar"] = metadata;
            });
            let manifest = fresh_manifest(&series_path).unwrap();
            load_run_manifest(&series_path, &manifest.runs[0]).unwrap();
        };

        write_and_load(&observations_for(false), true);
        write_and_load(&observations_for(true), false);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn nonce_bootconfig_verifies_the_exact_nonce_and_the_loaded_initrd() {
        let base = [0u8; 3];
        let nonce = "Nonce-1";
        let trailer = super::super::nonce_bootconfig_trailer(nonce, base.len() as u64);
        let loaded = [base.as_slice(), trailer.as_slice()].concat();
        let base_hash = od_testkit::sha256_hex(&base);
        let loaded_hash = od_testkit::sha256_hex(&loaded);
        let cmdline = super::super::nonce_bootconfig_cmdline();
        let raw = json!({
            "status": "loaded",
            "cmdline": cmdline,
            "cmdline_in_memory": cmdline,
            "cmdline_in_memory_matches": true,
            "memory_sha256": {"initrd": loaded_hash, "initrd_matches_file": true},
            "stop": {"inputs": {
                "initramfs_sha256": loaded_hash,
                "initramfs_sha256_in_memory": loaded_hash,
            }},
            "inputs": {"initramfs": {
                "bytes": base.len(),
                "sha256": base_hash,
                "expected_sha256": base_hash,
                "nonce_bootconfig": {
                    "loaded_bytes": loaded.len(),
                    "loaded_sha256": loaded_hash,
                    "trailer_sha256": od_testkit::sha256_hex(&trailer),
                },
            }},
        });
        assert!(validate_nonce_bootconfig(&raw, nonce).is_ok());
        assert!(validate_nonce_bootconfig(&raw, "Nonce-2").is_err());
        for field in NONCE_INITRD_STOP_HASH_FIELDS {
            let mut bad = raw.clone();
            bad["stop"]["inputs"][field] = json!(base_hash);
            assert!(validate_nonce_bootconfig(&bad, nonce).is_err());
        }
        for (key, value) in [
            ("initrd", json!(base_hash)),
            ("initrd_matches_file", json!(false)),
        ] {
            let mut bad = raw.clone();
            bad["memory_sha256"][key] = value;
            assert!(validate_nonce_bootconfig(&bad, nonce).is_err());
        }
    }

    #[test]
    fn automatic_series_configuration_records_linux_boot_diagnostics() {
        let configuration = configuration_from_runner(&json!({
            "cmdline": "nokaslr norandmaps randomize_kstack_offset=off",
            "ram_mib": 128,
            "stop": {"budget": 3000000000u64, "path": "cpl0_set", "cpu_profile": "stage4a"},
            "diagnostic_past_c7": false,
            "linux_norandmaps": true,
            "linux_no_randomize_kstack_offset": true,
            "acceptance": "diagnostic_not_counted"
        }));
        assert_eq!(configuration["linux_norandmaps"], true);
        assert_eq!(configuration["linux_no_randomize_kstack_offset"], true);
        assert_eq!(configuration["acceptance"], "diagnostic_not_counted");
    }

    #[test]
    fn seeded_configuration_difference_is_visible_with_identical_stop_and_uart() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage6-seed-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = write_test_series(&temp, "seed", "nonceStage6Seed0001", "PASS");
        let manifest = fresh_manifest(&path).unwrap();
        let mut left = load_run_manifest(&path, &manifest.runs[0]).unwrap();
        let raw_path = path.parent().unwrap().join(&left.runner.raw_json);
        let mut raw: Value = serde_json::from_slice(&fs::read(raw_path).unwrap()).unwrap();
        assert!(left.configuration.get("interleaved_scheduler").is_none());
        raw["interleaved_scheduler"] = json!({"seed": 0, "vcpus": 1});
        assert!(!configuration_matches_runner(&left.configuration, &raw));
        left.configuration = configuration_from_runner(&raw);
        assert!(configuration_matches_runner(&left.configuration, &raw));

        let mut right = left.clone();
        raw["interleaved_scheduler"]["seed"] = json!(1);
        right.configuration = configuration_from_runner(&raw);
        let (mut explained, mut unexplained) = (Vec::new(), Vec::new());
        compare_run(
            0,
            &path,
            &path,
            &left,
            &right,
            &mut explained,
            &mut unexplained,
        )
        .unwrap();
        assert_eq!(unexplained, ["run[0].configuration differs"]);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn comparison_normalizes_only_inactive_linux_diagnostic_configuration() {
        let legacy = json!({
            "cmdline": "nokaslr norandmaps",
            "ram_mib": 128,
            "linux_norandmaps": true
        });
        let current_equivalent = json!({
            "cmdline": "nokaslr norandmaps",
            "ram_mib": 128,
            "linux_norandmaps": true,
            "linux_no_randomize_kstack_offset": false
        });
        assert_eq!(
            normalize_optional_linux_diagnostics(&legacy),
            normalize_optional_linux_diagnostics(&current_equivalent)
        );

        let true_flag_without_suffix = json!({
            "cmdline": "nokaslr norandmaps",
            "ram_mib": 128,
            "linux_norandmaps": true,
            "linux_no_randomize_kstack_offset": true
        });
        assert_ne!(
            normalize_optional_linux_diagnostics(&legacy),
            normalize_optional_linux_diagnostics(&true_flag_without_suffix)
        );

        let active_diagnostic = json!({
            "cmdline": "nokaslr norandmaps randomize_kstack_offset=off",
            "ram_mib": 128,
            "linux_norandmaps": true,
            "linux_no_randomize_kstack_offset": true
        });
        assert_ne!(
            normalize_optional_linux_diagnostics(&legacy),
            normalize_optional_linux_diagnostics(&active_diagnostic)
        );
    }

    #[test]
    fn build_source_revision_is_a_git_hash_when_available() {
        if let Some(revision) = build_source_revision() {
            assert!(matches!(revision.len(), 40 | 64));
            assert!(revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn build_source_metadata_parser_preserves_available_and_unavailable_values() {
        let revision = "0123456789012345678901234567890123456789";
        assert_eq!(
            source_revision_from_env(Some(revision)).as_deref(),
            Some(revision)
        );
        assert_eq!(source_revision_from_env(Some("unknown")), None);
        assert_eq!(source_revision_from_env(None), None);
        assert_eq!(source_dirty_from_env(Some("true")), Some(true));
        assert_eq!(source_dirty_from_env(Some("false")), Some(false));
        assert_eq!(source_dirty_from_env(Some("unknown")), None);
        assert_eq!(source_dirty_from_env(None), None);
    }

    #[test]
    fn legacy_v1_host_metadata_defaults_dirty_state_to_unobserved() {
        let host: HostEvidence = serde_json::from_value(json!({
            "os":"linux",
            "arch":"x86_64",
            "runner_version":"0.1.0",
            "source_revision":"0123456789012345678901234567890123456789"
        }))
        .unwrap();
        assert_eq!(host.source_dirty, None);
        assert!(supported_schema_version(1));
        assert!(supported_schema_version(SERIES_SCHEMA_VERSION));
        assert!(!supported_schema_version(0));
        for version in 1..=13 {
            assert!(supported_execution_observations_schema_version(version));
        }
        assert!(supported_execution_observations_schema_version(14));
        assert!(supported_execution_observations_schema_version(15));
        assert!(supported_execution_observations_schema_version(16));
        assert!(supported_execution_observations_schema_version(17));
        assert!(supported_execution_observations_schema_version(18));
        assert!(supported_execution_observations_schema_version(19));
        assert!(supported_execution_observations_schema_version(20));
        assert!(supported_execution_observations_schema_version(21));
        assert!(!supported_execution_observations_schema_version(u64::MAX));
    }

    #[test]
    fn elf_stack_copy_probe_cap_is_rejected_when_exceeded() {
        let within_cap = vec![Value::Null; od_machine::LINUX_ELF_STACK_COPY_PROBE_CAPACITY];
        let over_cap = vec![Value::Null; od_machine::LINUX_ELF_STACK_COPY_PROBE_CAPACITY + 1];
        assert!(elf_stack_copy_probe_count_valid(&within_cap));
        assert!(!elf_stack_copy_probe_count_valid(&over_cap));
    }

    #[test]
    fn elf_stack_copy_target_cr3_event_validates_not_seen_and_chain_outcomes() {
        let not_seen = json!({
            "events": [],
            "target_event": {
                "status": "not_seen", "sequence": null, "rip": null, "cr3": null,
                "retired_instructions": null, "rsp": null, "rbp": null,
                "chain_status": null, "frames": [], "error": null
            }
        });
        assert!(valid_elf_stack_copy_target_cr3_event(&not_seen));

        let event = json!({
            "sequence": 26596,
            "rip": "0xffffffff8141d753",
            "old_cr3": "0x1000",
            "new_cr3": "0x2000",
            "cpl": 0,
            "retired_instructions_through_current": 456
        });
        let base_target = json!({
            "status": "captured", "sequence": 26596, "rip": "0xffffffff8141d753",
            "cr3": "0x2000", "retired_instructions": 456, "rsp": "0x8000",
            "rbp": "0x8100", "chain_status": "complete", "frames": [], "error": null
        });
        let mut captured = json!({"events": [event], "target_event": base_target});
        assert!(!valid_elf_stack_copy_target_cr3_event(&captured));
        captured["target_event"]["rbp"] = json!("0x0");
        assert!(valid_elf_stack_copy_target_cr3_event(&captured));

        let mut unreadable = captured.clone();
        unreadable["target_event"]["chain_status"] = json!("unreadable");
        unreadable["target_event"]["error"] = json!("read-only RAM read failed");
        unreadable["target_event"]["rbp"] = json!("0x8100");
        assert!(valid_elf_stack_copy_target_cr3_event(&unreadable));

        let mut malformed = captured.clone();
        malformed["target_event"]["chain_status"] = json!("malformed");
        malformed["target_event"]["error"] = json!("invalid frame pointer");
        malformed["target_event"]["rbp"] = json!("0x8101");
        assert!(valid_elf_stack_copy_target_cr3_event(&malformed));
        malformed["target_event"]["rbp"] = json!("0x8100");
        assert!(!valid_elf_stack_copy_target_cr3_event(&malformed));
        let mut zero_rbp_unreadable = captured.clone();
        zero_rbp_unreadable["target_event"]["chain_status"] = json!("unreadable");
        zero_rbp_unreadable["target_event"]["error"] = json!("unmapped");
        assert!(!valid_elf_stack_copy_target_cr3_event(&zero_rbp_unreadable));
        let mut zero_rbp_malformed = captured.clone();
        zero_rbp_malformed["target_event"]["chain_status"] = json!("malformed");
        zero_rbp_malformed["target_event"]["error"] = json!("bad chain");
        assert!(!valid_elf_stack_copy_target_cr3_event(&zero_rbp_malformed));

        let mut wrong_sequence = captured.clone();
        wrong_sequence["target_event"]["sequence"] = json!(26595);
        assert!(!valid_elf_stack_copy_target_cr3_event(&wrong_sequence));
        let mut wrong_rip = captured.clone();
        wrong_rip["target_event"]["rip"] = json!("0xffffffff8141d752");
        assert!(!valid_elf_stack_copy_target_cr3_event(&wrong_rip));

        let frames = (0..od_machine::LINUX_ELF_STACK_COPY_RBP_FRAME_CAPACITY)
            .map(|index| {
                let rbp = 0x1000 + index as u64 * 0x10;
                json!({
                    "rbp": format!("0x{rbp:x}"),
                    "saved_rbp_slot": format!("0x{rbp:x}"),
                    "saved_rbp": format!("0x{:x}", rbp + 0x10),
                    "return_address_slot": format!("0x{:x}", rbp + 8),
                    "return_address": "0xffffffff81000000"
                })
            })
            .collect::<Vec<_>>();
        let mut frame_limit = captured.clone();
        frame_limit["target_event"]["rbp"] = json!("0x1000");
        frame_limit["target_event"]["chain_status"] = json!("frame_limit");
        frame_limit["target_event"]["frames"] = json!(frames);
        frame_limit["target_event"]["error"] = json!("frame cap");
        assert!(valid_elf_stack_copy_target_cr3_event(&frame_limit));
        frame_limit["target_event"]["frames"][7]["saved_rbp"] = json!("0xfffffffffffffff8");
        assert!(valid_elf_stack_copy_target_cr3_event(&frame_limit));
        let mut malformed_payload_at_frame_cap = frame_limit.clone();
        malformed_payload_at_frame_cap["target_event"]["chain_status"] = json!("malformed");
        malformed_payload_at_frame_cap["target_event"]["error"] =
            json!("malformed saved RBP in eighth frame");
        malformed_payload_at_frame_cap["target_event"]["frames"][7]["saved_rbp"] = json!("0x1081");
        assert!(valid_elf_stack_copy_target_cr3_event(
            &malformed_payload_at_frame_cap
        ));
        let mut malformed_at_frame_cap = frame_limit.clone();
        malformed_at_frame_cap["target_event"]["chain_status"] = json!("malformed");
        malformed_at_frame_cap["target_event"]["error"] = json!("forged next slot overflow");
        assert!(!valid_elf_stack_copy_target_cr3_event(
            &malformed_at_frame_cap
        ));
        frame_limit["target_event"]["frames"][7]["saved_rbp"] = json!("0x1080");
        let mut unreadable_after_frame_cap = frame_limit.clone();
        unreadable_after_frame_cap["target_event"]["chain_status"] = json!("unreadable");
        unreadable_after_frame_cap["target_event"]["error"] = json!("forged read error");
        assert!(!valid_elf_stack_copy_target_cr3_event(
            &unreadable_after_frame_cap
        ));
        frame_limit["target_event"]["frames"][7]["saved_rbp"] = json!("0x0");
        assert!(!valid_elf_stack_copy_target_cr3_event(&frame_limit));
        frame_limit["target_event"]["frames"][7]["saved_rbp"] = json!("0x1080");
        frame_limit["target_event"]["frames"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(!valid_elf_stack_copy_target_cr3_event(&frame_limit));

        let good_prefix = json!([
            {"rbp":"0x1000", "saved_rbp_slot":"0x1000", "saved_rbp":"0x1010", "return_address_slot":"0x1008", "return_address":"0xffffffff81000000"},
            {"rbp":"0x1010", "saved_rbp_slot":"0x1010", "saved_rbp":"0x1011", "return_address_slot":"0x1018", "return_address":"0xffffffff81000010"}
        ]);
        let mut malformed_terminal = captured.clone();
        malformed_terminal["target_event"]["rbp"] = json!("0x1000");
        malformed_terminal["target_event"]["chain_status"] = json!("malformed");
        malformed_terminal["target_event"]["frames"] = good_prefix;
        malformed_terminal["target_event"]["error"] = json!("malformed saved RBP");
        assert!(valid_elf_stack_copy_target_cr3_event(&malformed_terminal));
        malformed_terminal["target_event"]["frames"][0]["return_address"] = json!("0x800000000000");
        assert!(!valid_elf_stack_copy_target_cr3_event(&malformed_terminal));

        let mut slot_overflow = captured.clone();
        slot_overflow["target_event"]["chain_status"] = json!("malformed");
        slot_overflow["target_event"]["error"] = json!("frame slot address overflow");
        slot_overflow["target_event"]["rbp"] = json!("0xfffffffffffffff8");
        assert!(valid_elf_stack_copy_target_cr3_event(&slot_overflow));

        let mut malformed_after_valid_prefix = captured.clone();
        malformed_after_valid_prefix["target_event"]["chain_status"] = json!("malformed");
        malformed_after_valid_prefix["target_event"]["error"] = json!("next frame slot overflow");
        malformed_after_valid_prefix["target_event"]["rbp"] = json!("0x1000");
        malformed_after_valid_prefix["target_event"]["frames"] = json!([{
            "rbp":"0x1000", "saved_rbp_slot":"0x1000",
            "saved_rbp":"0xfffffffffffffff8", "return_address_slot":"0x1008",
            "return_address":"0xffffffff81000000"
        }]);
        assert!(valid_elf_stack_copy_target_cr3_event(
            &malformed_after_valid_prefix
        ));
        malformed_after_valid_prefix["target_event"]["chain_status"] = json!("unreadable");
        assert!(!valid_elf_stack_copy_target_cr3_event(
            &malformed_after_valid_prefix
        ));

        let mut unreadable_initial_slot_overflow = captured.clone();
        unreadable_initial_slot_overflow["target_event"]["chain_status"] = json!("unreadable");
        unreadable_initial_slot_overflow["target_event"]["error"] = json!("forged read failure");
        unreadable_initial_slot_overflow["target_event"]["rbp"] = json!("0xfffffffffffffff8");
        assert!(!valid_elf_stack_copy_target_cr3_event(
            &unreadable_initial_slot_overflow
        ));
        malformed_terminal["target_event"]["frames"][0]["return_address"] =
            json!("0xffffffff81000000");
        malformed_terminal["target_event"]["frames"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "rbp":"0x1020", "saved_rbp_slot":"0x1020", "saved_rbp":"0x1030",
                "return_address_slot":"0x1028", "return_address":"0xffffffff81000020"
            }));
        assert!(!valid_elf_stack_copy_target_cr3_event(&malformed_terminal));
    }

    #[test]
    fn elf_stack_copy_pre_copy_cr3_ledger_requires_complete_order_and_entry_reconciliation() {
        let markers = od_machine::LINUX_ELF_STACK_COPY_BOUNDARIES
            .iter()
            .enumerate()
            .map(|(index, (rip, name))| {
                json!({
                    "name": name,
                    "rip": format!("0x{rip:016x}"),
                    "retired_instructions_before_current": index as u64 + 1,
                })
            })
            .collect::<Vec<_>>();
        let mut snapshot = json!({
            "status": "captured",
            "invocation": {"entry": {
                "cr3": "0x3000",
                "cr3_changes": 2,
                "retired_instructions_through_current": 20
            }}
        });
        let mut ledger = json!({
            "status": "complete",
            "event_count": 2,
            "events_omitted": 0,
            "events": [
                {
                    "sequence": 1,
                    "rip": "0xffffffff81001000",
                    "old_cr3": "0x1000",
                    "new_cr3": "0x2000",
                    "cpl": 0,
                    "retired_instructions_through_current": 10
                },
                {
                    "sequence": 2,
                    "rip": "0xffffffff81002000",
                    "old_cr3": "0x2000",
                    "new_cr3": "0x3000",
                    "cpl": 0,
                    "retired_instructions_through_current": 12
                }
            ],
            "boundary_markers": markers.clone()
        });
        assert!(
            valid_elf_stack_copy_pre_copy_cr3_ledger(&ledger, snapshot.as_object().unwrap()),
            "ledger={ledger:#}; snapshot={snapshot:#}"
        );
        ledger["status"] = json!("run_ended_before_target");
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["status"] = json!("complete");
        ledger["boundary_markers"].as_array_mut().unwrap().pop();
        assert!(valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["boundary_markers"] = json!(markers);
        ledger["boundary_markers"][1]["retired_instructions_before_current"] = json!(0);
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["boundary_markers"] = json!(markers);

        ledger["event_count"] = json!(1);
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["event_count"] = json!(2);
        ledger["events"][1]["sequence"] = json!(3);
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["events"][1]["sequence"] = json!(2);
        ledger["events"][1]["retired_instructions_through_current"] = json!(21);
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["events"][1]["retired_instructions_through_current"] = json!(12);
        ledger["events"][1]["cpl"] = json!(3);
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["events"][1]["cpl"] = json!(0);
        ledger["events"][1]["old_cr3"] = json!("0x9999");
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));

        ledger["events"][1]["old_cr3"] = json!("0x2000");
        ledger["status"] = json!("incomplete_overflow");
        ledger["events_omitted"] = json!(1);
        ledger["events"].as_array_mut().unwrap().pop();
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));

        let cap = od_machine::LINUX_ELF_STACK_COPY_PRE_COPY_CR3_EVENT_CAPACITY;
        let full_prefix = (0..cap)
            .map(|index| {
                let (old_cr3, new_cr3) = if index % 2 == 0 {
                    ("0x1000", "0x2000")
                } else {
                    ("0x2000", "0x1000")
                };
                json!({
                    "sequence": index as u64 + 1,
                    "rip": "0xffffffff81001000",
                    "old_cr3": old_cr3,
                    "new_cr3": new_cr3,
                    "cpl": 0,
                    "retired_instructions_through_current": 10
                })
            })
            .collect::<Vec<_>>();
        ledger["events"] = json!(full_prefix);
        ledger["event_count"] = json!(cap as u64 + 1);
        snapshot["invocation"]["entry"]["cr3_changes"] = json!(cap as u64 + 1);
        assert!(valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
        ledger["events"].as_array_mut().unwrap().pop();
        ledger["events_omitted"] = json!(2);
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));
    }

    #[test]
    fn elf_stack_copy_run_ended_ledger_cannot_hide_omitted_events() {
        let snapshot = json!({"status": "run_ended_before_target"});
        let ledger = json!({
            "status": "run_ended_before_target",
            "event_count": 1,
            "events_omitted": 1,
            "events": [],
            "boundary_markers": []
        });
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &ledger,
            snapshot.as_object().unwrap()
        ));

        let complete_prefix = json!({
            "status": "run_ended_before_target",
            "event_count": 0,
            "events_omitted": 0,
            "events": [],
            "boundary_markers": []
        });
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger(
            &complete_prefix,
            snapshot.as_object().unwrap()
        ));
        let zero_stop = json!({"stop": {"cr3_changes": 0}});
        assert!(valid_elf_stack_copy_pre_copy_cr3_ledger_with_runner(
            &complete_prefix,
            snapshot.as_object().unwrap(),
            Some(&zero_stop)
        ));

        let captured_prefix = json!({
            "status": "run_ended_before_target",
            "event_count": 1,
            "events_omitted": 0,
            "events": [{
                "sequence": 1,
                "rip": "0xffffffff81001000",
                "old_cr3": "0x1000",
                "new_cr3": "0x2000",
                "cpl": 0,
                "retired_instructions_through_current": 10
            }],
            "boundary_markers": []
        });
        let run_stop = json!({"stop": {"cr3_changes": 1}});
        assert!(valid_elf_stack_copy_pre_copy_cr3_ledger_with_runner(
            &captured_prefix,
            snapshot.as_object().unwrap(),
            Some(&run_stop)
        ));
        let mismatched_stop = json!({"stop": {"cr3_changes": 0}});
        assert!(!valid_elf_stack_copy_pre_copy_cr3_ledger_with_runner(
            &captured_prefix,
            snapshot.as_object().unwrap(),
            Some(&mismatched_stop)
        ));
    }

    #[test]
    fn legacy_v1_series_and_run_manifests_remain_readable() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-v1-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let series_path = write_test_series(&temp, "series-v1", "nonceLegacyV10001", "PASS");
        let mut series: Value = serde_json::from_slice(&fs::read(&series_path).unwrap()).unwrap();
        series["schema_version"] = json!(1);

        let reference = series["runs"][0].clone();
        let run_path = series_path
            .parent()
            .unwrap()
            .join(reference["manifest"].as_str().unwrap());
        let mut run: Value = serde_json::from_slice(&fs::read(&run_path).unwrap()).unwrap();
        run["schema_version"] = json!(1);
        run["host"].as_object_mut().unwrap().remove("source_dirty");
        let run_bytes = serde_json::to_vec_pretty(&run).unwrap();
        fs::write(&run_path, &run_bytes).unwrap();
        series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&run_bytes));
        fs::write(&series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();

        let manifest = fresh_manifest(&series_path).unwrap();
        let run = load_run_manifest(&series_path, &manifest.runs[0]).unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(run.schema_version, 1);
        assert_eq!(run.host.source_dirty, None);
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn series_loader_verifies_execution_observations_sidecar_hash_and_size() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-observation-sidecar-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let series_path =
            write_test_series(&temp, "observation-sidecar", "nonceObsSidecar0001", "PASS");
        let series_dir = series_path.parent().unwrap();
        let run_id = "observation-sidecar-run-0001";
        let run_dir = series_dir.join("runs").join(run_id);
        let sidecar_path = run_dir.join("execution-observations.json");
        let sidecar_value = json!({
            "schema_version": 8,
            "rdtsc_sample_rip": "0xffffffff8271b34d",
            "rdtsc_sample_stride": 512,
            "rdtsc_sample_site_hits": 19_000_000,
            "rdtsc_site_samples": [],
            "rdtsc_site_samples_omitted": 0,
            "rdtsc_entry_sample_rip": "0xffffffff8271b31b",
            "rdtsc_entry_sample_hits": 100,
            "rdtsc_entry_samples": [],
            "rdtsc_entry_samples_omitted": 0,
        });
        let mut sidecar_bytes = serde_json::to_vec_pretty(&sidecar_value).unwrap();
        sidecar_bytes.push(b'\n');
        fs::write(&sidecar_path, &sidecar_bytes).unwrap();

        let metadata = json!({
            "path": format!("runs/{run_id}/execution-observations.json"),
            "capture_scope": "CPL0 RDTSC instruction RIP counts and successfully delivered #PF events, with optional selected-site and entry-site samples",
            "write_error": null,
            "selected_rip": "0xffffffff8271b34d",
            "stride": 512,
            "selected_site_hits": 19_000_000,
            "sample_count": 0,
            "samples_omitted": 0,
            "entry_rip": "0xffffffff8271b31b",
            "entry_hits": 100,
            "entry_sample_count": 0,
            "entry_samples_omitted": 0,
            "byte_count": sidecar_bytes.len(),
            "sha256": od_testkit::sha256_hex(&sidecar_bytes),
        });
        let update_metadata = |metadata: Value| {
            let mut series: Value =
                serde_json::from_slice(&fs::read(&series_path).unwrap()).unwrap();
            let manifest_path = series_dir.join(series["runs"][0]["manifest"].as_str().unwrap());
            let mut run: Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
            let raw_path = series_dir.join(run["runner"]["raw_json"].as_str().unwrap());
            let mut raw: Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
            raw["execution_observations_sidecar"] = metadata;
            let raw_bytes = serde_json::to_vec_pretty(&raw).unwrap();
            fs::write(&raw_path, &raw_bytes).unwrap();
            run["runner"]["raw_json_sha256"] = json!(od_testkit::sha256_hex(&raw_bytes));
            let run_bytes = serde_json::to_vec_pretty(&run).unwrap();
            fs::write(&manifest_path, &run_bytes).unwrap();
            series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&run_bytes));
            fs::write(&series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        };
        let assert_rejected = |sidecar_value: Value, mut metadata: Value, expected_detail: &str| {
            let mut bytes = serde_json::to_vec_pretty(&sidecar_value).unwrap();
            bytes.push(b'\n');
            fs::write(&sidecar_path, &bytes).unwrap();
            metadata["byte_count"] = json!(bytes.len());
            metadata["sha256"] = json!(od_testkit::sha256_hex(&bytes));
            update_metadata(metadata);
            let loaded = fresh_manifest(&series_path).unwrap();
            let error = load_run_manifest(&series_path, &loaded.runs[0]).unwrap_err();
            assert!(error.contains(expected_detail), "{error}");
        };
        update_metadata(metadata.clone());

        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        let mut malformed_rip = metadata.clone();
        malformed_rip["selected_rip"] = json!("0x0ffffffff8271b34d");
        update_metadata(malformed_rip);
        let loaded = fresh_manifest(&series_path).unwrap();
        let error = load_run_manifest(&series_path, &loaded.runs[0]).unwrap_err();
        assert!(error.contains("integrity metadata is invalid"), "{error}");
        update_metadata(metadata.clone());

        let mut malformed_summary = metadata.clone();
        malformed_summary["sample_count"] = json!(37_109);
        update_metadata(malformed_summary);
        let loaded = fresh_manifest(&series_path).unwrap();
        let error = load_run_manifest(&series_path, &loaded.runs[0]).unwrap_err();
        assert!(error.contains("summary does not match"), "{error}");
        update_metadata(metadata.clone());

        for (key, malformed) in [
            ("entry_hits", None),
            ("entry_hits", Some(Value::Null)),
            ("entry_hits", Some(json!("100"))),
            ("entry_samples_omitted", None),
            ("entry_samples_omitted", Some(Value::Null)),
            ("entry_samples_omitted", Some(json!("0"))),
            ("entry_sample_count", None),
            ("entry_sample_count", Some(Value::Null)),
            ("entry_sample_count", Some(json!("0"))),
        ] {
            let mut bad_entry_metadata = json!({
                "path": format!("runs/{run_id}/execution-observations.json"),
                "capture_scope": "CPL0 RDTSC observation sidecar",
                "write_error": null,
                "selected_rip": "0xffffffff8271b34d",
                "stride": 512,
                "selected_site_hits": 19_000_000,
                "sample_count": 0,
                "samples_omitted": 0,
                "entry_rip": "0xffffffff8271b31b",
                "entry_hits": 100,
                "entry_sample_count": 0,
                "entry_samples_omitted": 0,
                "byte_count": sidecar_bytes.len(),
                "sha256": od_testkit::sha256_hex(&sidecar_bytes),
            });
            if let Some(value) = malformed {
                bad_entry_metadata[key] = value;
            } else {
                bad_entry_metadata.as_object_mut().unwrap().remove(key);
            }
            assert_rejected(
                sidecar_value.clone(),
                bad_entry_metadata.clone(),
                "entry sample metadata is missing or invalid",
            );
        }

        for (key, malformed) in [
            ("rdtsc_entry_sample_hits", None),
            ("rdtsc_entry_sample_hits", Some(Value::Null)),
            ("rdtsc_entry_sample_hits", Some(json!("100"))),
            ("rdtsc_entry_samples_omitted", None),
            ("rdtsc_entry_samples_omitted", Some(Value::Null)),
            ("rdtsc_entry_samples_omitted", Some(json!("0"))),
            ("rdtsc_entry_samples", None),
            ("rdtsc_entry_samples", Some(Value::Null)),
        ] {
            let mut malformed_sidecar = sidecar_value.clone();
            if let Some(value) = malformed {
                malformed_sidecar[key] = value;
            } else {
                malformed_sidecar.as_object_mut().unwrap().remove(key);
            }
            assert_rejected(
                malformed_sidecar,
                metadata.clone(),
                "entry sample metadata is missing or invalid",
            );
        }

        for version in [
            None,
            Some(Value::Null),
            Some(json!("8")),
            Some(json!(u64::MAX)),
        ] {
            let mut malformed_sidecar = sidecar_value.clone();
            if let Some(value) = version {
                malformed_sidecar["schema_version"] = value;
            } else {
                malformed_sidecar
                    .as_object_mut()
                    .unwrap()
                    .remove("schema_version");
            }
            assert_rejected(
                malformed_sidecar,
                metadata.clone(),
                "schema_version is missing, invalid, or unsupported",
            );
        }

        let window = json!({"first_hit": 512, "last_hit": 1024});
        let mut v9_sidecar = sidecar_value.clone();
        v9_sidecar["schema_version"] = json!(9);
        v9_sidecar["rdtsc_sample_window"] = window.clone();
        v9_sidecar["rdtsc_site_samples"] = json!([{}, {}]);
        let mut v9_metadata = metadata.clone();
        v9_metadata["sample_window"] = window;
        v9_metadata["sample_count"] = json!(2);
        let mut v9_bytes = serde_json::to_vec_pretty(&v9_sidecar).unwrap();
        v9_bytes.push(b'\n');
        fs::write(&sidecar_path, &v9_bytes).unwrap();
        v9_metadata["byte_count"] = json!(v9_bytes.len());
        v9_metadata["sha256"] = json!(od_testkit::sha256_hex(&v9_bytes));
        update_metadata(v9_metadata.clone());
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        let trace_sample = |hit_ordinal: u64| {
            json!({
                "rip": "0xffffffff8271b31b",
                "hit_ordinal": hit_ordinal,
                "initial_now_ns": 0,
                "now_ns": 20,
                "callback_tsc": "0x30",
                "tsc_hz": 2_400_000_000u64,
                "retired_instructions_charged": 10,
                "retired_ns_charged": 10,
                "pm_timer_ns_charged": 2,
                "pit_ns_charged": 3,
                "rdtsc_ns_charged": 1,
                "idle_ns_advanced": 4,
                "quantum_ordinal": 11 + hit_ordinal,
                "irqs_delivered": 1,
                "unaccounted_elapsed_ns": 0,
            })
        };
        let mut v10_sidecar = v9_sidecar.clone();
        v10_sidecar["schema_version"] = json!(10);
        v10_sidecar["rdtsc_entry_clock_trace_samples"] = json!([trace_sample(1), trace_sample(2)]);
        v10_sidecar["rdtsc_entry_clock_trace_missing"] = json!(0);
        let mut v10_metadata = v9_metadata.clone();
        v10_metadata["entry_clock_trace_sample_count"] = json!(2);
        v10_metadata["entry_clock_trace_missing"] = json!(0);
        let mut v10_bytes = serde_json::to_vec_pretty(&v10_sidecar).unwrap();
        v10_bytes.push(b'\n');
        fs::write(&sidecar_path, &v10_bytes).unwrap();
        v10_metadata["byte_count"] = json!(v10_bytes.len());
        v10_metadata["sha256"] = json!(od_testkit::sha256_hex(&v10_bytes));
        update_metadata(v10_metadata.clone());
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        let mut v11_sidecar = v10_sidecar.clone();
        v11_sidecar["schema_version"] = json!(11);
        for sample in v11_sidecar["rdtsc_entry_clock_trace_samples"]
            .as_array_mut()
            .unwrap()
        {
            sample["quantum_instructions_executed"] = json!(3);
            sample["retired_instructions_through_rdtsc"] = json!(13);
        }
        let mut v11_metadata = v10_metadata.clone();
        let mut v11_bytes = serde_json::to_vec_pretty(&v11_sidecar).unwrap();
        v11_bytes.push(b'\n');
        fs::write(&sidecar_path, &v11_bytes).unwrap();
        v11_metadata["byte_count"] = json!(v11_bytes.len());
        v11_metadata["sha256"] = json!(od_testkit::sha256_hex(&v11_bytes));
        update_metadata(v11_metadata.clone());
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        let mut v12_sidecar = v11_sidecar.clone();
        v12_sidecar["schema_version"] = json!(12);
        v12_sidecar["rdtsc_prefix_sentinel_rip"] = json!("0xffffffff8271b31b");
        v12_sidecar["rdtsc_prefix_status"] = json!("sentinel_seen");
        v12_sidecar["rdtsc_prefix_event_count"] = json!(1);
        v12_sidecar["rdtsc_prefix_missing"] = json!(0);
        v12_sidecar["rdtsc_prefix_samples"] = json!([{
            "ordinal": 1,
            "rip": "0xffffffff8271b31b",
            "cpl": 0,
            "cr3": "0x4000",
            "rsp": "0x5000",
            "rbp": "0x6000",
            "page_faults_delivered_through": 4,
            "general_protection_faults_delivered_through": 2,
            "clock": {
                "rip": "0xffffffff8271b31b",
                "hit_ordinal": 1,
                "initial_now_ns": 0,
                "now_ns": 20,
                "callback_tsc": "0x30",
                "tsc_hz": 2_400_000_000u64,
                "retired_instructions_charged": 10,
                "retired_ns_charged": 10,
                "quantum_instructions_executed": 3,
                "retired_instructions_through_rdtsc": 13,
                "pm_timer_ns_charged": 2,
                "pit_ns_charged": 3,
                "rdtsc_ns_charged": 1,
                "idle_ns_advanced": 4,
                "quantum_ordinal": 12,
                "irqs_delivered": 1,
                "unaccounted_elapsed_ns": 0,
            }
        }]);
        let mut v12_metadata = v11_metadata.clone();
        v12_metadata["rdtsc_prefix_sentinel_rip"] = json!("0xffffffff8271b31b");
        v12_metadata["rdtsc_prefix_status"] = json!("sentinel_seen");
        v12_metadata["rdtsc_prefix_sample_count"] = json!(1);
        v12_metadata["rdtsc_prefix_event_count"] = json!(1);
        v12_metadata["rdtsc_prefix_missing"] = json!(0);
        let mut v12_bytes = serde_json::to_vec_pretty(&v12_sidecar).unwrap();
        v12_bytes.push(b'\n');
        fs::write(&sidecar_path, &v12_bytes).unwrap();
        v12_metadata["byte_count"] = json!(v12_bytes.len());
        v12_metadata["sha256"] = json!(od_testkit::sha256_hex(&v12_bytes));
        update_metadata(v12_metadata.clone());
        let loaded = fresh_manifest(&series_path).unwrap();
        let loaded_run = load_run_manifest(&series_path, &loaded.runs[0]);
        assert!(loaded_run.is_ok(), "{loaded_run:?}");
        let mut v13_sidecar = v12_sidecar.clone();
        v13_sidecar["schema_version"] = json!(13);
        v13_sidecar["instruction_path_trace"] = Value::Null;
        let mut v13_metadata = v12_metadata.clone();
        v13_metadata["instruction_path_trace_status"] = Value::Null;
        v13_metadata["instruction_path_trace_record_count"] = Value::Null;
        v13_metadata["instruction_path_trace_omitted_records"] = Value::Null;
        let mut v13_bytes = serde_json::to_vec_pretty(&v13_sidecar).unwrap();
        v13_bytes.push(b'\n');
        fs::write(&sidecar_path, &v13_bytes).unwrap();
        v13_metadata["byte_count"] = json!(v13_bytes.len());
        v13_metadata["sha256"] = json!(od_testkit::sha256_hex(&v13_bytes));
        update_metadata(v13_metadata.clone());
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        let mut captured_trace = v13_sidecar.clone();
        captured_trace["instruction_path_trace"] = json!({
            "requested": true,
            "status": "captured",
            "quantum_ordinal": od_machine::LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            "retired_instructions_charged": od_machine::LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
            "rdtsc_cpl0_hits_before_dispatch": od_machine::LINUX_INSTRUCTION_PATH_TRACE_START_HITS,
            "quantum_instructions_executed": 1,
            "rdtsc_cpl0_hits_after_quantum": od_machine::LINUX_INSTRUCTION_PATH_TRACE_END_HIT,
            "omitted_records": 0,
            "header": {
                "rip": "0x1000",
                "rflags": "0x202",
                "gpr": ["0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0", "0x0"],
                "cr3": "0x3000",
                "cs_selector": "0x0008",
                "frame_chain": [],
                "frame_chain_end": "NullFramePointer"
            },
            "records": [{"rip": "0x1000", "next_rip": "0x1001", "control_flow": null}],
        });
        let mut captured_metadata = v13_metadata.clone();
        captured_metadata["instruction_path_trace_status"] = json!("captured");
        captured_metadata["instruction_path_trace_record_count"] = json!(1);
        captured_metadata["instruction_path_trace_omitted_records"] = json!(0);
        let mut captured_bytes = serde_json::to_vec_pretty(&captured_trace).unwrap();
        captured_bytes.push(b'\n');
        fs::write(&sidecar_path, &captured_bytes).unwrap();
        captured_metadata["byte_count"] = json!(captured_bytes.len());
        captured_metadata["sha256"] = json!(od_testkit::sha256_hex(&captured_bytes));
        update_metadata(captured_metadata.clone());
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        let mut malformed_header = captured_trace.clone();
        malformed_header["instruction_path_trace"]["header"] = json!({"rip": "0x1000"});
        assert_rejected(
            malformed_header,
            captured_metadata.clone(),
            "instruction trace summary or record is invalid",
        );
        let mut malformed_control_flow = captured_trace.clone();
        malformed_control_flow["instruction_path_trace"]["records"][0]["control_flow"] =
            json!({"kind": "branch"});
        assert_rejected(
            malformed_control_flow,
            captured_metadata.clone(),
            "instruction trace summary or record is invalid",
        );

        let mut incomplete_captured_trace = captured_trace.clone();
        incomplete_captured_trace["instruction_path_trace"]["rdtsc_cpl0_hits_after_quantum"] =
            json!(8382);
        assert_rejected(
            incomplete_captured_trace,
            captured_metadata.clone(),
            "complete instruction trace does not satisfy its gate or end marker",
        );

        let mut gate_mismatch_trace = v13_sidecar.clone();
        gate_mismatch_trace["instruction_path_trace"] = json!({
            "requested": true,
            "status": "gate_mismatch",
            "quantum_ordinal": od_machine::LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            "retired_instructions_charged": od_machine::LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE + 1,
            "rdtsc_cpl0_hits_before_dispatch": 8_382,
            "quantum_instructions_executed": null,
            "rdtsc_cpl0_hits_after_quantum": null,
            "omitted_records": 0,
            "header": null,
            "records": [],
        });
        let mut gate_mismatch_metadata = v13_metadata.clone();
        gate_mismatch_metadata["instruction_path_trace_status"] = json!("gate_mismatch");
        gate_mismatch_metadata["instruction_path_trace_record_count"] = json!(0);
        gate_mismatch_metadata["instruction_path_trace_omitted_records"] = json!(0);
        let mut gate_mismatch_bytes = serde_json::to_vec_pretty(&gate_mismatch_trace).unwrap();
        gate_mismatch_bytes.push(b'\n');
        fs::write(&sidecar_path, &gate_mismatch_bytes).unwrap();
        gate_mismatch_metadata["byte_count"] = json!(gate_mismatch_bytes.len());
        gate_mismatch_metadata["sha256"] = json!(od_testkit::sha256_hex(&gate_mismatch_bytes));
        update_metadata(gate_mismatch_metadata);
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());
        let mut empty_prefix_clock = v12_sidecar.clone();
        empty_prefix_clock["rdtsc_prefix_samples"][0]["clock"] = json!({});
        assert_rejected(
            empty_prefix_clock,
            v12_metadata.clone(),
            "RDTSC prefix sample is invalid",
        );
        let mut mismatched_prefix_sentinel = v12_sidecar.clone();
        mismatched_prefix_sentinel["rdtsc_prefix_sentinel_rip"] = Value::Null;
        assert_rejected(
            mismatched_prefix_sentinel,
            v12_metadata.clone(),
            "RDTSC prefix summary is invalid",
        );
        let mut zero_event_sentinel = v12_sidecar.clone();
        zero_event_sentinel["rdtsc_prefix_event_count"] = json!(0);
        let mut zero_event_metadata = v12_metadata.clone();
        zero_event_metadata["rdtsc_prefix_event_count"] = json!(0);
        assert_rejected(
            zero_event_sentinel,
            zero_event_metadata,
            "RDTSC prefix sample is invalid",
        );
        let mut missing_count_prefix = v12_sidecar.clone();
        missing_count_prefix["rdtsc_prefix_missing"] = json!(1);
        let mut missing_count_metadata = v12_metadata.clone();
        missing_count_metadata["rdtsc_prefix_missing"] = json!(1);
        assert_rejected(
            missing_count_prefix,
            missing_count_metadata,
            "RDTSC prefix termination status is inconsistent",
        );
        let mut over_capacity_prefix = v12_sidecar.clone();
        let impossible_count = od_machine::LINUX_RDTSC_PREFIX_CAPACITY as u64 + 1;
        over_capacity_prefix["rdtsc_prefix_event_count"] = json!(impossible_count);
        over_capacity_prefix["rdtsc_prefix_samples"][0]["ordinal"] = json!(impossible_count);
        over_capacity_prefix["rdtsc_prefix_samples"][0]["clock"]["hit_ordinal"] =
            json!(impossible_count);
        let mut over_capacity_metadata = v12_metadata.clone();
        over_capacity_metadata["rdtsc_prefix_event_count"] = json!(impossible_count);
        assert_rejected(
            over_capacity_prefix,
            over_capacity_metadata,
            "RDTSC prefix summary is invalid",
        );
        let mut invalid_instruction_sum = v11_sidecar.clone();
        invalid_instruction_sum["rdtsc_entry_clock_trace_samples"][0]["retired_instructions_through_rdtsc"] =
            json!(12);
        assert_rejected(
            invalid_instruction_sum,
            v11_metadata.clone(),
            "clock trace instruction counts are invalid",
        );
        let mut missing_quantum_count = v11_sidecar.clone();
        missing_quantum_count["rdtsc_entry_clock_trace_samples"][0]
            .as_object_mut()
            .unwrap()
            .remove("quantum_instructions_executed");
        assert_rejected(
            missing_quantum_count,
            v11_metadata.clone(),
            "clock trace instruction counts are invalid",
        );
        let mut overflowing_instruction_sum = v11_sidecar.clone();
        overflowing_instruction_sum["rdtsc_entry_clock_trace_samples"][0]["retired_instructions_charged"] =
            json!(u64::MAX);
        overflowing_instruction_sum["rdtsc_entry_clock_trace_samples"][0]["quantum_instructions_executed"] =
            json!(1);
        overflowing_instruction_sum["rdtsc_entry_clock_trace_samples"][0]["retired_instructions_through_rdtsc"] =
            json!(u64::MAX);
        assert_rejected(
            overflowing_instruction_sum,
            v11_metadata.clone(),
            "clock trace instruction counts are invalid",
        );

        for malformed in [None, Some(Value::Null), Some(json!("2")), Some(json!(3))] {
            let mut bad_metadata = v10_metadata.clone();
            if let Some(value) = malformed {
                bad_metadata["entry_clock_trace_sample_count"] = value;
            } else {
                bad_metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("entry_clock_trace_sample_count");
            }
            assert_rejected(
                v10_sidecar.clone(),
                bad_metadata,
                "clock trace summary does not match",
            );
        }
        let mut invalid_trace_sample = v10_sidecar.clone();
        invalid_trace_sample["rdtsc_entry_clock_trace_samples"][0]["tsc_hz"] = json!(0);
        assert_rejected(
            invalid_trace_sample,
            v10_metadata.clone(),
            "clock trace sample is invalid",
        );
        let mut missing_trace = v10_sidecar.clone();
        missing_trace
            .as_object_mut()
            .unwrap()
            .remove("rdtsc_entry_clock_trace_samples");
        assert_rejected(
            missing_trace,
            v10_metadata.clone(),
            "clock trace is missing or invalid",
        );

        for malformed in [
            None,
            Some(Value::Null),
            Some(json!("512:1024")),
            Some(json!({"first_hit": 0, "last_hit": 1024})),
            Some(json!({"first_hit": 1024, "last_hit": 512})),
        ] {
            let mut bad_metadata = v9_metadata.clone();
            if let Some(value) = malformed {
                bad_metadata["sample_window"] = value;
            } else {
                bad_metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("sample_window");
            }
            assert_rejected(v9_sidecar.clone(), bad_metadata, "sample window");
        }
        for malformed in [
            None,
            Some(Value::Null),
            Some(json!("512:1024")),
            Some(json!({"first_hit": 0, "last_hit": 1024})),
            Some(json!({"first_hit": 1024, "last_hit": 512})),
        ] {
            let mut bad_sidecar = v9_sidecar.clone();
            if let Some(value) = malformed {
                bad_sidecar["rdtsc_sample_window"] = value;
            } else {
                bad_sidecar
                    .as_object_mut()
                    .unwrap()
                    .remove("rdtsc_sample_window");
            }
            assert_rejected(bad_sidecar, v9_metadata.clone(), "sample window");
        }

        let mut v9_null_sidecar = sidecar_value.clone();
        v9_null_sidecar["schema_version"] = json!(9);
        v9_null_sidecar["rdtsc_sample_window"] = Value::Null;
        let mut v9_null_metadata = metadata.clone();
        v9_null_metadata["sample_window"] = Value::Null;
        let mut v9_null_bytes = serde_json::to_vec_pretty(&v9_null_sidecar).unwrap();
        v9_null_bytes.push(b'\n');
        fs::write(&sidecar_path, &v9_null_bytes).unwrap();
        v9_null_metadata["byte_count"] = json!(v9_null_bytes.len());
        v9_null_metadata["sha256"] = json!(od_testkit::sha256_hex(&v9_null_bytes));
        update_metadata(v9_null_metadata);
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        let mut legacy_sidecar = sidecar_value.clone();
        legacy_sidecar["schema_version"] = json!(7);
        for key in [
            "rdtsc_entry_sample_rip",
            "rdtsc_entry_sample_hits",
            "rdtsc_entry_samples",
            "rdtsc_entry_samples_omitted",
        ] {
            legacy_sidecar.as_object_mut().unwrap().remove(key);
        }
        let mut legacy_metadata = metadata.clone();
        for key in [
            "entry_rip",
            "entry_hits",
            "entry_sample_count",
            "entry_samples_omitted",
        ] {
            legacy_metadata.as_object_mut().unwrap().remove(key);
        }
        let mut legacy_bytes = serde_json::to_vec_pretty(&legacy_sidecar).unwrap();
        legacy_bytes.push(b'\n');
        fs::write(&sidecar_path, &legacy_bytes).unwrap();
        legacy_metadata["byte_count"] = json!(legacy_bytes.len());
        legacy_metadata["sha256"] = json!(od_testkit::sha256_hex(&legacy_bytes));
        update_metadata(legacy_metadata);
        let loaded = fresh_manifest(&series_path).unwrap();
        assert!(load_run_manifest(&series_path, &loaded.runs[0]).is_ok());

        fs::write(&sidecar_path, &sidecar_bytes).unwrap();
        update_metadata(metadata.clone());
        let loaded = fresh_manifest(&series_path).unwrap();

        fs::write(&sidecar_path, b"tampered sidecar").unwrap();
        let error = load_run_manifest(&series_path, &loaded.runs[0]).unwrap_err();
        assert!(
            error.contains("sidecar byte count or SHA-256 mismatch"),
            "{error}"
        );

        fs::write(&sidecar_path, &sidecar_bytes).unwrap();
        fs::remove_file(&sidecar_path).unwrap();
        let error = load_run_manifest(&series_path, &loaded.runs[0]).unwrap_err();
        assert!(
            error.contains("could not resolve series artifact"),
            "{error}"
        );
        fs::remove_dir_all(temp).unwrap();
    }

    fn sample_mmu_gather_capture() -> od_machine::ElfMmuGatherCapture {
        use od_machine::{
            ElfMmuGatherCapture, ElfMmuGatherClosure, ElfMmuGatherContext, ElfMmuGatherInvocation,
            ElfMmuGatherPageWalk, ElfMmuGatherPhase, ElfMmuGatherRaw, ElfMmuGatherSetupData,
            GuestPhysicalAddress, PageTableLevel, WalkEntry,
        };
        let setup_rbp = 0xffff_ffff_8000_1000;
        let relocate_rbp = 0xffff_ffff_8000_2000;
        let relocate_rsp = 0xffff_ffff_8000_0ff0;
        let setup_context = ElfMmuGatherContext {
            rip: od_machine::LINUX_SETUP_ARG_PAGES_STACK_SHIFT_RIP,
            retired_instructions: 80,
            cr3: 0x1000,
            cr3_changes: 0,
            page_faults: 0,
            rbp: setup_rbp,
            rsp: 0xffff_ffff_8000_1100,
            tlb_pointer: None,
            saved_rbp: Some(0xffff_ffff_8000_3000),
            saved_return_rip: Some(0xffff_ffff_8180_0000),
        };
        let relocate_context = ElfMmuGatherContext {
            rip: od_machine::LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_CALL_RIP,
            retired_instructions: 85,
            cr3: 0x1000,
            cr3_changes: 0,
            page_faults: 1,
            rbp: relocate_rbp,
            rsp: relocate_rsp,
            tlb_pointer: Some(0xffff_ffff_8000_4000),
            saved_rbp: Some(setup_rbp),
            saved_return_rip: Some(0xffff_ffff_818f_24e5),
        };
        let gather_context = ElfMmuGatherContext {
            rip: od_machine::LINUX_TLB_FINISH_MMU_ACTUAL_POST_RIP,
            retired_instructions: 90,
            cr3: 0x1000,
            cr3_changes: 0,
            page_faults: 1,
            rbp: 0xffff_ffff_8000_3000,
            rsp: relocate_rsp - 16,
            tlb_pointer: Some(0xffff_ffff_8000_4000),
            saved_rbp: Some(relocate_rbp),
            saved_return_rip: Some(od_machine::LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_RETURN_RIP),
        };
        let return_context = ElfMmuGatherContext {
            rip: od_machine::LINUX_RELOCATE_VMA_DOWN_TLB_FINISH_RETURN_RIP,
            retired_instructions: 95,
            cr3: 0x1000,
            cr3_changes: 0,
            page_faults: 2,
            rbp: relocate_rbp,
            rsp: relocate_rsp,
            tlb_pointer: None,
            saved_rbp: Some(setup_rbp),
            saved_return_rip: Some(0xffff_ffff_818f_24e5),
        };
        let initial = ElfMmuGatherRaw {
            tlb: 0xffff_ffff_8000_4000,
            mm: 0xffff_ffff_8000_5000,
            start: 0x4000,
            end: 0x8000,
            flags: 0,
            mm_tlb_flush_nested: 0,
        };
        let actual_post = ElfMmuGatherRaw {
            flags: 0x800,
            ..initial
        };
        let page_walk = |address: u64, physical: u64| ElfMmuGatherPageWalk {
            address: Some(address),
            access: "Write",
            cpl: 0,
            origin: "Explicit",
            profile: "long64_4k_2m",
            cr0_wp: true,
            efer_nxe: true,
            status: "translated",
            entries: [
                (
                    PageTableLevel::Pml4,
                    39,
                    0x1000 + ((address >> 39) & 0x1ff) * 8,
                    0x2007,
                ),
                (
                    PageTableLevel::Pdpt,
                    30,
                    0x2000 + ((address >> 30) & 0x1ff) * 8,
                    0x3007,
                ),
                (
                    PageTableLevel::Pd,
                    21,
                    0x3000 + ((address >> 21) & 0x1ff) * 8,
                    0x4007,
                ),
                (
                    PageTableLevel::Pt,
                    12,
                    0x4000 + ((address >> 12) & 0x1ff) * 8,
                    physical | 7,
                ),
            ]
            .into_iter()
            .map(|(level, shift, entry_address, entry_value)| WalkEntry {
                level,
                index: ((address >> shift) & 0x1ff) as u16,
                entry_address: GuestPhysicalAddress::new(entry_address),
                entry_value,
            })
            .collect(),
            terminal_level: Some(PageTableLevel::Pt),
            leaf_size: Some(4096),
            translated_physical: Some(physical),
            error: None,
        };
        let candidate_pages = [0xffff_ffff_83fe_f000, 0xffff_ffff_83ff_0000];
        let walks = [
            page_walk(candidate_pages[0], 0x10_000),
            page_walk(candidate_pages[1], 0x11_000),
        ];
        ElfMmuGatherCapture {
            setup_count: 1,
            setups_omitted: 0,
            setups: vec![ElfMmuGatherInvocation {
                sequence: 1,
                closure: ElfMmuGatherClosure::Returned,
                association_mismatch: false,
                setup: ElfMmuGatherPhase::Captured {
                    context: setup_context,
                    value: ElfMmuGatherSetupData {
                        stack_shift: 0x1000,
                        vma: 0xffff_ffff_8000_6000,
                        vm_start: Some(0xffff_ffff_83ff_1000),
                        vm_end: Some(0xffff_ffff_83ff_2000),
                        relocated_start: Some(0xffff_ffff_83ff_0000),
                        aligned_start: Some(0xffff_ffff_83ff_0000),
                        candidate_pages: candidate_pages.map(Some),
                        candidate_error: None,
                    },
                },
                setup_page_walks: walks.clone(),
                initial: ElfMmuGatherPhase::Captured {
                    context: relocate_context,
                    value: initial,
                },
                initial_page_walks: Some(walks.clone()),
                actual_post: ElfMmuGatherPhase::Captured {
                    context: gather_context,
                    value: actual_post,
                },
                returned: ElfMmuGatherPhase::Captured {
                    context: return_context,
                    value: (),
                },
                return_page_walks: Some(walks),
            }],
            unpaired: Vec::new(),
            unpaired_omitted: 0,
            copy_destination: Some(0xffff_ffff_83fe_fff9),
            destination_in_candidate_pages: Some(true),
        }
    }

    fn captured_elf_stack_copy_observations() -> od_machine::LinuxExecutionObservations {
        use od_machine::{
            ElfStackAlignmentCapture, ElfStackAlignmentPoint, ElfStackCopyBoundaryMarker,
            ElfStackCopyCounterSnapshot, ElfStackCopyCr3Event, ElfStackCopyCr3LedgerStatus,
            ElfStackCopyInvocation, ElfStackCopyInvocationStatus, ElfStackCopyPageFaultEvent,
            ElfStackCopyPreCopyCr3Ledger, ElfStackCopyProbe, ElfStackCopyRdtscSiteCount,
            ElfStackCopySnapshot, ElfStackCopyStatus, GuestPhysicalAddress, PageTableLevel,
            WalkEntry,
        };
        let r13 = 0xffff_ffff_83ff_0000;
        let r15 = r13 + 0x40;
        let rdi = r13 - 7;
        let rsi = 0xffff_ffff_82fd_eb8f;
        let bprm_p = r13 + 0x100;
        let rbp = 0xffff_ffff_8000_7000;
        let rsp = 0xffff_ffff_8000_6000;
        let entry = WalkEntry {
            level: PageTableLevel::Pml4,
            index: 511,
            entry_address: GuestPhysicalAddress::new(0x1000),
            entry_value: 0x2007,
        };
        let probe = |name: &str, address: u64, access: &str, outcome: &str| ElfStackCopyProbe {
            name: name.into(),
            address,
            page_base: address & !0xfff,
            access: access.into(),
            cpl: 0,
            entries: vec![entry],
            outcome: outcome.into(),
            translated_physical: (outcome == "translated").then_some(0x4000),
            error: (outcome == "page_fault").then(|| "record=PageFault".into()),
        };
        let snapshot = ElfStackCopySnapshot {
            profile: od_machine::LINUX_ELF_STACK_COPY_PROFILE,
            status: ElfStackCopyStatus::Captured,
            rip: Some(od_machine::LINUX_ELF_STACK_COPY_RIP),
            cr3: Some(0x1000),
            cs_selector: Some(0x8),
            cpl: Some(0),
            r15: Some(r15),
            rbp: Some(rbp),
            rsp: Some(rsp),
            r13: Some(r13),
            rdi: Some(rdi),
            rsi: Some(rsi),
            rdx: Some(7),
            bprm_p_address: Some(r15 + 0x20),
            bprm_p: Some(bprm_p),
            aligned_stack_pointer: Some(r13),
            copy_destination: Some(rdi),
            copy_length: Some(7),
            instruction_bytes: Some(vec![0xe8, 0x47, 0xd9, 0x34, 0x00]),
            mismatch_reasons: Vec::new(),
            probes: vec![
                probe("stack_input", bprm_p, "Read", "translated"),
                probe("copy_source", rsi, "Read", "translated"),
                probe("copy_destination", rdi, "Write", "page_fault"),
            ],
            error: None,
            return_rip: Some(od_machine::LINUX_ELF_STACK_COPY_RIP + 5),
            platform_string_matches: Some(true),
            alignment: ElfStackAlignmentCapture {
                status: "captured",
                before_call: Some(ElfStackAlignmentPoint {
                    rip: od_machine::LINUX_ELF_STACK_ALIGN_BEFORE_RIP,
                    retired_instructions_before_current: 80,
                    cr3: 0x1000,
                    cs_selector: Some(8),
                    cpl: Some(0),
                    r15,
                    rbp,
                    rsp,
                    pointer: bprm_p,
                    instruction_bytes: Some(vec![0xe8, 0xf6, 0x9a, 0xa1, 0xff]),
                    error: None,
                }),
                after_return: Some(ElfStackAlignmentPoint {
                    rip: od_machine::LINUX_ELF_STACK_ALIGN_AFTER_RIP,
                    retired_instructions_before_current: 81,
                    cr3: 0x1000,
                    cs_selector: Some(8),
                    cpl: Some(0),
                    r15,
                    rbp,
                    rsp,
                    pointer: r13,
                    instruction_bytes: Some(vec![0xba, 0x07, 0x00, 0x00, 0x00]),
                    error: None,
                }),
                before_omitted: 0,
                after_omitted: 0,
                matched_copy: Some(true),
            },
            pre_copy_cr3_ledger: Some(ElfStackCopyPreCopyCr3Ledger {
                status: ElfStackCopyCr3LedgerStatus::Complete,
                event_count: 0,
                events_omitted: 0,
                events: Vec::new(),
                boundary_markers: od_machine::LINUX_ELF_STACK_COPY_BOUNDARIES
                    .iter()
                    .enumerate()
                    .map(|(index, (rip, name))| ElfStackCopyBoundaryMarker {
                        name,
                        rip: *rip,
                        retired_instructions_before_current: [5, 20, 50][index],
                    })
                    .collect(),
                target_event: od_machine::ElfStackCopyTargetCr3Capture::default(),
                mmu_gather: sample_mmu_gather_capture(),
            }),
            invocation: Some(ElfStackCopyInvocation {
                status: ElfStackCopyInvocationStatus::Complete,
                return_rip: Some(od_machine::LINUX_ELF_STACK_COPY_RIP + 5),
                entry: Some(ElfStackCopyCounterSnapshot {
                    cr3: 0x1000,
                    delivered_page_faults: 2,
                    delivered_exceptions: 2,
                    cr3_changes: 0,
                    rdtsc_reads: 10,
                    retired_instructions_through_current: 100,
                }),
                return_boundary: Some(ElfStackCopyCounterSnapshot {
                    cr3: 0x2000,
                    delivered_page_faults: 3,
                    delivered_exceptions: 3,
                    cr3_changes: 1,
                    rdtsc_reads: 11,
                    retired_instructions_through_current: 105,
                }),
                stop: Some(ElfStackCopyCounterSnapshot {
                    cr3: 0x2000,
                    delivered_page_faults: 4,
                    delivered_exceptions: 4,
                    cr3_changes: 1,
                    rdtsc_reads: 12,
                    retired_instructions_through_current: 200,
                }),
                cr3_events: vec![ElfStackCopyCr3Event {
                    rip: 0xffff_ffff_8199_9c00,
                    old_cr3: 0x1000,
                    new_cr3: 0x2000,
                    retired_instructions_through_current: 103,
                }],
                cr3_events_omitted: 0,
                page_fault_events: vec![ElfStackCopyPageFaultEvent {
                    rip: 0xffff_ffff_8199_9c10,
                    cr2: 0x7fff_0000_1000,
                    error_code: 3,
                    access: "Write",
                    cpl: 0,
                    retired_instructions_through_current: 101,
                }],
                page_fault_events_omitted: 0,
                rdtsc_sites: vec![ElfStackCopyRdtscSiteCount {
                    rip: 0xffff_ffff_8199_9c20,
                    reads: 1,
                }],
                rdtsc_site_overflow_reads: 0,
                retired_duration: 5,
                retired_duration_cap: od_machine::LINUX_ELF_STACK_COPY_RETIRED_CAPACITY,
            }),
        };
        od_machine::LinuxExecutionObservations {
            rdtsc_sample_stride: 1,
            elf_stack_copy: Some(snapshot),
            ..Default::default()
        }
    }

    #[test]
    fn schema_v21_captured_elf_stack_copy_serializes_and_loads_with_strict_validation() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-elf-stack-copy-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let series_path = write_test_series(&temp, "elf-stack-copy", "nonceElfStackCopy01", "PASS");
        let series_dir = series_path.parent().unwrap();
        let run_id = "elf-stack-copy-run-0001";
        let sidecar_path = series_dir
            .join("runs")
            .join(run_id)
            .join("execution-observations.json");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&sidecar_path)
            .unwrap();
        let observations = captured_elf_stack_copy_observations();
        let (byte_count, sha256) = super::super::write_execution_observations_sidecar(
            &mut file,
            sidecar_path.to_str().unwrap(),
            &observations,
        )
        .unwrap();
        let base_metadata = super::super::execution_observations_sidecar_metadata(
            &format!("runs/{run_id}/execution-observations.json"),
            &observations,
            byte_count,
            sha256,
        );
        let original_bytes = fs::read(&sidecar_path).unwrap();
        let written: Value = serde_json::from_slice(&original_bytes).unwrap();
        assert_eq!(written["schema_version"], 21);
        assert_eq!(written["elf_stack_copy"]["status"], "captured");
        assert_eq!(
            written["elf_stack_copy"]["registers"]["r13"],
            "0xffffffff83ff0000"
        );
        assert_eq!(
            written["elf_stack_copy"]["probes"][2]["outcome"],
            "page_fault"
        );
        assert_eq!(
            written["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["actual_post"]
                ["value"]["flags"],
            "0x00000800"
        );
        assert_eq!(
            written["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["needs_flush"],
            true
        );
        assert!(
            written["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]
                .get("actual_post_page_walks")
                .is_none()
        );

        let update_metadata = |mut metadata: Value, bytes: &[u8]| {
            metadata["byte_count"] = json!(bytes.len());
            metadata["sha256"] = json!(od_testkit::sha256_hex(bytes));
            let mut series: Value =
                serde_json::from_slice(&fs::read(&series_path).unwrap()).unwrap();
            let manifest_path = series_dir.join(series["runs"][0]["manifest"].as_str().unwrap());
            let mut manifest: Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
            let raw_path = series_dir.join(manifest["runner"]["raw_json"].as_str().unwrap());
            let mut raw: Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
            raw["stop"]["retired"] = json!(200);
            raw["stop"]["cr3"] = json!("0x2000");
            raw["stop"]["cr3_changes"] = json!(1);
            raw["stop"]["exceptions_delivered"] = json!(4);
            raw["stop"]["page_faults_delivered"] = json!(4);
            raw["stop"]["rdtsc_reads_by_cpl"] = json!([12, 0, 0, 0]);
            raw["execution_observations_sidecar"] = metadata;
            let raw_bytes = serde_json::to_vec_pretty(&raw).unwrap();
            fs::write(&raw_path, &raw_bytes).unwrap();
            manifest["runner"]["stop"] = raw["stop"].clone();
            manifest["runner"]["raw_json_sha256"] = json!(od_testkit::sha256_hex(&raw_bytes));
            let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
            fs::write(&manifest_path, &manifest_bytes).unwrap();
            series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&manifest_bytes));
            fs::write(&series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        };
        let assert_rejected = |value: Value, expected: &str| {
            let bytes = serde_json::to_vec_pretty(&value).unwrap();
            fs::write(&sidecar_path, &bytes).unwrap();
            update_metadata(base_metadata.clone(), &bytes);
            let loaded = fresh_manifest(&series_path).unwrap();
            let error = load_run_manifest(&series_path, &loaded.runs[0]).unwrap_err();
            assert!(error.contains(expected), "{error}; sidecar={value}");
        };

        update_metadata(base_metadata.clone(), &original_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut legacy_v20 = written.clone();
        legacy_v20["schema_version"] = json!(20);
        legacy_v20["elf_stack_copy"]
            .as_object_mut()
            .unwrap()
            .remove("alignment");
        let legacy_registers = legacy_v20["elf_stack_copy"]["registers"]
            .as_object_mut()
            .unwrap();
        legacy_registers.remove("rbp");
        legacy_registers.remove("rsp");
        let legacy_v20_bytes = serde_json::to_vec_pretty(&legacy_v20).unwrap();
        fs::write(&sidecar_path, &legacy_v20_bytes).unwrap();
        update_metadata(base_metadata.clone(), &legacy_v20_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut incomplete_alignment = written.clone();
        incomplete_alignment["elf_stack_copy"]["alignment"]["status"] = json!("incomplete");
        incomplete_alignment["elf_stack_copy"]["alignment"]["before_call"] = Value::Null;
        incomplete_alignment["elf_stack_copy"]["alignment"]["after_return"] = Value::Null;
        incomplete_alignment["elf_stack_copy"]["alignment"]["matched_copy"] = Value::Null;
        let incomplete_bytes = serde_json::to_vec_pretty(&incomplete_alignment).unwrap();
        fs::write(&sidecar_path, &incomplete_bytes).unwrap();
        update_metadata(base_metadata.clone(), &incomplete_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut wrong_alignment_copy = written.clone();
        wrong_alignment_copy["elf_stack_copy"]["alignment"]["after_return"]["pointer"] =
            json!("0xffffffff83ff0008");
        assert_rejected(
            wrong_alignment_copy,
            "execution-observations ELF stack copy snapshot is invalid",
        );

        let mut late_alignment = written.clone();
        late_alignment["elf_stack_copy"]["alignment"]["after_return"]["retired_instructions_before_current"] =
            json!(100);
        assert_rejected(
            late_alignment,
            "execution-observations ELF stack copy snapshot is invalid",
        );

        let mut wrong_selector = written.clone();
        wrong_selector["elf_stack_copy"]["alignment"]["before_call"]["cs_selector"] =
            json!("0x0013");
        assert_rejected(
            wrong_selector,
            "execution-observations ELF stack copy snapshot is invalid",
        );

        let mut noncanonical_frame = written.clone();
        noncanonical_frame["elf_stack_copy"]["registers"]["rbp"] = json!("0x0ffffc900000fd10");
        assert_rejected(
            noncanonical_frame,
            "execution-observations ELF stack copy snapshot is invalid",
        );

        let mut noncanonical_mismatch_frame = written.clone();
        noncanonical_mismatch_frame["elf_stack_copy"]["registers"]["rbp"] =
            json!("0x0ffffc900000fd10");
        noncanonical_mismatch_frame["elf_stack_copy"]["alignment"]["status"] =
            json!("invocation_mismatch");
        noncanonical_mismatch_frame["elf_stack_copy"]["alignment"]["matched_copy"] = json!(false);
        assert_rejected(
            noncanonical_mismatch_frame,
            "execution-observations ELF stack copy snapshot is invalid",
        );

        let mut incoherent_selector_mismatch = written.clone();
        incoherent_selector_mismatch["elf_stack_copy"]["alignment"]["before_call"]["cs_selector"] =
            json!("0x0013");
        incoherent_selector_mismatch["elf_stack_copy"]["alignment"]["status"] =
            json!("invocation_mismatch");
        incoherent_selector_mismatch["elf_stack_copy"]["alignment"]["matched_copy"] = json!(false);
        assert_rejected(
            incoherent_selector_mismatch,
            "execution-observations ELF stack copy snapshot is invalid",
        );

        let mut false_no_target = written.clone();
        false_no_target["elf_stack_copy"]["alignment"]["status"] = json!("copy_target_not_seen");
        false_no_target["elf_stack_copy"]["alignment"]["matched_copy"] = Value::Null;
        assert_rejected(
            false_no_target,
            "execution-observations ELF stack copy snapshot is invalid",
        );

        let mut terminal_outcomes = written.clone();
        let gather = &mut terminal_outcomes["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"];
        let setup = &mut gather["setups"][0];
        for checkpoint in [
            "setup_page_walks",
            "initial_page_walks",
            "return_page_walks",
        ] {
            let walks = &mut setup[checkpoint];
            let absent = &mut walks[0];
            absent["status"] = json!("non_present");
            absent["entries"][3]["entry_value"] = json!("0x0");
            absent["leaf_size"] = Value::Null;
            absent["translated_physical"] = Value::Null;
            let protection = &mut walks[1];
            protection["status"] = json!("protection");
            protection["entries"][3]["entry_value"] = json!("0x10005");
            protection["translated_physical"] = Value::Null;
        }
        let terminal_outcomes_bytes = serde_json::to_vec_pretty(&terminal_outcomes).unwrap();
        fs::write(&sidecar_path, &terminal_outcomes_bytes).unwrap();
        update_metadata(base_metadata.clone(), &terminal_outcomes_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();
        let mut two_megabyte_leaf = written.clone();
        let gather = &mut two_megabyte_leaf["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"];
        for setup in gather["setups"].as_array_mut().unwrap() {
            for checkpoint in [
                "setup_page_walks",
                "initial_page_walks",
                "return_page_walks",
            ] {
                for walk in setup[checkpoint].as_array_mut().unwrap() {
                    let address = u64::from_str_radix(
                        walk["address"].as_str().unwrap().trim_start_matches("0x"),
                        16,
                    )
                    .unwrap();
                    let physical = 0x20_0000 | (address & 0x1f_ffff);
                    walk["entries"][2]["entry_value"] = json!("0x200087");
                    walk["entries"].as_array_mut().unwrap().truncate(3);
                    walk["terminal_level"] = json!("Pd");
                    walk["leaf_size"] = json!(2_097_152);
                    walk["translated_physical"] = json!(format!("0x{physical:x}"));
                }
            }
        }
        let two_megabyte_bytes = serde_json::to_vec_pretty(&two_megabyte_leaf).unwrap();
        fs::write(&sidecar_path, &two_megabyte_bytes).unwrap();
        update_metadata(base_metadata.clone(), &two_megabyte_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();
        let mut malformed_walk = written.clone();
        malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["copy_destination"] =
            json!("0xffffffff83ff0001");
        malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["destination_in_candidate_pages"] =
            json!(true);
        assert_rejected(malformed_walk, "ELF stack copy snapshot is invalid");
        for (field, invalid) in [
            ("terminal_level", Value::Null),
            ("terminal_level", json!(3)),
            ("leaf_size", json!("4096")),
            ("translated_physical", json!(0x10000)),
            ("translated_physical", Value::Null),
        ] {
            let mut malformed_walk = written.clone();
            malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup_page_walks"]
                [0][field] = invalid;
            assert_rejected(malformed_walk, "ELF stack copy snapshot is invalid");
        }
        let mut mismatched_profile = written.clone();
        mismatched_profile["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup_page_walks"]
            [1]["efer_nxe"] = json!(false);
        assert_rejected(mismatched_profile, "ELF stack copy snapshot is invalid");
        for (path, invalid) in [
            (vec!["setup", "context", "cr3"], json!("0x0000000000002000")),
            (
                vec!["setup_page_walks", "0", "entries", "0", "entry_value"],
                json!("0x0"),
            ),
            (
                vec!["setup_page_walks", "0", "entries", "0", "index"],
                json!(510),
            ),
            (
                vec!["setup_page_walks", "0", "entries", "0", "entry_address"],
                json!("0x0000000000001000"),
            ),
            (
                vec!["setup_page_walks", "0", "entries", "0", "entry_value"],
                json!("0x4007"),
            ),
            (
                vec!["setup_page_walks", "0", "entries", "2", "level"],
                json!("Pt"),
            ),
            (vec!["setup_page_walks", "0", "terminal_level"], json!("Pd")),
            (vec!["setup_page_walks", "0", "leaf_size"], json!(2048)),
            (
                vec!["setup_page_walks", "0", "translated_physical"],
                json!("0x0000000000010001"),
            ),
        ] {
            let mut malformed_walk = written.clone();
            let mut current = &mut malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]
                ["setups"][0];
            for component in &path[..path.len() - 1] {
                current = if let Ok(index) = component.parse::<usize>() {
                    &mut current[index]
                } else {
                    &mut current[*component]
                };
            }
            current[path.last().unwrap()] = invalid;
            assert_rejected(malformed_walk, "ELF stack copy snapshot is invalid");
        }
        let invalid_address_walk = json!({
            "address": null,
            "access": "Write",
            "cpl": 0,
            "origin": "Explicit",
            "profile": "long64_4k_2m",
            "cr0_wp": true,
            "efer_nxe": true,
            "status": "invalid_address",
            "entries": [],
            "terminal_level": null,
            "leaf_size": null,
            "translated_physical": null,
            "error": "candidate derivation failed",
        });
        assert!(valid_mmu_gather_page_walk(
            &invalid_address_walk,
            None,
            Some(0x1000)
        ));
        for (field, invalid) in [
            ("terminal_level", json!(3)),
            ("leaf_size", json!("null")),
            ("translated_physical", json!(0)),
        ] {
            let mut malformed = invalid_address_walk.clone();
            malformed[field] = invalid;
            assert!(!valid_mmu_gather_page_walk(&malformed, None, Some(0x1000)));
        }
        let mut bad_protection = terminal_outcomes.clone();
        bad_protection["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup_page_walks"]
            [1]["entries"][3]["entry_value"] = json!("0x10007");
        assert_rejected(bad_protection, "ELF stack copy snapshot is invalid");
        let mut bad_absence = terminal_outcomes.clone();
        bad_absence["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup_page_walks"]
            [0]["entries"][3]["entry_value"] = json!("0x10007");
        assert_rejected(bad_absence, "ELF stack copy snapshot is invalid");
        let mut bad_read_error = written.clone();
        let walk = &mut bad_read_error["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"]
            [0]["setup_page_walks"][0];
        walk["status"] = json!("read_error");
        walk["entries"][3]["entry_value"] = json!("0x0");
        walk["leaf_size"] = Value::Null;
        walk["translated_physical"] = Value::Null;
        walk["error"] = json!("table read failed");
        assert_rejected(bad_read_error, "ELF stack copy snapshot is invalid");
        let mut read_error_prefix = written.clone();
        let gather = &mut read_error_prefix["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"];
        gather["status"] = json!("incomplete");
        for setup in gather["setups"].as_array_mut().unwrap() {
            for checkpoint in [
                "setup_page_walks",
                "initial_page_walks",
                "return_page_walks",
            ] {
                for walk in setup[checkpoint].as_array_mut().unwrap() {
                    walk["status"] = json!("read_error");
                    walk["entries"].as_array_mut().unwrap().truncate(3);
                    walk["terminal_level"] = json!("Pt");
                    walk["leaf_size"] = Value::Null;
                    walk["translated_physical"] = Value::Null;
                    walk["error"] = json!("page table read failed");
                }
            }
        }
        let read_error_bytes = serde_json::to_vec_pretty(&read_error_prefix).unwrap();
        fs::write(&sidecar_path, &read_error_bytes).unwrap();
        update_metadata(base_metadata.clone(), &read_error_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut malformed_walk = written.clone();
        malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup"]
            ["value"]["candidate_pages"][0] = json!("0xffffffff83fed000");
        assert_rejected(malformed_walk, "ELF stack copy snapshot is invalid");
        let mut malformed_walk = written.clone();
        malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup_page_walks"]
            [0]["entries"][0]["entry_value"] = json!("not-hex");
        assert_rejected(malformed_walk, "ELF stack copy snapshot is invalid");
        let mut malformed_walk = written.clone();
        malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["initial_page_walks"]
            [0]["translated_physical"] = Value::Null;
        assert_rejected(malformed_walk, "ELF stack copy snapshot is invalid");
        let mut malformed_walk = written.clone();
        malformed_walk["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["destination_in_candidate_pages"] =
            json!(false);
        assert_rejected(malformed_walk, "ELF stack copy snapshot is invalid");

        let mut legacy_v19 = written.clone();
        legacy_v19["schema_version"] = json!(19);
        let legacy_gather = &mut legacy_v19["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"];
        legacy_gather
            .as_object_mut()
            .unwrap()
            .remove("copy_destination");
        legacy_gather
            .as_object_mut()
            .unwrap()
            .remove("destination_in_candidate_pages");
        for setup in legacy_gather["setups"].as_array_mut().unwrap() {
            for field in [
                "setup_page_walks",
                "initial_page_walks",
                "return_page_walks",
            ] {
                setup.as_object_mut().unwrap().remove(field);
            }
            for phase_name in ["setup"] {
                let phase = &mut setup[phase_name];
                let data = if phase["status"] == "captured" {
                    &mut phase["value"]
                } else {
                    &mut phase["partial"]
                };
                if let Some(data) = data.as_object_mut() {
                    for field in [
                        "relocated_start",
                        "aligned_start",
                        "candidate_pages",
                        "candidate_error",
                    ] {
                        data.remove(field);
                    }
                }
            }
        }
        for event in legacy_gather["unpaired"].as_array_mut().unwrap() {
            event.as_object_mut().unwrap().remove("page_walks");
        }
        let legacy_v19_bytes = serde_json::to_vec_pretty(&legacy_v19).unwrap();
        fs::write(&sidecar_path, &legacy_v19_bytes).unwrap();
        update_metadata(base_metadata.clone(), &legacy_v19_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut mm_mismatch = written.clone();
        let gather = &mut mm_mismatch["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"];
        gather["status"] = json!("incomplete");
        gather["setups"][0]["association_mismatch"] = json!(true);
        gather["setups"][0]["actual_post"]["value"]["mm"] = json!("0xffffffff80005001");
        assert_eq!(
            gather["setups"][0]["initial"]["value"]["mm"],
            "0xffffffff80005000"
        );
        assert_eq!(
            gather["setups"][0]["actual_post"]["value"]["mm"],
            "0xffffffff80005001"
        );
        let mm_mismatch_bytes = serde_json::to_vec_pretty(&mm_mismatch).unwrap();
        fs::write(&sidecar_path, &mm_mismatch_bytes).unwrap();
        update_metadata(base_metadata.clone(), &mm_mismatch_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();
        let mut unmarked_mm_mismatch = mm_mismatch.clone();
        unmarked_mm_mismatch["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]
            ["association_mismatch"] = json!(false);
        assert_rejected(unmarked_mm_mismatch, "ELF stack copy snapshot is invalid");

        let runner_bounds = json!({"stop": {
            "retired": 200,
            "cr3_changes": 1,
            "page_faults_delivered": 4,
        }});
        let ledger = written["elf_stack_copy"]["pre_copy_cr3_ledger"].clone();
        let copy_invocation = written["elf_stack_copy"]["invocation"].clone();
        for (counter, value) in [
            ("retired_instructions", 101),
            ("cr3_changes", 1),
            ("page_faults", 3),
        ] {
            let mut bounded = ledger.clone();
            for phase in ["setup", "initial", "actual_post", "returned"] {
                bounded["mmu_gather"]["setups"][0][phase]["context"][counter] = json!(value);
            }
            if counter == "cr3_changes" {
                bounded["event_count"] = json!(1);
            }
            assert!(
                !valid_elf_mmu_gather_capture(
                    &bounded,
                    &runner_bounds,
                    Some(&copy_invocation),
                    20,
                    Some(&written["elf_stack_copy"]["copy_destination"]),
                ),
                "copy-entry {counter} bound must reject while runner stop permits it"
            );
        }
        let mut read_error_bound = ledger.clone();
        let read_error = &mut read_error_bound["mmu_gather"]["setups"][0]["initial"];
        read_error["status"] = json!("read_error");
        read_error["partial"] = Value::Null;
        read_error["error"] = json!("read failed");
        read_error.as_object_mut().unwrap().remove("value");
        for phase in ["setup", "initial", "actual_post", "returned"] {
            read_error_bound["mmu_gather"]["setups"][0][phase]["context"]["page_faults"] = json!(3);
        }
        read_error_bound["mmu_gather"]["status"] = json!("incomplete");
        assert!(!valid_elf_mmu_gather_capture(
            &read_error_bound,
            &runner_bounds,
            Some(&copy_invocation),
            20,
            Some(&written["elf_stack_copy"]["copy_destination"]),
        ));
        let mut unpaired_bound = ledger.clone();
        unpaired_bound["mmu_gather"]["unpaired"] = json!([{
            "phase": "actual_post",
            "status": "read_error",
            "context": ledger["mmu_gather"]["setups"][0]["actual_post"]["context"].clone(),
            "raw": null,
            "page_walks": null,
            "error": "read failed",
        }]);
        unpaired_bound["mmu_gather"]["unpaired"][0]["context"]["page_faults"] = json!(3);
        unpaired_bound["mmu_gather"]["status"] = json!("incomplete");
        assert!(!valid_elf_mmu_gather_capture(
            &unpaired_bound,
            &runner_bounds,
            Some(&copy_invocation),
            20,
            Some(&written["elf_stack_copy"]["copy_destination"]),
        ));
        let mut ledger_bound = ledger.clone();
        for phase in ["setup", "initial", "actual_post", "returned"] {
            ledger_bound["mmu_gather"]["setups"][0][phase]["context"]["cr3_changes"] = json!(1);
        }
        let mut entry_allows_ledger_bound = copy_invocation.clone();
        entry_allows_ledger_bound["entry"]["cr3_changes"] = json!(1);
        assert!(
            !valid_elf_mmu_gather_capture(
                &ledger_bound,
                &runner_bounds,
                Some(&entry_allows_ledger_bound),
                20,
                Some(&written["elf_stack_copy"]["copy_destination"]),
            ),
            "context CR3 changes must be bounded by the full ledger event count"
        );
        let mut initial_read_error = written.clone();
        let initial_phase = &mut initial_read_error["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]
            ["setups"][0]["initial"];
        initial_phase["status"] = json!("read_error");
        initial_phase["partial"] = Value::Null;
        initial_phase["error"] = json!("initial MMU gather read failed");
        initial_phase.as_object_mut().unwrap().remove("value");
        initial_read_error["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["status"] =
            json!("incomplete");
        assert_eq!(
            initial_read_error["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]
                ["actual_post"]["value"]["flags"],
            "0x00000800"
        );
        let initial_read_error_bytes = serde_json::to_vec_pretty(&initial_read_error).unwrap();
        fs::write(&sidecar_path, &initial_read_error_bytes).unwrap();
        update_metadata(base_metadata.clone(), &initial_read_error_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut legacy_v18 = written.clone();
        legacy_v18["schema_version"] = json!(18);
        legacy_v18["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"] = Value::Null;
        legacy_v18["elf_stack_copy"]["pre_copy_cr3_ledger"]
            .as_object_mut()
            .unwrap()
            .remove("mmu_gather");
        let legacy_v18_bytes = serde_json::to_vec_pretty(&legacy_v18).unwrap();
        fs::write(&sidecar_path, &legacy_v18_bytes).unwrap();
        update_metadata(base_metadata.clone(), &legacy_v18_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut gate_mismatch = written.clone();
        gate_mismatch["elf_stack_copy"]["status"] = json!("gate_mismatch");
        gate_mismatch["elf_stack_copy"]["mismatch_reasons"] =
            json!(["platform_call_gate_incomplete"]);
        gate_mismatch["elf_stack_copy"]["probes"] = json!([]);
        gate_mismatch["elf_stack_copy"]["alignment"]["status"] = json!("invocation_mismatch");
        gate_mismatch["elf_stack_copy"]["alignment"]["matched_copy"] = json!(false);
        gate_mismatch["elf_stack_copy"]
            .as_object_mut()
            .unwrap()
            .remove("invocation");
        let gate_mismatch_bytes = serde_json::to_vec_pretty(&gate_mismatch).unwrap();
        fs::write(&sidecar_path, &gate_mismatch_bytes).unwrap();
        let mut gate_mismatch_metadata = base_metadata.clone();
        gate_mismatch_metadata["elf_stack_copy_status"] = json!("gate_mismatch");
        update_metadata(gate_mismatch_metadata, &gate_mismatch_bytes);
        let loaded = fresh_manifest(&series_path).unwrap();
        load_run_manifest(&series_path, &loaded.runs[0]).unwrap();

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["probes"][0]["entries"][0] = json!({});
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["probes"][0]["address"] = json!("0xffffffff83ff0200");
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["probes"][0]["cpl"] = json!(3);
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["cs_selector"] = Value::Null;
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["actual_post"]
            ["value"]["flags"] = json!("0x00000000");
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["initial"]
            ["context"]["saved_rbp"] = json!("0xffffffff80009999");
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups_omitted"] =
            json!(1);
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup"]["value"]
            ["vm_start"] = Value::Null;
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["setup"]["context"]
            ["saved_return_rip"] = Value::Null;
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["needs_flush"] =
            Value::Null;
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]
            .as_object_mut()
            .unwrap()
            .remove("needs_flush");
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["actual_post"] =
            json!({"status":"not_observed"});
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["returned"] =
            json!({"status":"not_observed"});
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["closure"] =
            json!("run_ended");
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["needs_flush"] =
            json!("unknown");
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["status"] =
            json!("incomplete");
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["actual_post"]
            ["context"]["retired_instructions"] = json!(105);
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["actual_post"]
            ["context"]["cr3_changes"] = json!(2);
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups"][0]["actual_post"]
            ["context"]["page_faults"] = json!(0);
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setup_count"] =
            json!(u64::MAX);
        malformed["elf_stack_copy"]["pre_copy_cr3_ledger"]["mmu_gather"]["setups_omitted"] =
            json!(u64::MAX);
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written.clone();
        malformed["elf_stack_copy"]["registers"]["r15"] = json!("0x0");
        malformed["elf_stack_copy"]["registers"]["r13"] = json!("0x0");
        malformed["elf_stack_copy"]["registers"]["rdi"] = json!("0x0");
        malformed["elf_stack_copy"]["bprm_p_address"] = json!("0x20");
        malformed["elf_stack_copy"]["bprm_p"] = json!("0x100");
        malformed["elf_stack_copy"]["aligned_stack_pointer"] = json!("0x0");
        malformed["elf_stack_copy"]["copy_destination"] = json!("0x0");
        malformed["elf_stack_copy"]["probes"][0]["address"] = json!("0x100");
        malformed["elf_stack_copy"]["probes"][0]["page_base"] = json!("0x0");
        malformed["elf_stack_copy"]["probes"][2]["address"] = json!("0x0");
        malformed["elf_stack_copy"]["probes"][2]["page_base"] = json!("0x0");
        assert_rejected(malformed, "ELF stack copy snapshot is invalid");

        let mut malformed = written;
        malformed["schema_version"] = json!(u64::MAX);
        assert_rejected(
            malformed,
            "schema_version is missing, invalid, or unsupported",
        );
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn elf_stack_copy_invocation_validator_rejects_invalid_boundaries_counters_and_overflow() {
        let snapshot = json!({"return_rip": "0xffffffff81999bd9"});
        let snapshot = snapshot.as_object().unwrap().clone();
        let mut invocation = super::super::elf_stack_copy_invocation_json(
            &captured_elf_stack_copy_observations()
                .elf_stack_copy
                .unwrap()
                .invocation
                .unwrap(),
        );
        let runner = json!({"stop": {
            "retired": 200,
            "cr3": "0x2000",
            "cr3_changes": 1,
            "exceptions_delivered": 4,
            "page_faults_delivered": 4,
            "rdtsc_reads_by_cpl": [12, 0, 0, 0]
        }});
        let original = invocation.clone();
        assert!(
            valid_elf_stack_copy_invocation(&original, &snapshot, &runner),
            "invocation={original:#}; runner={runner:#}; snapshot={snapshot:?}"
        );

        invocation["return_rip"] = json!("0xffffffff81999bd8");
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));

        invocation = original.clone();
        invocation["status"] = json!("incomplete_run_ended");
        invocation["return_boundary"] = Value::Null;
        assert!(valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));
        invocation["return_boundary"] = original["return_boundary"].clone();
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));
        invocation = original.clone();
        invocation["status"] = json!("incomplete_run_ended");
        invocation["return_boundary"] = Value::Null;
        invocation["cr3_events_omitted"] = json!(1);
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));

        invocation = original.clone();
        invocation["status"] = json!("incomplete_overflow");
        invocation["return_boundary"] = Value::Null;
        invocation["cr3_events_omitted"] = json!(1);
        assert!(valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));
        invocation["cr3_events"][0]["old_cr3"] = json!("0x3000");
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));
        invocation = original.clone();
        invocation["status"] = json!("incomplete_overflow");
        invocation["return_boundary"] = Value::Null;
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));

        invocation = original.clone();
        invocation["retired_duration_cap"] = json!(1);
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));

        invocation = original.clone();
        invocation["cr3_events"][0]["retired_instructions_through_current"] = json!(99);
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));

        invocation = original.clone();
        invocation["cr3_events"][0]["old_cr3"] = json!("0x3000");
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));

        let mut ordered_runner = runner.clone();
        invocation = original.clone();
        let mut second_fault = invocation["page_fault_events"][0].clone();
        invocation["page_fault_events"][0]["retired_instructions_through_current"] = json!(102);
        second_fault["retired_instructions_through_current"] = json!(101);
        invocation["page_fault_events"]
            .as_array_mut()
            .unwrap()
            .push(second_fault);
        invocation["return_boundary"]["delivered_page_faults"] = json!(4);
        invocation["return_boundary"]["delivered_exceptions"] = json!(4);
        invocation["stop"]["delivered_page_faults"] = json!(5);
        invocation["stop"]["delivered_exceptions"] = json!(5);
        ordered_runner["stop"]["page_faults_delivered"] = json!(5);
        ordered_runner["stop"]["exceptions_delivered"] = json!(5);
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &ordered_runner
        ));

        for field in [
            "delivered_page_faults",
            "delivered_exceptions",
            "cr3_changes",
            "rdtsc_reads",
            "retired_instructions_through_current",
        ] {
            invocation = original.clone();
            invocation["stop"][field] = json!(0);
            assert!(
                !valid_elf_stack_copy_invocation(&invocation, &snapshot, &runner),
                "{field}"
            );
        }

        invocation = original;
        invocation["rdtsc_sites"] = json!([
            {"rip": "0x1000", "reads": u64::MAX},
            {"rip": "0x2000", "reads": 2}
        ]);
        assert!(!valid_elf_stack_copy_invocation(
            &invocation,
            &snapshot,
            &runner
        ));
    }

    #[test]
    fn legacy_v2_configuration_without_linux_diagnostic_keys_still_verifies_raw_json() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-legacy-linux-config-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let prepare_norandmaps_run = |series_path: &Path, legacy_config: bool| {
            let mut series: Value =
                serde_json::from_slice(&fs::read(series_path).unwrap()).unwrap();
            let run_path = series_path
                .parent()
                .unwrap()
                .join(series["runs"][0]["manifest"].as_str().unwrap());
            let mut run: Value = serde_json::from_slice(&fs::read(&run_path).unwrap()).unwrap();
            let raw_path = series_path
                .parent()
                .unwrap()
                .join(run["runner"]["raw_json"].as_str().unwrap());
            let mut raw: Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
            raw["cmdline"] = json!(format!("{} norandmaps", raw["cmdline"].as_str().unwrap()));
            raw["linux_norandmaps"] = json!(true);
            if !legacy_config {
                raw["linux_no_randomize_kstack_offset"] = json!(false);
            }
            // This raw JSON models a historical run with norandmaps enabled; its manifest uses
            // the old seven-key configuration, while current manifests record both flags.
            let raw_bytes = serde_json::to_vec(&raw).unwrap();
            fs::write(&raw_path, &raw_bytes).unwrap();
            run["runner"]["raw_json_sha256"] = json!(od_testkit::sha256_hex(&raw_bytes));
            run["configuration"]["cmdline"] = raw["cmdline"].clone();
            if legacy_config {
                run["configuration"]
                    .as_object_mut()
                    .unwrap()
                    .remove("linux_norandmaps");
                run["configuration"]
                    .as_object_mut()
                    .unwrap()
                    .remove("linux_no_randomize_kstack_offset");
            } else {
                run["configuration"]["linux_norandmaps"] = json!(true);
                run["configuration"]["linux_no_randomize_kstack_offset"] = json!(false);
            }
            let run_bytes = serde_json::to_vec_pretty(&run).unwrap();
            fs::write(&run_path, &run_bytes).unwrap();
            series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&run_bytes));
            fs::write(series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        };

        let legacy_path =
            write_test_series(&temp, "legacy-linux-config", "nonceLegacyCfg001", "PASS");
        let current_path =
            write_test_series(&temp, "current-linux-config", "nonceCurrentCfg001", "PASS");
        prepare_norandmaps_run(&legacy_path, true);
        prepare_norandmaps_run(&current_path, false);

        let legacy_reference: Value =
            serde_json::from_slice(&fs::read(&legacy_path).unwrap()).unwrap();
        let legacy_manifest_path = legacy_path
            .parent()
            .unwrap()
            .join(legacy_reference["runs"][0]["manifest"].as_str().unwrap());
        let legacy_manifest_value: Value =
            serde_json::from_slice(&fs::read(legacy_manifest_path).unwrap()).unwrap();
        assert_eq!(
            legacy_manifest_value["configuration"]
                .as_object()
                .unwrap()
                .len(),
            7
        );

        let legacy_series = fresh_manifest(&legacy_path).unwrap();
        let legacy = load_run_manifest(&legacy_path, &legacy_series.runs[0]).unwrap();
        let current_series = fresh_manifest(&current_path).unwrap();
        let current = load_run_manifest(&current_path, &current_series.runs[0]).unwrap();
        let loaded = &legacy;
        assert_eq!(loaded.schema_version, SERIES_SCHEMA_VERSION);
        assert_eq!(loaded.configuration["linux_norandmaps"], true);
        assert!(!loaded.configuration["linux_no_randomize_kstack_offset"].is_boolean());

        let mut explained = Vec::new();
        let mut unexplained = Vec::new();
        compare_run(
            0,
            &legacy_path,
            &current_path,
            &legacy,
            &current,
            &mut explained,
            &mut unexplained,
        )
        .unwrap();
        assert!(
            unexplained
                .iter()
                .all(|item| !item.contains("configuration")),
            "{unexplained:?}"
        );

        let mut tampered_configuration = loaded.configuration.clone();
        tampered_configuration["cmdline"] = json!("od.nonce=changed norandmaps");
        let raw_path = legacy_path.parent().unwrap().join(&loaded.runner.raw_json);
        let raw_value: Value = serde_json::from_slice(&fs::read(raw_path).unwrap()).unwrap();
        assert!(!configuration_matches_runner(
            &tampered_configuration,
            &raw_value
        ));
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn crlf_marker_manifests_load_with_legacy_v1_v2_and_canonical_v2_evidence() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-crlf-compat-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        for (series_id, legacy_marker_evidence, schema_version) in [
            ("crlf-legacy-v1", true, 1),
            ("crlf-legacy-v2", true, SERIES_SCHEMA_VERSION),
            ("crlf-canonical-v2", false, SERIES_SCHEMA_VERSION),
        ] {
            let series_path =
                write_crlf_test_series(&temp, series_id, legacy_marker_evidence, schema_version);
            let series = fresh_manifest(&series_path).unwrap();
            let loaded = load_run_manifest(&series_path, &series.runs[0]).unwrap();
            let root = series_path.parent().unwrap();
            let uart_path = root.join(&loaded.uart.path);
            let uart = fs::read(uart_path).unwrap();
            let expected = if legacy_marker_evidence {
                validate_markers_legacy(
                    &uart,
                    &loaded.nonce,
                    &loaded.guest_markers.expected_s5_marker_keys,
                )
            } else {
                validate_markers(
                    &uart,
                    &loaded.nonce,
                    &loaded.guest_markers.expected_s5_marker_keys,
                )
            };
            assert_eq!(loaded.schema_version, schema_version);
            assert_eq!(loaded.guest_markers, expected.evidence);
        }

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn marker_validator_requires_exact_nonce_order_and_no_duplicates() {
        let expected = vec!["A_FILE".to_string(), "A_PASS".to_string()];
        let good = b"kernel: boot\nOD_NONCE=runNonce001\nOD_S5_A_FILE nonce=runNonce001 path=/tmp/a sha256=abc\nOD_S5_A_PASS nonce=runNonce001 result=ok\n";
        let check = validate_markers(good, "runNonce001", &expected);
        assert!(check.evidence.valid, "{:?}", check.evidence.errors);
        assert_eq!(check.evidence.ordered_s5_marker_keys, expected);

        for bad in [
            b"OD_NONCE=runNonce001\nOD_S5_A_PASS nonce=runNonce001 result=ok\n".as_slice(),
            b"OD_NONCE=runNonce001\nOD_S5_A_FILE nonce=wrong path=/tmp/a sha256=abc\nOD_S5_A_PASS nonce=runNonce001 result=ok\n".as_slice(),
            b"OD_NONCE=runNonce001\nOD_S5_A_FILE nonce=runNonce001 path=/tmp/a sha256=abc\nOD_S5_A_FILE nonce=runNonce001 path=/tmp/a sha256=abc\nOD_S5_A_PASS nonce=runNonce001 result=ok\n".as_slice(),
            b"OD_NONCE=wrongNonce001\nOD_S5_A_FILE nonce=runNonce001 path=/tmp/a sha256=abc\nOD_S5_A_PASS nonce=runNonce001 result=ok\n".as_slice(),
            b"OD_NONCE=runNonce001\r\r\nOD_S5_A_FILE nonce=runNonce001 path=/tmp/a sha256=abc\nOD_S5_A_PASS nonce=runNonce001 result=ok\n".as_slice(),
            b"OD_NONCE=runNonce001\nOD_S5_A_PASS nonce=runNonce001 result=ok\nOD_S5_A_FILE nonce=runNonce001 path=/tmp/a sha256=abc\n".as_slice(),
        ] {
            assert!(!validate_markers(bad, "runNonce001", &expected).evidence.valid);
        }
    }

    #[test]
    fn marker_validator_accepts_crlf_nonce_line_without_relaxing_exact_value() {
        let nonce = "runNonce001";
        let crlf = format!("OD_NONCE={nonce}\r\n");
        let check = validate_markers(crlf.as_bytes(), nonce, &[]);
        assert!(check.evidence.valid, "{:?}", check.evidence.errors);
        assert_eq!(
            check.evidence.stage4_nonce_line.as_deref(),
            Some("OD_NONCE=runNonce001")
        );

        for malformed in [
            format!("OD_NONCE={nonce}x\r\n"),
            format!("OD_NONCE={nonce}\r\r\n"),
        ] {
            assert!(
                !validate_markers(malformed.as_bytes(), nonce, &[])
                    .evidence
                    .valid,
                "accepted malformed marker {malformed:?}"
            );
        }
    }

    #[test]
    fn s5_marker_validation_does_not_claim_s5_scenario_pass_before_integration() {
        let nonce = "runNonce001";
        let expected = vec!["A_PASS".to_string()];
        let bytes = format!("OD_NONCE={nonce}\nOD_S5_A_PASS nonce={nonce} result=ok\n");
        let markers = validate_markers(bytes.as_bytes(), nonce, &expected).evidence;
        assert!(markers.valid);
        assert_eq!(
            scenario_status(
                "s5-A",
                "loaded",
                true,
                &markers,
                Some(&json!({"exit_kind":"halted"})),
                "halted"
            ),
            ScenarioStatus::NotRun
        );
    }

    #[test]
    fn s5_08_requires_ordered_exact_markers_and_a_typed_shutdown() {
        let nonce = "runNonce001";
        let expected = [S5_08_SHUTDOWN_MARKER.to_owned()];
        let check = |uart: &str, stop: &Value, kind: &str| {
            let markers =
                validate_scenario_markers(S5_08_SCENARIO, uart.as_bytes(), nonce, &expected, true);
            scenario_status(
                S5_08_SCENARIO,
                "loaded",
                true,
                &markers.evidence,
                Some(stop),
                kind,
            )
        };
        let marker = format!("OD_S5_A_SHUTDOWN_REQUEST nonce={nonce} method=poweroff");
        let good = format!("OD_NONCE={nonce}\r\n{marker}\r\n");
        let shutdown = json!({"exit_kind":"stopped","machine_stop_reason":"shutdown"});
        assert_eq!(check(&good, &shutdown, "stopped"), ScenarioStatus::Passed);
        for uart in [
            format!("OD_NONCE={nonce}\n"),
            format!("{marker}\nOD_NONCE={nonce}\n"),
            format!("OD_NONCE={nonce}\n{marker}"),
            good.replace("nonce=runNonce001", "nonce=wrongNonce"),
            good.replace("method=poweroff", "method=reset"),
            format!("{good}[    3.0] Kernel panic - not syncing: panic despite markers\n"),
        ] {
            assert_eq!(check(&uart, &shutdown, "stopped"), ScenarioStatus::Failed);
        }
        for stop in [
            json!({"exit_kind":"stopped"}),
            json!({"exit_kind":"stopped","machine_stop_reason":"reset"}),
            json!({"exit_kind":"budget_exhausted","machine_stop_reason":null}),
            json!({"exit_kind":"halted","machine_stop_reason":"shutdown"}),
        ] {
            assert_eq!(check(&good, &stop, "stopped"), ScenarioStatus::Failed);
        }
        assert_eq!(check(&good, &shutdown, "halted"), ScenarioStatus::Failed);
    }

    #[test]
    fn stage4_nonce_smoke_requires_exact_marker_and_expected_stop() {
        let nonce = "runNonce001";
        let markers =
            validate_markers(format!("OD_NONCE={nonce}\n").as_bytes(), nonce, &[]).evidence;
        assert!(markers.valid);
        assert_eq!(
            scenario_status(
                "stage4-init-nonce",
                "loaded",
                true,
                &markers,
                Some(&json!({"exit_kind":"halted"})),
                "halted"
            ),
            ScenarioStatus::Passed
        );
        assert_eq!(
            scenario_status(
                "stage4-init-nonce",
                "loaded",
                true,
                &markers,
                Some(&json!({"exit_kind":"budget_exhausted"})),
                "halted"
            ),
            ScenarioStatus::NotRun
        );
    }

    #[test]
    fn series_cli_requires_explicit_smoke_count_and_valid_scenario_markers() {
        let base = vec![
            "--test-runs".into(),
            "2".into(),
            "--series-id".into(),
            "smoke01".into(),
            "--scenario-id".into(),
            "s5-A".into(),
            "--expected-stop-kind".into(),
            "halted".into(),
            "--expected-s5-marker".into(),
            "A_FILE".into(),
            "--expected-s5-marker".into(),
            "A_PASS".into(),
            "--out".into(),
            "work/stage5/smoke01/series.json".into(),
            "--run".into(),
            "--budget".into(),
            "4096".into(),
        ];
        let parsed = parse_series_args(&base).unwrap();
        assert_eq!(parsed.requested_runs, 2);
        assert_eq!(parsed.expected_s5_marker_keys, ["A_FILE", "A_PASS"]);
        assert!(!parsed.capture_execution_observations);
        assert!(
            !parsed
                .forwarded
                .iter()
                .any(|argument| argument == "--linux-norandmaps")
        );
        assert!(parse_series_args(&base[2..]).is_err());
        let mut diagnostic = base.clone();
        diagnostic.extend(["--stack-context-out".into(), "context.json".into()]);
        assert!(parse_series_args(&diagnostic).is_err());
        let mut execution_observations = base.clone();
        execution_observations.push("--execution-observations".into());
        let parsed = parse_series_args(&execution_observations).unwrap();
        assert!(parsed.capture_execution_observations);
        let mut delay_summary = execution_observations.clone();
        delay_summary.push("--execution-observations-delay-call-summary".into());
        let parsed_summary = parse_series_args(&delay_summary).unwrap();
        assert!(parsed_summary.execution_observations_delay_call_summary);
        let mut generated_args = parsed_summary.forwarded.clone();
        append_execution_observations_args(
            &mut generated_args,
            std::path::Path::new("series/run/execution-observations.json"),
            None,
            None,
            None,
            None,
            false,
            false,
            parsed_summary.execution_observations_delay_call_summary,
        );
        assert!(
            generated_args
                .iter()
                .any(|argument| argument == "--execution-observations-delay-call-summary")
        );
        let mut summary_without_capture = base.clone();
        summary_without_capture.push("--execution-observations-delay-call-summary".into());
        assert!(parse_series_args(&summary_without_capture).is_err());
        let mut selected_rdtsc = execution_observations.clone();
        selected_rdtsc.extend([
            "--execution-observations-rdtsc-rip".into(),
            "0xffffffff8271b34d".into(),
        ]);
        let parsed = parse_series_args(&selected_rdtsc).unwrap();
        assert!(parsed.capture_execution_observations);
        assert_eq!(
            parsed.execution_observations_rdtsc_rip,
            Some(0xffff_ffff_8271_b34d)
        );
        let mut selected_rdtsc_stride = selected_rdtsc.clone();
        selected_rdtsc_stride
            .extend(["--execution-observations-rdtsc-stride".into(), "512".into()]);
        let parsed = parse_series_args(&selected_rdtsc_stride).unwrap();
        assert_eq!(parsed.execution_observations_rdtsc_stride, Some(512));
        let mut selected_window = selected_rdtsc.clone();
        selected_window.extend([
            "--execution-observations-rdtsc-window".into(),
            "1150000:1200000".into(),
        ]);
        let parsed = parse_series_args(&selected_window).unwrap();
        assert_eq!(
            parsed.execution_observations_rdtsc_window,
            Some(od_machine::RdtscSampleWindow {
                first_hit: 1_150_000,
                last_hit: 1_200_000,
            })
        );
        let mut invalid_window = selected_rdtsc.clone();
        invalid_window.extend([
            "--execution-observations-rdtsc-window".into(),
            "0:10".into(),
        ]);
        assert!(parse_series_args(&invalid_window).is_err());
        let mut window_without_selected = execution_observations.clone();
        window_without_selected.extend([
            "--execution-observations-rdtsc-window".into(),
            "1:10".into(),
        ]);
        assert!(
            parse_series_args(&window_without_selected)
                .unwrap_err()
                .contains("needs")
        );
        let mut oversized_window = selected_rdtsc.clone();
        oversized_window.extend([
            "--execution-observations-rdtsc-window".into(),
            format!("1:{}", od_machine::LINUX_RDTSC_SAMPLE_CAPACITY + 1),
        ]);
        assert!(
            parse_series_args(&oversized_window)
                .unwrap_err()
                .contains("sample cap")
        );
        let mut selected_entry = selected_rdtsc.clone();
        selected_entry.extend([
            "--execution-observations-rdtsc-entry-rip".into(),
            "0xffffffff8271b31b".into(),
        ]);
        let parsed = parse_series_args(&selected_entry).unwrap();
        assert_eq!(
            parsed.execution_observations_rdtsc_entry_rip,
            Some(0xffff_ffff_8271_b31b)
        );
        let mut instruction_trace = selected_entry.clone();
        instruction_trace.push("--execution-observations-quantum-trace".into());
        let parsed = parse_series_args(&instruction_trace).unwrap();
        assert!(parsed.execution_observations_quantum_trace);
        let mut elf_copy_args = base.clone();
        elf_copy_args.push("--execution-observations".into());
        elf_copy_args.push("--execution-observations-elf-stack-copy".into());
        let elf_copy = parse_series_args(&elf_copy_args).unwrap();
        assert!(elf_copy.execution_observations_elf_stack_copy);
        assert!(
            parsed
                .forwarded
                .iter()
                .all(|argument| argument != "--execution-observations-quantum-trace")
        );
        let mut composed = parsed.forwarded.clone();
        append_execution_observations_args(
            &mut composed,
            std::path::Path::new("series/preflight-execution-observations.json"),
            parsed.execution_observations_rdtsc_rip,
            parsed.execution_observations_rdtsc_stride,
            parsed.execution_observations_rdtsc_entry_rip,
            parsed.execution_observations_rdtsc_window,
            parsed.execution_observations_quantum_trace,
            parsed.execution_observations_elf_stack_copy,
            parsed.execution_observations_delay_call_summary,
        );
        assert_eq!(
            composed
                .iter()
                .filter(|argument| *argument == "--execution-observations-quantum-trace")
                .count(),
            1
        );
        composed.clear();
        append_execution_observations_args(
            &mut composed,
            std::path::Path::new("series/preflight-execution-observations.json"),
            None,
            None,
            None,
            None,
            true,
            true,
            false,
        );
        assert_eq!(
            composed
                .iter()
                .filter(|argument| *argument == "--execution-observations-quantum-trace")
                .count(),
            1
        );
        assert_eq!(
            composed
                .iter()
                .filter(|argument| *argument == "--execution-observations-elf-stack-copy")
                .count(),
            1
        );
        let mut trace_without_entry_site = execution_observations.clone();
        trace_without_entry_site.push("--execution-observations-quantum-trace".into());
        let parsed = parse_series_args(&trace_without_entry_site).unwrap();
        assert!(parsed.execution_observations_quantum_trace);
        assert_eq!(parsed.execution_observations_rdtsc_entry_rip, None);
        let mut zero_stride = selected_rdtsc.clone();
        zero_stride.extend(["--execution-observations-rdtsc-stride".into(), "0".into()]);
        assert!(
            parse_series_args(&zero_stride)
                .unwrap_err()
                .contains("positive integer")
        );
        let mut stride_without_rip = execution_observations.clone();
        stride_without_rip.extend(["--execution-observations-rdtsc-stride".into(), "512".into()]);
        assert!(
            parse_series_args(&stride_without_rip)
                .unwrap_err()
                .contains("needs")
        );
        let mut duplicate_stride = selected_rdtsc_stride.clone();
        duplicate_stride.extend(["--execution-observations-rdtsc-stride".into(), "256".into()]);
        assert!(
            parse_series_args(&duplicate_stride)
                .unwrap_err()
                .contains("duplicate")
        );
        let mut rdtsc_without_observations = base.clone();
        rdtsc_without_observations
            .extend(["--execution-observations-rdtsc-rip".into(), "0x1234".into()]);
        assert!(
            parse_series_args(&rdtsc_without_observations)
                .unwrap_err()
                .contains("needs --execution-observations")
        );
        let mut invalid_rdtsc = execution_observations.clone();
        invalid_rdtsc.extend(["--execution-observations-rdtsc-rip".into(), "1234".into()]);
        assert!(
            parse_series_args(&invalid_rdtsc)
                .unwrap_err()
                .contains("0x-prefixed")
        );
        execution_observations.push("--execution-observations".into());
        assert!(
            parse_series_args(&execution_observations)
                .unwrap_err()
                .contains("duplicate --execution-observations")
        );
        let mut direct_sidecar_path = base.clone();
        direct_sidecar_path.extend([
            "--execution-observations-out".into(),
            "observations.json".into(),
        ]);
        let error = parse_series_args(&direct_sidecar_path).unwrap_err();
        assert!(error.contains("--execution-observations-out"), "{error}");
        let mut post_iret = base.clone();
        post_iret.extend([
            "--execution-observations-post-iret-index".into(),
            "30".into(),
        ]);
        let error = parse_series_args(&post_iret).unwrap_err();
        assert!(
            error.contains("--execution-observations-post-iret-index"),
            "{error}"
        );

        let mut wrong_scenario = base.clone();
        let marker = wrong_scenario
            .iter()
            .position(|argument| argument == "A_FILE")
            .unwrap();
        wrong_scenario[marker] = "B_FILE".into();
        assert!(parse_series_args(&wrong_scenario).is_err());
    }

    #[test]
    fn observation_args_keep_per_run_sidecar_paths_and_forward_stride() {
        let rip = Some(0xffff_ffff_8271_b34d);
        let stride = Some(512);
        let entry_rip = Some(0xffff_ffff_8271_b31b);
        let mut first = vec!["--run".into()];
        append_execution_observations_args(
            &mut first,
            std::path::Path::new("series/runs/run-1/execution-observations.json"),
            rip,
            stride,
            entry_rip,
            Some(od_machine::RdtscSampleWindow {
                first_hit: 1_150_000,
                last_hit: 1_200_000,
            }),
            true,
            false,
            false,
        );
        let mut second = vec!["--run".into()];
        append_execution_observations_args(
            &mut second,
            std::path::Path::new("series/runs/run-2/execution-observations.json"),
            rip,
            stride,
            entry_rip,
            Some(od_machine::RdtscSampleWindow {
                first_hit: 1_150_000,
                last_hit: 1_200_000,
            }),
            true,
            false,
            false,
        );
        assert_ne!(first[2], second[2]);
        assert!(
            first
                .iter()
                .any(|arg| arg == "--execution-observations-quantum-trace")
        );
        assert!(first.windows(2).any(|pair| {
            pair[0] == "--execution-observations-rdtsc-entry-rip" && pair[1] == "0xffffffff8271b31b"
        }));
        assert!(first.windows(2).any(|pair| {
            pair[0] == "--execution-observations-rdtsc-window" && pair[1] == "1150000:1200000"
        }));
        for args in [&first, &second] {
            assert!(
                args.windows(2).any(
                    |pair| pair == ["--execution-observations-rdtsc-rip", "0xffffffff8271b34d"]
                )
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--execution-observations-rdtsc-stride", "512"])
            );
        }
    }

    #[test]
    fn series_cli_forwards_user_mode_as_a_flag_without_consuming_the_next_option() {
        let args = vec![
            "--test-runs".into(),
            "1".into(),
            "--series-id".into(),
            "user-mode01".into(),
            "--scenario-id".into(),
            "stage4-init-nonce".into(),
            "--expected-stop-kind".into(),
            "budget_exhausted".into(),
            "--out".into(),
            "work/stage5/user-mode01/series.json".into(),
            "--run".into(),
            "--set-cpl0".into(),
            "--user-mode".into(),
            "--linux-norandmaps".into(),
            "--linux-no-randomize-kstack-offset".into(),
            "--cpuid-profile".into(),
            "stage4a".into(),
            "--budget".into(),
            "3000000000".into(),
        ];

        let parsed = parse_series_args(&args).unwrap();
        assert!(
            parsed
                .forwarded
                .iter()
                .any(|argument| argument == "--user-mode")
        );
        assert!(
            parsed
                .forwarded
                .iter()
                .any(|argument| argument == "--linux-norandmaps")
        );
        assert!(
            parsed
                .forwarded
                .iter()
                .any(|argument| argument == "--linux-no-randomize-kstack-offset")
        );
        assert!(
            parsed
                .forwarded
                .windows(2)
                .any(|pair| pair == ["--cpuid-profile", "stage4a"])
        );
    }

    #[test]
    fn series_cli_rejects_unreservable_usize_max_count_without_panicking() {
        assert!(reserve_series::<u8>(usize::MAX).is_err());
    }

    #[test]
    fn generated_run_nonces_fit_linux_direct_and_are_distinct_within_series() {
        let nonces = (0..8)
            .map(fresh_nonce)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let unique = nonces.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), nonces.len());
        assert!(nonces.iter().all(|nonce| {
            (16..=64).contains(&nonce.len())
                && nonce.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }));
    }

    #[test]
    fn uart_normalization_preserves_non_utf8_byte_differences() {
        assert_ne!(
            normalize_uart(b"OD_NONCE=runNonce001\n\xff", "runNonce001"),
            normalize_uart(b"OD_NONCE=runNonce002\n\xfe", "runNonce002")
        );
        assert_eq!(
            normalize_uart(b"prefix runNonce001 suffix", "runNonce001"),
            normalize_uart(b"prefix runNonce002 suffix", "runNonce002")
        );
        assert_eq!(normalize_uart(b"opaque\xffbytes", ""), b"opaque\xffbytes");
    }

    fn write_test_series(root: &Path, series_id: &str, nonce: &str, marker: &str) -> PathBuf {
        let series_dir = root.join(series_id);
        let run_id = format!("{series_id}-run-0001");
        let run_dir = series_dir.join("runs").join(&run_id);
        fs::create_dir_all(&run_dir).unwrap();
        let uart =
            format!("OD_NONCE={nonce}\nOD_S5_A_{marker} nonce={nonce} value=stable\n").into_bytes();
        fs::write(run_dir.join("uart.bin"), &uart).unwrap();
        let com1_hex = uart
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let stop = json!({
            "exit_kind":"halted",
            "rip":"0x1000",
            "com1_hex":com1_hex,
            "com1_text":String::from_utf8_lossy(&uart),
            "budget":4096,
            "path":"interpreter",
            "cpu_profile":"bounded"
        });
        let raw = serde_json::to_vec(&json!({
            "nonce":nonce,
            "status":"loaded",
            "guest_started":true,
            "retired":7,
            "stop":stop,
            "cmdline":format!("od.nonce={nonce}"),
            "ram_mib":128,
            "diagnostic_past_c7":false,
            "acceptance":null,
            "inputs":{"vmlinuz":{"sha256":"abc"}}
        }))
        .unwrap();
        fs::write(run_dir.join("runner.json"), &raw).unwrap();
        let raw_value: Value = serde_json::from_slice(&raw).unwrap();
        let expected = vec![format!("A_{marker}")];
        let marker_evidence = validate_markers(&uart, nonce, &expected).evidence;
        let machine_stop_reason = machine_stop_reason_from_stop(raw_value.get("stop")).unwrap();
        let run = RunManifest {
            schema_version: SERIES_SCHEMA_VERSION,
            series_id: series_id.into(),
            run_id: run_id.clone(),
            scenario_id: "s5-A".into(),
            run_profile: RUN_PROFILE.into(),
            nonce: nonce.into(),
            hardware_status: "loaded".into(),
            scenario_status: ScenarioStatus::NotRun,
            reason: Some("S5-08 guest scenario invocation is not integrated".into()),
            machine_stop_reason,
            runner: RunnerEvidence {
                exit_code: 0,
                guest_started: true,
                retired: Some(7),
                stop: Some(stop),
                raw_json: format!("runs/{run_id}/runner.json"),
                raw_json_sha256: od_testkit::sha256_hex(&raw),
            },
            host: HostEvidence {
                os: "test-os".into(),
                arch: "test-arch".into(),
                runner_version: "0.1.0".into(),
                source_revision: None,
                source_dirty: None,
            },
            configuration: configuration_from_runner(&raw_value),
            artifact_hashes: artifact_hashes(&raw_value),
            guest_markers: marker_evidence,
            uart: UartEvidence {
                path: format!("runs/{run_id}/uart.bin"),
                sha256: od_testkit::sha256_hex(&uart),
                byte_count: uart.len(),
                capture_scope: COM1_CAPTURE_SCOPE.into(),
                transcript_complete_for_run_boot: true,
            },
            timing: TimingEvidence {
                wall_elapsed_seconds: 0.01,
                user_cpu_seconds: None,
                user_cpu_note: "not measured by linux-direct".into(),
            },
            expected_stop_kind: "halted".into(),
        };
        let run_bytes = serde_json::to_vec_pretty(&run).unwrap();
        let run_path = run_dir.join("manifest.json");
        fs::write(&run_path, &run_bytes).unwrap();
        let reference = RunReference {
            run_id,
            manifest: format!("runs/{series_id}-run-0001/manifest.json"),
            manifest_sha256: od_testkit::sha256_hex(&run_bytes),
        };
        let series = SeriesManifest {
            schema_version: SERIES_SCHEMA_VERSION,
            series_id: series_id.into(),
            scenario_id: "s5-A".into(),
            run_profile: RUN_PROFILE.into(),
            requested_runs: 1,
            outcome: ScenarioStatus::NotRun,
            runs: vec![reference],
        };
        let series_path = series_dir.join("series.json");
        fs::write(&series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        series_path
    }

    fn stage4_nonce_comparison_run(
        mut run: RunManifest,
        series_root: &Path,
        nonce: &str,
        variant: &str,
    ) -> RunManifest {
        let dynamic = matches!(
            variant,
            "timing" | "failed" | "no-panic" | "optional-pid-frame" | "nonce-order"
        );
        let marker_nonce = if matches!(variant, "marker" | "failed") {
            "wrongMarkerNonce"
        } else {
            nonce
        };
        let time = if dynamic { "2.273117" } else { "2.273241" };
        let optional_frame = if variant == "optional-pid-frame" {
            "? __task_pid_nr_ns+0x6c/0x120"
        } else if dynamic {
            "? handle_pte_fault+0x1cb/0x1f0"
        } else {
            "? ksys_write+0x71/0xf0"
        };
        let mut lines = vec![
            format!(
                "[{time}] Memory: {}K/130360K available (stable)",
                if dynamic { 61632 } else { 62064 }
            ),
            format!(
                "[{time}] audit: type=2000 audit({}): state=initialized audit_enabled=0 res=1",
                if dynamic { "0.141:1" } else { "0.140:1" }
            ),
            format!(
                "[{time}] sched_clock: Marking stable ({}, {})->({}, {})",
                if dynamic { 727401607 } else { 727600823 },
                if dynamic { 1447693312 } else { 1447617526 },
                if dynamic { "2175508456" } else { "2175432185" },
                if dynamic { "-413537" } else { "-213836" }
            ),
            format!("OD_NONCE={marker_nonce}"),
            format!("[{time}] Run /init as init process"),
            format!(
                "[{time}] Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000"
            ),
            format!(
                "[{time}] CPU: 0 UID: 0 PID: 1 Comm: init Not tainted 7.0.0-38-generic #38-Ubuntu PREEMPT(lazy)"
            ),
            format!("[{time}] Call Trace:"),
        ];
        if variant != "nonce-order" {
            lines.swap(3, 4);
        }
        if variant == "warning" {
            lines.push(format!(
                "[{time}] WARNING: unexpected condition in panic trace"
            ));
        }
        if variant == "unknown-frame" {
            lines.push(format!("[{time}]  ? unexpected_symbol+0x1/0x2"));
        }
        if variant != "no-panic" {
            lines.extend([
            format!("[{time}]  <TASK>"),
            format!("[{time}]  show_stack+0x49/0x60"),
            format!("[{time}]  dump_stack_lvl+0x5f/0x90"),
            format!("[{time}]  dump_stack+0x10/0x18"),
            format!("[{time}]  vpanic+0x262/0x4b0"),
            format!("[{time}]  panic+0x67/0x70"),
            format!("[{time}]  ? acct_update_integrals+0x5e/0xa0"),
            format!("[{time}]  do_exit.cold+0x15/0x15"),
            format!("[{time}]  __x64_sys_exit+0x1b/0x20"),
            format!("[{time}]  x64_sys_call+0x2384/0x2390"),
            format!("[{time}]  do_syscall_64+0x105/0x5a0"),
            format!("[{time}]  {optional_frame}"),
            format!("[{time}]  entry_SYSCALL_64_after_hwframe+0x76/0x7e"),
            format!("[{time}] RIP: 0033:0x40024d"),
            format!("[{time}] Code: 00 00 b8 01 00 00 00 0f 05 <eb> fe"),
            format!(
                "[{time}] RSP: 002b:{} EFLAGS: 00000246 ORIG_RAX: 000000000000003c",
                if dynamic { "00007fff745f12d0" } else { "00007fff22bec9f0" }
            ),
            format!("[{time}] RAX: ffffffffffffffda RBX: 0000000000000040 RCX: 000000000040024d"),
            format!("[{time}] RDX: 000000000000002d RSI: 0000000000000000 RDI: 0000000000000000"),
            format!("[{time}] RBP: 0000000000000000 R08: 0000000000401049 R09: 0000000000000023"),
            format!("[{time}] R10: 0000000000000000 R11: 0000000000000246 R12: 0000000000000003"),
            format!("[{time}] R13: 0000000000000075 R14: 0000000000401000 R15: 0000000000000000"),
            format!("[{time}]  </TASK>"),
            format!("[{time}] Kernel Offset: disabled"),
            format!("[{time}] ---[ end Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000 ]---"),
            ]);
        }
        let uart = lines
            .into_iter()
            .map(|line| format!("{line}\r\n"))
            .collect::<String>()
            .into_bytes();
        let uart_relative = format!("runs/{nonce}/uart.bin");
        let uart_path = series_root.join(&uart_relative);
        fs::create_dir_all(uart_path.parent().unwrap()).unwrap();
        fs::write(&uart_path, &uart).unwrap();

        let exit_kind = if variant == "stop" {
            "halted"
        } else {
            "budget_exhausted"
        };
        let stop = json!({
            "exit_kind":exit_kind,
            "budget":3000000000u64,
            "retired":2999999998u64,
            "path":"cpl0_set",
            "cpu_profile":"Cpl0ProfileSet (S2-45) of od-isa under Cpl0BaseLong64V1, 2 MiB walk rebuilt on MOV CR3; members in set_members",
            "user_mode":true,
            "syscall_journal":{
                "entered":9,
                "returned":8,
            "entries":[
                {"nr":83},
                {"nr":165},
                {"nr":257},
                {"nr":0},
                {"nr":3},
                {"nr":1},
                {"nr":39},
                {"nr":35},
                {"nr":if variant == "journal" {61} else {60},"rax_return":null}
            ]
            },
            "cr3":if dynamic {"0x649c000"} else {"0x64a6000"},
            "last_cr3_change":if dynamic {"0x649c000"} else {"0x64a6000"},
            "gpr":if dynamic {json!({"rax":"0x8b8e1fcdb"})} else {json!({"rax":"0x8b98444fc"})},
            "rflags":if dynamic {"0x287"} else {"0x246"},
            "rip":if dynamic {"0xffffffff8271b32c"} else {"0xffffffff8271b34d"},
            "rip_bytes":if dynamic {"65 ff 0d fd 1c d0 01"} else {"0f 31 0f 1f 00"},
            "rip_gpa":if dynamic {"0x271b32c"} else {"0x271b34d"},
            "halt_idle_ns":if dynamic {1139468} else {1187773},
            "virtual_ns":if dynamic {37461557466u64} else {37472192771u64},
            "irq0_edges":if dynamic {34803} else {34814},
            "irq_refused_masked":if dynamic {1150205} else {954633},
            "irqs_delivered":if dynamic {34808} else {34819},
            "quanta":if dynamic {34569643} else {34579979},
            "quantum_cut_by_deadline":if dynamic {33170411} else {33180705},
            "machine_stop_reason":null
        });
        run.scenario_id = "stage4-init-nonce".into();
        run.nonce = nonce.into();
        run.expected_stop_kind = "budget_exhausted".into();
        run.configuration = json!({
            "cmdline":format!("od.nonce={nonce}"),
            "ram_mib":128,
            "budget":3000000000u64,
            "cpu_path":"cpl0_set",
            "cpu_profile":"Cpl0ProfileSet (S2-45) of od-isa under Cpl0BaseLong64V1, 2 MiB walk rebuilt on MOV CR3; members in set_members",
            "acceptance":"counted",
            "diagnostic_past_c7":false
        });
        run.artifact_hashes.insert(
            "vmlinuz".into(),
            if variant == "artifact" {
                "vmlinuz-changed"
            } else {
                "vmlinuz-stable"
            }
            .into(),
        );
        run.guest_markers = validate_markers(&uart, nonce, &[]).evidence;
        run.runner.stop = Some(stop.clone());
        run.runner.retired = Some(2999999998);
        run.uart.path = uart_relative;
        run.uart.sha256 = od_testkit::sha256_hex(&uart);
        run.uart.byte_count = uart.len();
        run.scenario_status = scenario_status(
            &run.scenario_id,
            &run.hardware_status,
            run.runner.guest_started,
            &run.guest_markers,
            Some(&stop),
            &run.expected_stop_kind,
        );
        run.reason = scenario_reason(
            &run.scenario_id,
            &run.hardware_status,
            run.runner.guest_started,
            &run.guest_markers,
            Some(&stop),
            &run.expected_stop_kind,
        );
        run
    }

    fn write_crlf_test_series(
        root: &Path,
        series_id: &str,
        legacy_marker_evidence: bool,
        schema_version: u32,
    ) -> PathBuf {
        let nonce = "nonceCompatibility0001";
        let series_path = write_test_series(root, series_id, nonce, "PASS");
        let series_dir = series_path.parent().unwrap();
        let mut series: Value = serde_json::from_slice(&fs::read(&series_path).unwrap()).unwrap();
        let run_path = series_dir.join(series["runs"][0]["manifest"].as_str().unwrap());
        let mut run: RunManifest = serde_json::from_slice(&fs::read(&run_path).unwrap()).unwrap();
        let uart_path = series_dir.join(&run.uart.path);
        let uart = String::from_utf8(fs::read(&uart_path).unwrap())
            .unwrap()
            .replace('\n', "\r\n")
            .into_bytes();
        fs::write(&uart_path, &uart).unwrap();

        let raw_path = series_dir.join(&run.runner.raw_json);
        let mut raw: Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
        let com1_hex = uart
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        raw["stop"]["com1_hex"] = json!(com1_hex);
        raw["stop"]["com1_text"] = json!(String::from_utf8_lossy(&uart).into_owned());
        let raw_bytes = serde_json::to_vec(&raw).unwrap();
        fs::write(&raw_path, &raw_bytes).unwrap();

        run.runner.raw_json_sha256 = od_testkit::sha256_hex(&raw_bytes);
        run.runner.stop = raw.get("stop").cloned();
        run.uart.sha256 = od_testkit::sha256_hex(&uart);
        run.uart.byte_count = uart.len();
        let checked = if legacy_marker_evidence {
            validate_markers_legacy(&uart, nonce, &run.guest_markers.expected_s5_marker_keys)
        } else {
            validate_markers(&uart, nonce, &run.guest_markers.expected_s5_marker_keys)
        };
        run.guest_markers = checked.evidence;
        run.scenario_status = scenario_status(
            &run.scenario_id,
            &run.hardware_status,
            run.runner.guest_started,
            &run.guest_markers,
            run.runner.stop.as_ref(),
            &run.expected_stop_kind,
        );
        run.reason = scenario_reason(
            &run.scenario_id,
            &run.hardware_status,
            run.runner.guest_started,
            &run.guest_markers,
            run.runner.stop.as_ref(),
            &run.expected_stop_kind,
        );
        let mut run_value = serde_json::to_value(&run).unwrap();
        run_value["schema_version"] = json!(schema_version);
        series["schema_version"] = json!(schema_version);
        if schema_version == 1 {
            run_value["host"]
                .as_object_mut()
                .unwrap()
                .remove("source_dirty");
        }
        let run_bytes = serde_json::to_vec_pretty(&run_value).unwrap();
        fs::write(&run_path, &run_bytes).unwrap();
        series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&run_bytes));
        series["outcome"] = serde_json::to_value(run.scenario_status).unwrap();
        fs::write(&series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        series_path
    }

    fn rewrite_run_and_series(series_path: &Path, edit: impl FnOnce(&mut Value)) {
        let root = series_path.parent().unwrap();
        let mut series: Value = serde_json::from_slice(&fs::read(series_path).unwrap()).unwrap();
        let relative = series["runs"][0]["manifest"].as_str().unwrap();
        let manifest_path = root.join(relative);
        let mut run: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        edit(&mut run);
        let bytes = serde_json::to_vec_pretty(&run).unwrap();
        fs::write(&manifest_path, &bytes).unwrap();
        series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&bytes));
        fs::write(series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
    }

    fn rewrite_raw_and_series(series_path: &Path, edit: impl FnOnce(&mut Value)) {
        let root = series_path.parent().unwrap();
        let mut series: Value = serde_json::from_slice(&fs::read(series_path).unwrap()).unwrap();
        let run_path = root.join(series["runs"][0]["manifest"].as_str().unwrap());
        let mut run: Value = serde_json::from_slice(&fs::read(&run_path).unwrap()).unwrap();
        let raw_path = root.join(run["runner"]["raw_json"].as_str().unwrap());
        let mut raw: Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
        edit(&mut raw);
        run["nonce"] = raw["nonce"].clone();
        let raw_bytes = serde_json::to_vec_pretty(&raw).unwrap();
        fs::write(&raw_path, &raw_bytes).unwrap();
        run["runner"]["raw_json_sha256"] = json!(od_testkit::sha256_hex(&raw_bytes));
        let run_bytes = serde_json::to_vec_pretty(&run).unwrap();
        fs::write(&run_path, &run_bytes).unwrap();
        series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&run_bytes));
        fs::write(series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
    }

    fn set_machine_stop_reason(series_path: &Path, reason: Option<&str>) {
        let root = series_path.parent().unwrap();
        let mut series: Value = serde_json::from_slice(&fs::read(series_path).unwrap()).unwrap();
        let manifest_path = root.join(series["runs"][0]["manifest"].as_str().unwrap());
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        let raw_path = root.join(manifest["runner"]["raw_json"].as_str().unwrap());
        let mut raw: Value = serde_json::from_slice(&fs::read(&raw_path).unwrap()).unwrap();
        let stop = raw["stop"].as_object_mut().unwrap();
        match reason {
            Some(reason) => {
                stop.insert("machine_stop_reason".into(), json!(reason));
                manifest["machine_stop_reason"] = json!(reason);
            }
            None => {
                stop.remove("machine_stop_reason");
                manifest
                    .as_object_mut()
                    .unwrap()
                    .remove("machine_stop_reason");
            }
        }
        let raw_bytes = serde_json::to_vec_pretty(&raw).unwrap();
        fs::write(&raw_path, &raw_bytes).unwrap();
        manifest["runner"]["raw_json_sha256"] = json!(od_testkit::sha256_hex(&raw_bytes));
        manifest["runner"]["stop"] = raw["stop"].clone();
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        fs::write(&manifest_path, &manifest_bytes).unwrap();
        series["runs"][0]["manifest_sha256"] = json!(od_testkit::sha256_hex(&manifest_bytes));
        fs::write(series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
    }

    fn append_failed_test_run(series_path: &Path, nonce: &str) {
        let root = series_path.parent().unwrap();
        let mut series: Value = serde_json::from_slice(&fs::read(series_path).unwrap()).unwrap();
        let series_id = series["series_id"].as_str().unwrap().to_string();
        let run_id = format!("{series_id}-run-0002");
        let run_dir = root.join("runs").join(&run_id);
        fs::create_dir_all(&run_dir).unwrap();
        let uart =
            format!("OD_NONCE={nonce}\nOD_S5_A_PASS nonce={nonce} value=stable\n").into_bytes();
        fs::write(run_dir.join("uart.bin"), &uart).unwrap();
        let com1_hex = uart
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let stop = json!({
            "exit_kind":"halted", "rip":"0x1000", "com1_hex":com1_hex,
            "com1_text":String::from_utf8_lossy(&uart), "budget":4096,
            "path":"interpreter", "cpu_profile":"bounded"
        });
        let mut raw: Value = serde_json::from_slice(
            &fs::read(
                root.join(series["runs"][0]["manifest"].as_str().unwrap())
                    .parent()
                    .unwrap_or(root)
                    .join("runner.json"),
            )
            .unwrap(),
        )
        .unwrap();
        raw["nonce"] = json!(nonce);
        raw["status"] = json!("failed");
        raw["stop"] = stop.clone();
        let raw_bytes = serde_json::to_vec(&raw).unwrap();
        fs::write(run_dir.join("runner.json"), &raw_bytes).unwrap();
        let raw_value: Value = serde_json::from_slice(&raw_bytes).unwrap();
        let markers = validate_markers(&uart, nonce, &["A_PASS".to_string()]).evidence;
        let machine_stop_reason = machine_stop_reason_from_stop(raw_value.get("stop")).unwrap();
        let run = RunManifest {
            schema_version: SERIES_SCHEMA_VERSION,
            series_id: series_id.clone(),
            run_id: run_id.clone(),
            scenario_id: "s5-A".into(),
            run_profile: RUN_PROFILE.into(),
            nonce: nonce.into(),
            hardware_status: "failed".into(),
            scenario_status: ScenarioStatus::Failed,
            reason: Some("runner execution failed after guest start".into()),
            machine_stop_reason,
            runner: RunnerEvidence {
                exit_code: 1,
                guest_started: true,
                retired: raw_value["retired"].as_u64(),
                stop: Some(stop),
                raw_json: format!("runs/{run_id}/runner.json"),
                raw_json_sha256: od_testkit::sha256_hex(&raw_bytes),
            },
            host: serde_json::from_value(
                serde_json::from_slice::<Value>(
                    &fs::read(root.join(series["runs"][0]["manifest"].as_str().unwrap())).unwrap(),
                )
                .unwrap()["host"]
                    .clone(),
            )
            .unwrap(),
            configuration: configuration_from_runner(&raw_value),
            artifact_hashes: artifact_hashes(&raw_value),
            guest_markers: markers,
            uart: UartEvidence {
                path: format!("runs/{run_id}/uart.bin"),
                sha256: od_testkit::sha256_hex(&uart),
                byte_count: uart.len(),
                capture_scope: COM1_CAPTURE_SCOPE.into(),
                transcript_complete_for_run_boot: true,
            },
            timing: serde_json::from_value(
                serde_json::from_slice::<Value>(
                    &fs::read(root.join(series["runs"][0]["manifest"].as_str().unwrap())).unwrap(),
                )
                .unwrap()["timing"]
                    .clone(),
            )
            .unwrap(),
            expected_stop_kind: "halted".into(),
        };
        let manifest_bytes = serde_json::to_vec_pretty(&run).unwrap();
        fs::write(run_dir.join("manifest.json"), &manifest_bytes).unwrap();
        series["requested_runs"] = json!(2);
        series["outcome"] = json!("failed");
        series["runs"].as_array_mut().unwrap().push(json!({
            "run_id":run_id,
            "manifest":format!("runs/{run_id}/manifest.json"),
            "manifest_sha256":od_testkit::sha256_hex(&manifest_bytes)
        }));
        fs::write(series_path, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
    }

    #[test]
    fn stage5_comparator_explains_identity_only_and_reports_marker_substitution() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-series-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let left_root = temp.join("left");
        let right_root = temp.join("right");
        fs::create_dir_all(&temp).unwrap();
        let left = write_test_series(&left_root, "series-left", "nonceLeft00000100", "PASS");
        let matching = write_test_series(&right_root, "series-right", "nonceRight0000100", "PASS");
        let report = compare_series_paths(&left, &matching).unwrap();
        assert!(report.matches, "{report:?}");
        assert!(!report.explained_differences.is_empty());

        let substituted_root = temp.join("substituted");
        let substituted =
            write_test_series(&substituted_root, "series-sub", "nonceSub00000100", "FAIL");
        let report = compare_series_paths(&left, &substituted).unwrap();
        assert!(!report.matches);
        assert!(
            report
                .unexplained_differences
                .iter()
                .any(|value| value.contains("ordered_s5_marker_keys"))
        );

        let host_root = temp.join("host-diff");
        let host_diff = write_test_series(&host_root, "series-host", "nonceHost00000100", "PASS");
        rewrite_run_and_series(&host_diff, |run| run["host"]["os"] = json!("different-os"));
        let report = compare_series_paths(&left, &host_diff).unwrap();
        assert!(!report.matches);
        assert!(
            report
                .unexplained_differences
                .iter()
                .any(|value| value.contains("host_environment"))
        );
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn within_series_comparator_compares_each_run_to_the_first() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-within-series-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let seed = write_test_series(&temp.join("seed"), "seed", "nonceStage5Seed0001", "PASS");
        let manifest = fresh_manifest(&seed).unwrap();
        let base = load_run_manifest(&seed, &manifest.runs[0]).unwrap();
        let series_root = temp.join("within");
        fs::create_dir_all(&series_root).unwrap();
        let series_path = series_root.join("series.json");
        let first = stage4_nonce_comparison_run(
            base.clone(),
            &series_root,
            "nonceStage5First0001",
            "baseline",
        );
        let second =
            stage4_nonce_comparison_run(base, &series_root, "nonceStage5Second01", "timing");
        let third = stage4_nonce_comparison_run(
            load_run_manifest(&seed, &manifest.runs[0]).unwrap(),
            &series_root,
            "nonceStage5Third0001",
            "artifact",
        );

        let pair = compare_run_sequence(&series_path, &[first.clone(), second.clone()]).unwrap();
        assert!(pair.matches, "{:?}", pair.unexplained_differences);
        let comparison =
            compare_run_sequence(&series_path, &[first.clone(), second, third.clone()]).unwrap();
        assert!(!comparison.matches);
        assert!(
            comparison
                .unexplained_differences
                .iter()
                .any(|item| { item.contains("run[2].artifact_hashes") })
        );
        assert_eq!(comparison.comparison, "linux_stage5_within_series");
        assert!(!comparison.explained_differences.is_empty());
        assert!(compare_run_sequence(&series_path, &[first]).is_err());
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn stage4_nonce_comparator_explains_only_observed_run_variation() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage4-nonce-compare-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let seed = write_test_series(&temp.join("seed"), "seed", "nonceStage4Seed0001", "PASS");
        let manifest = fresh_manifest(&seed).unwrap();
        let base = load_run_manifest(&seed, &manifest.runs[0]).unwrap();
        let left_root = temp.join("left");
        let right_root = temp.join("right");
        fs::create_dir_all(&left_root).unwrap();
        fs::create_dir_all(&right_root).unwrap();
        let left_path = left_root.join("series.json");
        let right_path = right_root.join("series.json");
        let left = stage4_nonce_comparison_run(
            base.clone(),
            &left_root,
            "nonceStage4Left0001",
            "baseline",
        );
        let timing =
            stage4_nonce_comparison_run(base.clone(), &right_root, "nonceStage4Timing01", "timing");
        let mut explained = Vec::new();
        let mut unexplained = Vec::new();
        compare_run(
            0,
            &left_path,
            &right_path,
            &left,
            &timing,
            &mut explained,
            &mut unexplained,
        )
        .unwrap();
        assert!(unexplained.is_empty(), "{unexplained:?}");
        assert!(
            explained
                .iter()
                .any(|item| item.contains("terminal CPU snapshot"))
        );
        assert!(
            explained
                .iter()
                .any(|item| item.contains("UART timestamps"))
        );
        assert!(
            explained
                .iter()
                .any(|item| item.contains("audit initialization timestamp"))
        );

        let optional_pid_frame = stage4_nonce_comparison_run(
            base.clone(),
            &right_root,
            "nonceStage4OptionalPid01",
            "optional-pid-frame",
        );
        let mut explained = Vec::new();
        let mut unexplained = Vec::new();
        compare_run(
            0,
            &left_path,
            &right_path,
            &left,
            &optional_pid_frame,
            &mut explained,
            &mut unexplained,
        )
        .unwrap();
        assert!(unexplained.is_empty(), "{unexplained:?}");
        assert!(
            explained
                .iter()
                .any(|item| item.contains("terminal CPU snapshot"))
        );
        assert!(
            explained
                .iter()
                .any(|item| item.contains("UART timestamps"))
        );
        assert!(
            explained
                .iter()
                .any(|item| item.contains("audit initialization timestamp"))
        );

        let nonce_order = stage4_nonce_comparison_run(
            base.clone(),
            &right_root,
            "nonceStage4Order01",
            "nonce-order",
        );
        let mut explained = Vec::new();
        let mut unexplained = Vec::new();
        compare_run(
            0,
            &left_path,
            &right_path,
            &left,
            &nonce_order,
            &mut explained,
            &mut unexplained,
        )
        .unwrap();
        assert!(unexplained.is_empty(), "{unexplained:?}");
        assert!(
            explained
                .iter()
                .any(|item| item.contains("UART timestamps"))
        );
        assert!(
            explained
                .iter()
                .any(|item| item.contains("init/nonce console order"))
        );

        for (variant, expected) in [
            ("artifact", "artifact_hashes"),
            ("marker", "marker_validation"),
            ("stop", "machine_observations"),
            ("journal", "machine_observations"),
            ("warning", "ordered_uart_bytes"),
            ("unknown-frame", "ordered_uart_bytes"),
            ("no-panic", "machine_observations"),
        ] {
            let right = stage4_nonce_comparison_run(
                base.clone(),
                &right_root,
                &format!("nonceStage4{}0001", variant),
                variant,
            );
            let mut explained = Vec::new();
            let mut unexplained = Vec::new();
            compare_run(
                0,
                &left_path,
                &right_path,
                &left,
                &right,
                &mut explained,
                &mut unexplained,
            )
            .unwrap();
            assert!(
                unexplained.iter().any(|item| item.contains(expected)),
                "{variant}: {unexplained:?}"
            );
        }

        let failed = stage4_nonce_comparison_run(
            base.clone(),
            &right_root,
            "nonceStage4Failed0001",
            "failed",
        );
        let mut explained = Vec::new();
        let mut unexplained = Vec::new();
        compare_run(
            0,
            &left_path,
            &right_path,
            &left,
            &failed,
            &mut explained,
            &mut unexplained,
        )
        .unwrap();
        assert!(
            unexplained
                .iter()
                .any(|item| item.contains("machine_observations")),
            "failed scenario must retain strict stop comparison: {unexplained:?}"
        );
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn sched_clock_normalization_requires_four_integer_values() {
        assert_eq!(
            normalize_sched_clock_calibration("sched_clock: Marking stable (1, 2)->(3, -4)"),
            "sched_clock: Marking stable (<calibration>)"
        );
        let malformed = "sched_clock: Marking stable (1)->(3)";
        assert_eq!(normalize_sched_clock_calibration(malformed), malformed);
    }

    #[test]
    fn stage4_nonce_normalizes_only_the_exact_audit_init_event_time() {
        assert_eq!(
            normalize_stage4_init_nonce_uart_line(
                "[    1.588849] audit: type=2000 audit(0.140:1): state=initialized audit_enabled=0 res=1\r\n"
            ),
            "[<kernel-time>] audit: type=2000 audit(<event-time>:1): state=initialized audit_enabled=0 res=1\r\n"
        );
        let unrelated = "audit: type=1300 audit(0.140:1): op=USER_LOGIN";
        assert_eq!(normalize_stage4_audit_init_timestamp(unrelated), unrelated);
        let malformed = "audit: type=2000 audit(0.140:2): state=initialized audit_enabled=0 res=1";
        assert_eq!(normalize_stage4_audit_init_timestamp(malformed), malformed);
    }

    #[test]
    fn stage4_nonce_reorders_only_a_unique_adjacent_init_marker_pair() {
        let mut adjacent = vec![
            "OD_NONCE=<nonce>\r\n".to_string(),
            "[<kernel-time>] Run /init as init process\r\n".to_string(),
        ];
        normalize_stage4_init_nonce_marker_order(&mut adjacent);
        assert_eq!(
            adjacent,
            [
                "[<kernel-time>] Run /init as init process\r\n",
                "OD_NONCE=<nonce>\r\n"
            ]
        );

        let mut separated = vec![
            "OD_NONCE=<nonce>\r\n".to_string(),
            "unrelated output\r\n".to_string(),
            "[<kernel-time>] Run /init as init process\r\n".to_string(),
        ];
        let original = separated.clone();
        normalize_stage4_init_nonce_marker_order(&mut separated);
        assert_eq!(separated, original);

        let mut duplicate = vec![
            "OD_NONCE=<nonce>\r\n".to_string(),
            "[<kernel-time>] Run /init as init process\r\n".to_string(),
            "OD_NONCE=<nonce>\r\n".to_string(),
        ];
        let original = duplicate.clone();
        normalize_stage4_init_nonce_marker_order(&mut duplicate);
        assert_eq!(duplicate, original);
    }

    #[test]
    fn typed_machine_stop_reason_is_validated_and_legacy_absence_is_unobserved() {
        assert_eq!(
            machine_stop_reason_from_stop(Some(&json!({"machine_stop_reason":"reset"}))).unwrap(),
            Some(MachineStopReasonEvidence::Reset)
        );
        assert_eq!(
            machine_stop_reason_from_stop(Some(&json!({"machine_stop_reason":"shutdown"})))
                .unwrap(),
            Some(MachineStopReasonEvidence::Shutdown)
        );
        assert_eq!(
            machine_stop_reason_from_stop(Some(&json!({"machine_stop_reason":null}))).unwrap(),
            None
        );
        assert_eq!(
            machine_stop_reason_from_stop(Some(&json!({}))).unwrap(),
            None
        );
        assert_eq!(machine_stop_reason_from_stop(None).unwrap(), None);
        assert!(
            machine_stop_reason_from_stop(Some(&json!({"machine_stop_reason":"halt"})))
                .unwrap_err()
                .contains("reset, shutdown, or null")
        );

        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-stop-reason-legacy-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = write_test_series(&temp, "legacy-series", "nonceLegacyStop0001", "PASS");
        // Simulate both legacy JSON layers predating the additive field.
        set_machine_stop_reason(&legacy, None);
        assert!(compare_series_paths(&legacy, &legacy).unwrap().matches);
        let manifest = fresh_manifest(&legacy).unwrap();
        let loaded = load_series_runs(&legacy, &manifest).unwrap();
        assert_eq!(loaded[0].machine_stop_reason, None);
        assert_eq!(loaded[0].scenario_status, ScenarioStatus::NotRun);

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn typed_machine_stop_reason_is_integrity_checked_and_compared_without_passing_scenario() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-stop-reason-compare-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let reset = write_test_series(
            &temp.join("reset"),
            "reset-series",
            "nonceResetStop00001",
            "PASS",
        );
        let shutdown = write_test_series(
            &temp.join("shutdown"),
            "shutdown-series",
            "nonceShutdownStop001",
            "PASS",
        );
        set_machine_stop_reason(&reset, Some("reset"));
        set_machine_stop_reason(&shutdown, Some("shutdown"));

        let manifest = fresh_manifest(&reset).unwrap();
        let reset_run = load_series_runs(&reset, &manifest).unwrap();
        assert_eq!(reset_run[0].scenario_status, ScenarioStatus::NotRun);
        assert_eq!(
            reset_run[0].machine_stop_reason,
            Some(MachineStopReasonEvidence::Reset)
        );
        let comparison = compare_series_paths(&reset, &shutdown).unwrap();
        assert!(!comparison.matches);
        assert!(
            comparison
                .unexplained_differences
                .iter()
                .any(|difference| difference == "run[0].machine_stop_reason differs")
        );

        let tampered = write_test_series(
            &temp.join("tampered"),
            "tampered-series",
            "nonceTamperedStop001",
            "PASS",
        );
        set_machine_stop_reason(&tampered, Some("shutdown"));
        rewrite_run_and_series(&tampered, |manifest| {
            manifest["machine_stop_reason"] = json!("reset");
        });
        assert!(
            compare_series_paths(&reset, &tampered)
                .unwrap_err()
                .contains("machine stop reason does not match raw runner JSON")
        );

        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn series_integrity_rejects_tampered_derived_fields_and_aggregate() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-integrity-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        let left = write_test_series(
            &temp.join("left"),
            "series-left",
            "nonceLeft00000100",
            "PASS",
        );

        let config = write_test_series(
            &temp.join("config"),
            "series-config",
            "nonceConfig00001",
            "PASS",
        );
        rewrite_run_and_series(&config, |run| run["configuration"]["ram_mib"] = json!(999));
        assert!(
            compare_series_paths(&left, &config)
                .unwrap_err()
                .contains("configuration or artifact hashes")
        );

        let status = write_test_series(
            &temp.join("status"),
            "series-status",
            "nonceStatus00001",
            "PASS",
        );
        rewrite_run_and_series(&status, |run| run["scenario_status"] = json!("passed"));
        assert!(
            compare_series_paths(&left, &status)
                .unwrap_err()
                .contains("scenario status/reason")
        );

        let raw_status = write_test_series(
            &temp.join("raw-status"),
            "series-raw-status",
            "nonceRawStatus001",
            "PASS",
        );
        rewrite_raw_and_series(&raw_status, |raw| raw["status"] = json!("refused"));
        assert!(
            compare_series_paths(&left, &raw_status)
                .unwrap_err()
                .contains("runner summary")
        );

        let empty_nonce = write_test_series(
            &temp.join("empty-nonce"),
            "series-empty-nonce",
            "nonceEmpty000001",
            "PASS",
        );
        rewrite_raw_and_series(&empty_nonce, |raw| raw["nonce"] = json!(""));
        assert!(
            compare_series_paths(&left, &empty_nonce)
                .unwrap_err()
                .contains("nonce must be 16..=64")
        );

        let outcome = write_test_series(
            &temp.join("outcome"),
            "series-outcome",
            "nonceOutcome0001",
            "PASS",
        );
        let mut series: Value = serde_json::from_slice(&fs::read(&outcome).unwrap()).unwrap();
        series["outcome"] = json!("passed");
        fs::write(&outcome, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        assert!(
            compare_series_paths(&left, &outcome)
                .unwrap_err()
                .contains("series outcome")
        );
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn series_manifest_rejects_empty_count_and_duplicate_run_reference() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-manifest-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        let zero = write_test_series(
            &temp.join("zero"),
            "series-zero",
            "nonceZero00000100",
            "PASS",
        );
        let mut series: Value = serde_json::from_slice(&fs::read(&zero).unwrap()).unwrap();
        series["requested_runs"] = json!(0);
        series["runs"] = json!([]);
        fs::write(&zero, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        assert!(fresh_manifest(&zero).unwrap_err().contains("run count"));

        let duplicate = write_test_series(
            &temp.join("duplicate"),
            "series-duplicate",
            "nonceDupe00000100",
            "PASS",
        );
        let mut series: Value = serde_json::from_slice(&fs::read(&duplicate).unwrap()).unwrap();
        series["requested_runs"] = json!(2);
        let first = series["runs"][0].clone();
        series["runs"] = json!([first.clone(), first]);
        fs::write(&duplicate, serde_json::to_vec_pretty(&series).unwrap()).unwrap();
        assert!(
            fresh_manifest(&duplicate)
                .unwrap_err()
                .contains("duplicate run reference")
        );

        let duplicate_nonce = write_test_series(
            &temp.join("nonce"),
            "series-nonce",
            "nonceSame00000100",
            "PASS",
        );
        let root = duplicate_nonce.parent().unwrap();
        let mut series: Value =
            serde_json::from_slice(&fs::read(&duplicate_nonce).unwrap()).unwrap();
        let first_ref = series["runs"][0].clone();
        let first_manifest_path = root.join(first_ref["manifest"].as_str().unwrap());
        let first_run_dir = first_manifest_path.parent().unwrap();
        let second_run_id = "series-nonce-run-0002";
        let second_run_dir = root.join("runs").join(second_run_id);
        fs::create_dir_all(&second_run_dir).unwrap();
        fs::copy(
            first_run_dir.join("runner.json"),
            second_run_dir.join("runner.json"),
        )
        .unwrap();
        fs::copy(
            first_run_dir.join("uart.bin"),
            second_run_dir.join("uart.bin"),
        )
        .unwrap();
        let mut second_manifest: Value =
            serde_json::from_slice(&fs::read(&first_manifest_path).unwrap()).unwrap();
        second_manifest["run_id"] = json!(second_run_id);
        second_manifest["runner"]["raw_json"] = json!(format!("runs/{second_run_id}/runner.json"));
        second_manifest["uart"]["path"] = json!(format!("runs/{second_run_id}/uart.bin"));
        let second_bytes = serde_json::to_vec_pretty(&second_manifest).unwrap();
        fs::write(second_run_dir.join("manifest.json"), &second_bytes).unwrap();
        series["requested_runs"] = json!(2);
        series["runs"].as_array_mut().unwrap().push(json!({
            "run_id":second_run_id,
            "manifest":format!("runs/{second_run_id}/manifest.json"),
            "manifest_sha256":od_testkit::sha256_hex(&second_bytes)
        }));
        fs::write(
            &duplicate_nonce,
            serde_json::to_vec_pretty(&series).unwrap(),
        )
        .unwrap();
        assert!(
            compare_series_paths(&duplicate_nonce, &duplicate_nonce)
                .unwrap_err()
                .contains("duplicate run nonce")
        );
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn comparison_validates_trailing_failed_run_in_unequal_length_series() {
        let temp = std::env::temp_dir().join(format!(
            "od-runner-stage5-lengths-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).unwrap();
        let one = write_test_series(&temp.join("one"), "series-one", "nonceOne00000001", "PASS");
        let two = write_test_series(&temp.join("two"), "series-two", "nonceTwo00000001", "PASS");
        append_failed_test_run(&two, "nonceThree000001");
        let report = compare_series_paths(&one, &two).unwrap();
        assert!(!report.matches);
        assert!(
            report
                .unexplained_differences
                .iter()
                .any(|item| item.contains("run count"))
        );
        assert!(
            report
                .unexplained_differences
                .iter()
                .any(|item| item.contains("outcome"))
        );
        let _ = fs::remove_dir_all(temp);
    }
}
