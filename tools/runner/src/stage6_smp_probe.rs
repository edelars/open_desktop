//! `--stage6-smp-probe` (S6-16f): the `Stage6SmpProbeV2` guest (INIT/SIPI start of an application
//! processor, Fixed IPI exchange, negative cases) on the board with the scheduler of S6-15.
//!
//! ```text
//! od-runner --stage6-smp-probe IMAGE --nonce TOKEN [--vcpus N] [--scenario NAME]
//!           [--quantum Q | --seed S | --schedule CPU:STEPS,...] [--budget B] [--record-schedule]
//! od-runner --stage6-smp-probe IMAGE --nonce TOKEN --parallel [--vcpus N] [--quantum Q]
//!           [--budget B] [--timeout-s N]
//! ```
//!
//! `--parallel` (S6-34) runs the same guest in the parallel mode instead of the interleaving
//! scheduler: one host thread per vCPU over the shared machine, `Q` steps per call of the
//! interpreter loop (default 1024), `--timeout-s` seconds (default 120) before the run is ended
//! and reported as `timeout`. The interleaving mode stays the default. The result is not a
//! function of the arguments (the threads race: the schedule is not recorded and `--seed`,
//! `--schedule` and `--record-schedule` are refused); what is checked is the result of the guest.
//!
//! `IMAGE` is `probe.bin` of `tools/build/build_stage6_smp_probe.py`. The scenario is chosen when
//! the image is built; `--scenario` only declares which one the caller expects, and it is checked
//! against `build_manifest.json` beside the image (a mismatch is an input error). The nonce is
//! written by the loader into the info block, never into the image.
//!
//! The policy of the quantum is one of: `--quantum Q` (every turn Q steps, the default is 1024),
//! `--seed S` (every turn drawn from SplitMix64(S) in 1..=256 steps) or `--schedule`
//! (`CPU:STEPS` runs, one step of the named vCPU per entry; the vCPU must be running when its
//! entry comes, which is why a schedule is normally the `schedule` of an earlier
//! `--record-schedule` run). The result is one JSON object on stdout: retired instructions, turns,
//! INIT/SIPI counters and activity of every vCPU, the COM1 bytes, the cells of the protocol, the
//! vectors still set in each Local APIC, the reason the run ended, the quantum policy and the
//! SHA-256 of the image. The same arguments give the same bytes.

use od_machine::{
    STAGE6_SMP_PROBE_AP_RX_GPA, STAGE6_SMP_PROBE_BSP_RX_GPA, STAGE6_SMP_PROBE_COUNT_GPA,
    STAGE6_SMP_PROBE_FLAG_GPA, STAGE6_SMP_PROBE_IMAGE_BYTES, STAGE6_SMP_PROBE_LOCK_GPA,
    STAGE6_SMP_PROBE_NMI_GPA, STAGE6_SMP_PROBE_SD_ACK_GPA, STAGE6_SMP_PROBE_SD_POST_GPA,
    STAGE6_SMP_PROBE_SD_SEEN_HANDLER_GPA, STAGE6_SMP_PROBE_SD_SEEN_MAIN_GPA, Stage6SmpProbe,
    Stage6SmpProbeCpu, Stage6SmpProbeFinish, Stage6SmpProbeParallelFinish, Stage6SmpProbeQuantum,
    Stage6SmpProbeRunError, load_stage6_smp_probe,
};
use od_testkit::sha256_hex;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

const PROFILE: &str = "Stage6SmpProbeV2";
const DEFAULT_BUDGET: u64 = 50_000_000;
const MAX_BUDGET: u64 = 4_000_000_000;
const SCHEDULE_MAX_STEPS: u64 = 10_000_000;
const DEFAULT_QUANTUM: u64 = 1024;
const SCENARIOS: [&str; 6] = [
    "normal",
    "exchange",
    "resipi",
    "bad-ipi",
    "nmi",
    "shootdown",
];
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;

