use super::{evaluate, parse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const SERIES_SCHEMA_VERSION: u32 = 1;
const RUN_PROFILE: &str = "smoke_not_acceptance";
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
    pub host: HostEvidence,
    pub configuration: Value,
    pub artifact_hashes: BTreeMap<String, String>,
    pub guest_markers: MarkerEvidence,
    pub uart: UartEvidence,
    pub timing: TimingEvidence,
    pub expected_stop_kind: String,
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
        let mut run_args = cli.forwarded.clone();
        run_args.extend([
            "--nonce".into(),
            nonce.clone(),
            "--out".into(),
            raw_path.to_string_lossy().into_owned(),
        ]);
        let options = match parse(&run_args) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error),
        };
        let started = Instant::now();
        let (runner_value, exit_code) = evaluate(&options);
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
        let marker_check = validate_markers(&uart_bytes, &nonce, &cli.expected_s5_marker_keys);
        let hardware_status = runner_value["status"]
            .as_str()
            .unwrap_or("invalid_runner_json")
            .to_string();
        let guest_started = runner_value["guest_started"].as_bool().unwrap_or(false);
        let stop = runner_value.get("stop").cloned();
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
            run_profile: RUN_PROFILE.to_string(),
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
            host: HostEvidence {
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                runner_version: env!("CARGO_PKG_VERSION").to_string(),
                source_revision: option_env!("OD_SOURCE_REVISION").map(str::to_string),
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
        run_profile: RUN_PROFILE.to_string(),
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
    println!("{}", String::from_utf8_lossy(&manifest_bytes));
    match outcome {
        ScenarioStatus::Passed => ExitCode::SUCCESS,
        ScenarioStatus::Failed => ExitCode::from(1),
        ScenarioStatus::NotRun | ScenarioStatus::Blocked => ExitCode::from(78),
    }
}

fn parse_series_args(args: &[String]) -> Result<SeriesCli, String> {
    let mut requested_runs = None;
    let mut series_id = None;
    let mut scenario_id = None;
    let mut expected_stop_kind = None;
    let mut out = None;
    let mut expected_s5_marker_keys = Vec::new();
    let mut forwarded = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if matches!(name, "--run" | "--set-cpl0" | "--diagnostic-past-c7") {
            forwarded.push(name.to_string());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("{name} requires a value"))?;
        match name {
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
        let line = raw.strip_suffix('\n').unwrap_or(raw);
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
    json!({
        "cmdline": runner["cmdline"],
        "ram_mib": runner["ram_mib"],
        "budget": runner["stop"]["budget"],
        "cpu_path": runner["stop"]["path"],
        "cpu_profile": runner["stop"]["cpu_profile"],
        "diagnostic_past_c7": runner["diagnostic_past_c7"],
        "acceptance": runner["acceptance"],
    })
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
    if manifest.schema_version != SERIES_SCHEMA_VERSION
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

fn load_series_runs(
    series_path: &Path,
    series: &SeriesManifest,
) -> Result<Vec<RunManifest>, String> {
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
    let manifest: RunManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid run manifest JSON: {error}"))?;
    if manifest.schema_version != SERIES_SCHEMA_VERSION || manifest.run_id != reference.run_id {
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
    if raw_value["status"].as_str() != Some(manifest.hardware_status.as_str())
        || raw_value["nonce"].as_str() != Some(manifest.nonce.as_str())
        || raw_value["guest_started"].as_bool() != Some(manifest.runner.guest_started)
        || raw_value["retired"].as_u64() != manifest.runner.retired
        || raw_value.get("stop") != manifest.runner.stop.as_ref()
    {
        return Err("run manifest runner summary does not match raw JSON".into());
    }
    if manifest.configuration != configuration_from_runner(&raw_value)
        || manifest.artifact_hashes != artifact_hashes(&raw_value)
    {
        return Err("run manifest configuration or artifact hashes do not match raw JSON".into());
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
    let checked = validate_markers(
        &uart,
        &manifest.nonce,
        &manifest.guest_markers.expected_s5_marker_keys,
    );
    if checked.evidence != manifest.guest_markers {
        return Err("recorded guest marker validation does not match UART bytes".into());
    }
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
    let left_config = normalize_nonce(&left.configuration, &left.nonce);
    let right_config = normalize_nonce(&right.configuration, &right.nonce);
    if left_config != right_config {
        unexplained.push(format!("{prefix}.configuration differs"));
    }
    let left_stop = normalize_nonce(&without_uart(left.runner.stop.as_ref()), &left.nonce);
    let right_stop = normalize_nonce(&without_uart(right.runner.stop.as_ref()), &right.nonce);
    if left_stop != right_stop {
        unexplained.push(format!("{prefix}.machine_observations differs"));
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
    if normalize_uart(&left_uart, &left.nonce) != normalize_uart(&right_uart, &right.nonce) {
        unexplained.push(format!(
            "{prefix}.ordered_uart_bytes differs after nonce normalization"
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

fn without_uart(stop: Option<&Value>) -> Value {
    let mut value = stop.cloned().unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("com1_hex");
        object.remove("com1_text");
    }
    value
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
        eprintln!("invalid input: --compare-results A.json B.json --linux-stage5 --json");
        return ExitCode::from(3);
    }
    let paths = args
        .iter()
        .filter(|argument| !argument.starts_with("--"))
        .collect::<Vec<_>>();
    if paths.len() != 2 {
        eprintln!("invalid input: --linux-stage5 comparison requires two series.json paths");
        return ExitCode::from(3);
    }
    let report = match compare_series_paths(Path::new(paths[0]), Path::new(paths[1])) {
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
            b"OD_NONCE=runNonce001\nOD_S5_A_PASS nonce=runNonce001 result=ok\nOD_S5_A_FILE nonce=runNonce001 path=/tmp/a sha256=abc\n".as_slice(),
        ] {
            assert!(!validate_markers(bad, "runNonce001", &expected).evidence.valid);
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
        assert!(parse_series_args(&base[2..]).is_err());

        let mut wrong_scenario = base.clone();
        let marker = wrong_scenario
            .iter()
            .position(|argument| argument == "A_FILE")
            .unwrap();
        wrong_scenario[marker] = "B_FILE".into();
        assert!(parse_series_args(&wrong_scenario).is_err());
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