struct Options {
    path: String,
    vcpus: usize,
    nonce: String,
    scenario: Option<String>,
    quantum: Stage6SmpProbeQuantum,
    budget: u64,
    record_schedule: bool,
    parallel: bool,
    timeout_s: u64,
}

pub fn run(args: &[String]) -> ExitCode {
    let (result, code) = match run_inner(args) {
        Ok(result) | Err(result) => result,
    };
    println!("{result}");
    ExitCode::from(code)
}

fn parse_number(value: &str) -> Result<u64, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse::<u64>()
    }
    .map_err(|_| format!("invalid number '{value}'"))
}

fn parse_schedule(value: &str) -> Result<Vec<usize>, String> {
    let mut list = Vec::new();
    for run in value.split(',') {
        let (cpu, steps) = run
            .split_once(':')
            .ok_or_else(|| format!("--schedule expects CPU:STEPS runs, got '{run}'"))?;
        let cpu = usize::try_from(parse_number(cpu)?)
            .map_err(|_| format!("vCPU out of range '{cpu}'"))?;
        let steps = parse_number(steps)?;
        // The list holds one entry per step (8 bytes each): at most SCHEDULE_MAX_STEPS of them, so
        // 80 MB. The sum is checked without overflow (`steps` is any u64).
        let fits = steps <= SCHEDULE_MAX_STEPS - list.len() as u64;
        if cpu >= 4 || steps == 0 || !fits {
            return Err(format!(
                "--schedule run '{run}': vCPU 0..=3, steps 1.., at most {SCHEDULE_MAX_STEPS} in all"
            ));
        }
        list.extend(std::iter::repeat_n(cpu, steps as usize));
    }
    Ok(list)
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut path = None;
    let mut vcpus = None;
    let mut nonce = None;
    let mut scenario = None;
    let mut quantum: Option<Stage6SmpProbeQuantum> = None;
    let mut budget = None;
    let mut record_schedule = false;
    let mut parallel = false;
    let mut timeout_s = None;
    let mut index = 0;
    let set_quantum = |slot: &mut Option<Stage6SmpProbeQuantum>, value: Stage6SmpProbeQuantum| {
        if slot.replace(value).is_some() {
            Err("only one of --quantum, --seed, --schedule is accepted".to_string())
        } else {
            Ok(())
        }
    };
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--record-schedule" {
            record_schedule = true;
            index += 1;
            continue;
        }
        if option == "--parallel" {
            parallel = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage6-smp-probe" if path.is_none() => path = Some(value.clone()),
            "--stage6-smp-probe" => return Err("probe may be specified only once".into()),
            "--vcpus" if vcpus.is_none() => {
                let parsed = parse_number(value)?;
                if !(1..=4).contains(&parsed) {
                    return Err("--vcpus must be between 1 and 4".into());
                }
                vcpus = Some(parsed as usize);
            }
            "--nonce" if nonce.is_none() => nonce = Some(value.clone()),
            "--scenario" if scenario.is_none() => {
                if !SCENARIOS.contains(&value.as_str()) {
                    return Err(format!("--scenario must be one of {SCENARIOS:?}"));
                }
                scenario = Some(value.clone());
            }
            "--quantum" => {
                let steps = parse_number(value)?;
                if steps == 0 {
                    return Err("--quantum must be at least 1".into());
                }
                set_quantum(&mut quantum, Stage6SmpProbeQuantum::Fixed(steps))?;
            }
            "--seed" => set_quantum(
                &mut quantum,
                Stage6SmpProbeQuantum::Seeded(parse_number(value)?),
            )?,
            "--schedule" => set_quantum(
                &mut quantum,
                Stage6SmpProbeQuantum::Schedule(parse_schedule(value)?),
            )?,
            "--budget" if budget.is_none() => {
                let parsed = parse_number(value)?;
                if parsed == 0 || parsed > MAX_BUDGET {
                    return Err(format!("--budget must be between 1 and {MAX_BUDGET}"));
                }
                budget = Some(parsed);
            }
            "--timeout-s" if timeout_s.is_none() => {
                let parsed = parse_number(value)?;
                if parsed == 0 || parsed > 3600 {
                    return Err("--timeout-s must be between 1 and 3600".into());
                }
                timeout_s = Some(parsed);
            }
            "--vcpus" | "--nonce" | "--scenario" | "--budget" | "--timeout-s" => {
                return Err(format!("{option} may be specified only once"));
            }
            other => return Err(format!("unexpected option '{other}'")),
        }
        index += 2;
    }
    if parallel
        && (record_schedule
            || matches!(
                quantum,
                Some(Stage6SmpProbeQuantum::Seeded(_) | Stage6SmpProbeQuantum::Schedule(_))
            ))
    {
        return Err(
            "--parallel takes --quantum only (no --seed, --schedule, --record-schedule)".into(),
        );
    }
    if !parallel && timeout_s.is_some() {
        return Err("--timeout-s needs --parallel".into());
    }
    Ok(Options {
        path: path.ok_or_else(|| "--stage6-smp-probe IMAGE is required".to_string())?,
        vcpus: vcpus.unwrap_or(2),
        nonce: nonce.ok_or_else(|| "--nonce is required".to_string())?,
        scenario,
        quantum: quantum.unwrap_or(Stage6SmpProbeQuantum::Fixed(DEFAULT_QUANTUM)),
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        record_schedule,
        parallel,
        timeout_s: timeout_s.unwrap_or(120),
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(STAGE6_SMP_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE6_SMP_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != STAGE6_SMP_PROBE_IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {STAGE6_SMP_PROBE_IMAGE_BYTES} bytes; read {}",
            image.len()
        ));
    }
    Ok(image)
}

/// The scenario recorded by the builder beside the image, if the manifest is there.
fn manifest_scenario(image_path: &str) -> Result<Option<String>, String> {
    let manifest = Path::new(image_path).with_file_name("build_manifest.json");
    let Ok(bytes) = std::fs::read(&manifest) else {
        return Ok(None);
    };
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not parse '{}': {error}", manifest.display()))?;
    if value["profile"] != PROFILE {
        return Err(format!(
            "'{}' is not a {PROFILE} manifest",
            manifest.display()
        ));
    }
    Ok(value["scenario"].as_str().map(str::to_string))
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": status,
        "error": {"kind": kind, "detail": detail},
    })
}

fn quantum_json(quantum: &Stage6SmpProbeQuantum) -> Value {
    match quantum {
        Stage6SmpProbeQuantum::Fixed(steps) => json!({"policy": "fixed", "quantum": steps}),
        Stage6SmpProbeQuantum::Seeded(seed) => {
            json!({"policy": "seeded", "seed": seed, "min": 1, "max": 256})
        }
        Stage6SmpProbeQuantum::Schedule(list) => {
            json!({"policy": "schedule", "steps": list.len()})
        }
    }
}

fn cell(probe: &mut Stage6SmpProbe, gpa: u64) -> u32 {
    u32::from_le_bytes(probe.read_ram(gpa, 4).try_into().expect("four bytes"))
}

fn finished_name(finished: &Stage6SmpProbeFinish) -> &'static str {
    match finished {
        Stage6SmpProbeFinish::AllIdle => "all_idle",
        Stage6SmpProbeFinish::BudgetExhausted => "budget",
        Stage6SmpProbeFinish::Stopped => "stopped",
        Stage6SmpProbeFinish::ScheduleEnd => "schedule_end",
    }
}

fn per_cpu(cpus: &[Stage6SmpProbeCpu], field: impl Fn(&Stage6SmpProbeCpu) -> u64) -> Vec<u64> {
    cpus.iter().map(field).collect()
}

fn run_inner(args: &[String]) -> Result<(Value, u8), (Value, u8)> {
    let invalid = |detail: String| {
        (
            failure("invalid_input", "arguments", detail),
            EXIT_INVALID_INPUT,
        )
    };
    let options = parse_args(args).map_err(invalid)?;
    let image = read_image(&options.path)
        .map_err(|detail| (failure("invalid_input", "file", detail), EXIT_INVALID_INPUT))?;
    let built = manifest_scenario(&options.path).map_err(invalid)?;
    if let (Some(declared), Some(built)) = (&options.scenario, &built)
        && declared != built
    {
        return Err(invalid(format!(
            "--scenario {declared} does not match the image, which was built as {built}"
        )));
    }
    let scenario = options.scenario.clone().or(built);
    let mut probe = load_stage6_smp_probe(&image, options.vcpus, options.nonce.as_bytes())
        .map_err(|error| {
            (
                failure("loader_error", "loader", format!("{error:?}")),
                EXIT_LOADER,
            )
        })?;
    if options.parallel {
        return run_parallel(probe, &options, &image, scenario);
    }
    let outcome = if options.record_schedule {
        probe.run_recorded(options.quantum.clone(), options.budget)
    } else {
        probe.run(options.quantum.clone(), options.budget)
    };

    // The parts every outcome has: what ran, the COM1 bytes, the cells, the Local APIC vectors.
    let (status, exit, error, cpus, com1, extra) = match outcome {
        Ok(report) => {
            // An explicit schedule that ran to its end is the requested outcome; a budget or a
            // STOP is not.
            let status = match report.finished {
                Stage6SmpProbeFinish::AllIdle => "guest_idle",
                Stage6SmpProbeFinish::ScheduleEnd => "schedule_end",
                _ => "work_limit",
            };
            (
                status,
                finished_name(&report.finished),
                Value::Null,
                report.cpus,
                report.com1,
                json!({
                    "turns": report.turns,
                    "idle_ns": report.idle_ns,
                    "idle_jumps": report.idle_jumps,
                    "schedule": report.schedule.map(|runs| {
                        runs.iter().map(|(cpu, steps)| json!([cpu, steps])).collect::<Vec<_>>()
                    }),
                }),
            )
        }
        Err(Stage6SmpProbeRunError::Board { error, com1, cpus }) => {
            let turns: u64 = cpus.iter().map(|cpu| cpu.turns).sum();
            (
                "guest_error",
                "board_error",
                json!({"kind": "board_error", "detail": format!("{error:?}")}),
                cpus,
                com1,
                json!({"turns": turns}),
            )
        }
        Err(Stage6SmpProbeRunError::Scheduler(detail)) => {
            return Err((failure("scheduler_error", "scheduler", detail), EXIT_GUEST));
        }
    };
    let cells = json!({
        "lock_entered": cell(&mut probe, STAGE6_SMP_PROBE_LOCK_GPA),
        "flag_ap_up": cell(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA),
        "start_count": cell(&mut probe, STAGE6_SMP_PROBE_COUNT_GPA),
        "ap_rx": cell(&mut probe, STAGE6_SMP_PROBE_AP_RX_GPA),
        "bsp_rx": cell(&mut probe, STAGE6_SMP_PROBE_BSP_RX_GPA),
        "nmi_cell": cell(&mut probe, STAGE6_SMP_PROBE_NMI_GPA),
        "sd_ack": cell(&mut probe, STAGE6_SMP_PROBE_SD_ACK_GPA),
        "sd_post": cell(&mut probe, STAGE6_SMP_PROBE_SD_POST_GPA),
        "sd_seen_handler": cell(&mut probe, STAGE6_SMP_PROBE_SD_SEEN_HANDLER_GPA),
        "sd_seen_main": cell(&mut probe, STAGE6_SMP_PROBE_SD_SEEN_MAIN_GPA),
    });
    let lapic_vectors: Vec<Value> = (0..options.vcpus)
        .map(|cpu| {
            Value::Array(
                probe
                    .pending_vectors(cpu)
                    .into_iter()
                    .map(|(vector, irr, isr)| json!({"vector": vector, "irr": irr, "isr": isr}))
                    .collect(),
            )
        })
        .collect();
    let mut result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": exit,
        "scenario": scenario,
        "vcpus": options.vcpus,
        "nonce": options.nonce,
        "image": {"bytes": image.len(), "sha256": sha256_hex(&image)},
        "quantum_policy": quantum_json(&options.quantum),
        "budget": options.budget,
        "retired": per_cpu(&cpus, |cpu| cpu.retired),
        "retired_total": cpus.iter().map(|cpu| cpu.retired).sum::<u64>(),
        "turns_per_cpu": per_cpu(&cpus, |cpu| cpu.turns),
        "steps": per_cpu(&cpus, |cpu| cpu.steps),
        "wakes": per_cpu(&cpus, |cpu| cpu.wakes),
        "init_applied": per_cpu(&cpus, |cpu| cpu.init_applied),
        "started": per_cpu(&cpus, |cpu| cpu.started),
        "nmi_delivered": per_cpu(&cpus, |cpu| cpu.nmi_delivered),
        "activity": cpus.iter().map(|cpu| format!("{:?}", cpu.activity)).collect::<Vec<_>>(),
        "legacy": cpus.iter().map(|cpu| cpu.legacy).collect::<Vec<_>>(),
        "com1_text": String::from_utf8_lossy(&com1),
        "com1_hex": com1.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        "cells": cells,
        "lapic_vectors": lapic_vectors,
        "error": error,
    });
    if let (Some(object), Some(extra)) = (result.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    if status == "guest_idle" || status == "schedule_end" {
        Ok((result, 0))
    } else {
        Err((result, EXIT_GUEST))
    }
}

/// The parallel mode (S6-34): the same cells and vectors as the scheduler's result, the counters
/// the threads keep (calls of the interpreter loop, sleeps, split `LOCK` operations), and the reason
/// the run ended.
fn run_parallel(
    mut probe: Stage6SmpProbe,
    options: &Options,
    image: &[u8],
    scenario: Option<String>,
) -> Result<(Value, u8), (Value, u8)> {
    let steps = match options.quantum {
        Stage6SmpProbeQuantum::Fixed(steps) => steps,
        _ => unreachable!("checked in parse_args"),
    };
    let report = match probe.run_parallel(
        steps,
        options.budget,
        std::time::Duration::from_secs(options.timeout_s),
    ) {
        Ok(report) => report,
        Err(error) => {
            return Err((
                failure("scheduler_error", "parallel", format!("{error:?}")),
                EXIT_GUEST,
            ));
        }
    };
    let (status, exit, error) = match &report.finished {
        Stage6SmpProbeParallelFinish::AllIdle => ("guest_idle", "all_idle", Value::Null),
        Stage6SmpProbeParallelFinish::BudgetExhausted => ("work_limit", "budget", Value::Null),
        Stage6SmpProbeParallelFinish::GuestStop(od_machine::MachineStopReason::Reset) => {
            ("guest_stop", "acpi_reset", Value::Null)
        }
        Stage6SmpProbeParallelFinish::GuestStop(od_machine::MachineStopReason::Shutdown) => {
            ("guest_stop", "acpi_shutdown", Value::Null)
        }
        Stage6SmpProbeParallelFinish::Timeout => ("work_limit", "timeout", Value::Null),
        Stage6SmpProbeParallelFinish::Error { cpu, error } => (
            "guest_error",
            "board_error",
            json!({"kind": "board_error", "cpu": cpu, "detail": error}),
        ),
        Stage6SmpProbeParallelFinish::Panicked { cpu } => (
            "guest_error",
            "panic",
            json!({"kind": "panic", "cpu": cpu, "detail": "the thread of the vCPU panicked"}),
        ),
    };
    let cells = json!({
        "lock_entered": cell(&mut probe, STAGE6_SMP_PROBE_LOCK_GPA),
        "flag_ap_up": cell(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA),
        "start_count": cell(&mut probe, STAGE6_SMP_PROBE_COUNT_GPA),
        "ap_rx": cell(&mut probe, STAGE6_SMP_PROBE_AP_RX_GPA),
        "bsp_rx": cell(&mut probe, STAGE6_SMP_PROBE_BSP_RX_GPA),
        "nmi_cell": cell(&mut probe, STAGE6_SMP_PROBE_NMI_GPA),
        "sd_ack": cell(&mut probe, STAGE6_SMP_PROBE_SD_ACK_GPA),
        "sd_post": cell(&mut probe, STAGE6_SMP_PROBE_SD_POST_GPA),
        "sd_seen_handler": cell(&mut probe, STAGE6_SMP_PROBE_SD_SEEN_HANDLER_GPA),
        "sd_seen_main": cell(&mut probe, STAGE6_SMP_PROBE_SD_SEEN_MAIN_GPA),
    });
    let lapic_vectors: Vec<Value> = (0..options.vcpus)
        .map(|cpu| {
            Value::Array(
                probe
                    .pending_vectors(cpu)
                    .into_iter()
                    .map(|(vector, irr, isr)| json!({"vector": vector, "irr": irr, "isr": isr}))
                    .collect(),
            )
        })
        .collect();
    let cpus = &report.cpus;
    let result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": exit,
        "mode": "parallel",
        "scenario": scenario,
        "vcpus": options.vcpus,
        "nonce": options.nonce,
        "image": {"bytes": image.len(), "sha256": sha256_hex(image)},
        "quantum": steps,
        "budget": options.budget,
        "timeout_s": options.timeout_s,
        "retired": cpus.iter().map(|cpu| cpu.retired).collect::<Vec<_>>(),
        "retired_total": cpus.iter().map(|cpu| cpu.retired).sum::<u64>(),
        "calls": cpus.iter().map(|cpu| cpu.calls).collect::<Vec<_>>(),
        "wakes": cpus.iter().map(|cpu| cpu.wakes).collect::<Vec<_>>(),
        "sleeps": cpus.iter().map(|cpu| cpu.sleeps).collect::<Vec<_>>(),
        "split_locks": cpus.iter().map(|cpu| cpu.split_locks).collect::<Vec<_>>(),
        "init_applied": cpus.iter().map(|cpu| cpu.init_applied).collect::<Vec<_>>(),
        "started": cpus.iter().map(|cpu| cpu.started).collect::<Vec<_>>(),
        "nmi_delivered": cpus.iter().map(|cpu| cpu.nmi_delivered).collect::<Vec<_>>(),
        "activity": cpus.iter().map(|cpu| format!("{:?}", cpu.activity)).collect::<Vec<_>>(),
        "legacy": cpus.iter().map(|cpu| cpu.legacy).collect::<Vec<_>>(),
        "idle_ns": report.idle_ns,
        "idle_jumps": report.idle_jumps,
        "com1_text": String::from_utf8_lossy(&report.com1),
        "com1_hex": report.com1.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        "cells": cells,
        "lapic_vectors": lapic_vectors,
        "error": error,
    });
    #[cfg(feature = "lock-wait-metrics")]
    let result = {
        let mut result = result;
        let metrics = report.lock_wait_metrics;
        let acquisition = |m: od_machine::MutexAcquisitionMetrics| {
            json!({
                "acquisition_count": m.acquisition_count,
                "acquisition_wall_ns": m.acquisition_wall_ns,
                "max_acquisition_wall_ns": m.max_acquisition_wall_ns,
            })
        };
        result["lock_wait_metrics"] = json!({
            "clock": "host_monotonic",
            "device_table": acquisition(metrics.device_table),
            "apic_pair": acquisition(metrics.apic_pair),
        });
        result
    };
    if status == "guest_idle" || status == "guest_stop" {
        Ok((result, 0))
    } else {
        Err((result, EXIT_GUEST))
    }
}
