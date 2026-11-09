//! `--stage4-pit-calibrate-probe` (S4-29d): guest TSC calibration on the 8254
//! channel 2 in virtual time, with the port tick of port `0x61`.
//!
//! `--stage4-pit-calibrate-probe IMAGE [--budget N] [--inject-no-port-tick]`.
//! The injection makes the session owner drop the port tick: the polling loop
//! of the guest then never sees time move and the run ends with
//! `budget_exhausted` / status `work_limit`.

use od_machine::{
    STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES, STAGE4_PIT_CALIBRATE_PROBE_TSC_HZ,
    Stage4PitCalibrateExit, Stage4PitCalibrateInjection, load_stage4_pit_calibrate_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4PitCalibrateProbeV1";
/// The accepted profile needs about 167 thousand steps.
const DEFAULT_BUDGET: u64 = 400_000;
const MAX_BUDGET: u64 = 2_000_000;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;

struct Options {
    path: String,
    budget: u64,
    inject_no_port_tick: bool,
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

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut path = None;
    let mut budget = None;
    let mut inject_no_port_tick = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--inject-no-port-tick" {
            if inject_no_port_tick {
                return Err("--inject-no-port-tick may be specified only once".into());
            }
            inject_no_port_tick = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-pit-calibrate-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-pit-calibrate-probe" => {
                return Err("probe may be specified only once".into());
            }
            "--budget" if budget.is_none() => {
                let parsed = parse_number(value)?;
                if parsed > MAX_BUDGET {
                    return Err(format!("--budget must be between 0 and {MAX_BUDGET}"));
                }
                budget = Some(parsed);
            }
            "--budget" => return Err("--budget may be specified only once".into()),
            other => return Err(format!("unexpected option '{other}'")),
        }
        index += 2;
    }
    Ok(Options {
        path: path.ok_or_else(|| "--stage4-pit-calibrate-probe IMAGE is required".to_string())?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        inject_no_port_tick,
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES} bytes; read {}",
            image.len()
        ));
    }
    Ok(image)
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn run_inner(args: &[String]) -> Result<(Value, u8), (Value, u8)> {
    let options = parse_args(args).map_err(|detail| {
        (
            failure("invalid_input", "arguments", detail),
            EXIT_INVALID_INPUT,
        )
    })?;
    let image = read_image(&options.path)
        .map_err(|detail| (failure("invalid_input", "file", detail), EXIT_INVALID_INPUT))?;
    let injection = if options.inject_no_port_tick {
        Stage4PitCalibrateInjection::NoPortTick
    } else {
        Stage4PitCalibrateInjection::None
    };
    let mut probe = load_stage4_pit_calibrate_probe(&image, injection).map_err(|error| {
        (
            failure("loader_error", "loader", format!("{error:?}")),
            EXIT_LOADER,
        )
    })?;
    let report = probe.run(options.budget, || false);
    let record = probe.record();

    let error = match &report.exit {
        Stage4PitCalibrateExit::Halted => Value::Null,
        Stage4PitCalibrateExit::BudgetExhausted => json!({
            "kind": "budget_exhausted",
            "detail": "step budget exhausted while the guest was still polling",
        }),
        Stage4PitCalibrateExit::Stopped => {
            json!({"kind": "stopped", "detail": "probe stop predicate returned true"})
        }
        Stage4PitCalibrateExit::Terminal(cause) => {
            json!({"kind": "guest_terminal", "detail": format!("{cause:?}")})
        }
        Stage4PitCalibrateExit::IntegrationError(detail) => {
            json!({"kind": "integration_error", "detail": detail})
        }
    };
    let status = match (&report.exit, record) {
        (Stage4PitCalibrateExit::Halted, Some(record)) if record.guest_accepted() => {
            "guest_completed"
        }
        (Stage4PitCalibrateExit::Halted, _) => "completion_mismatch",
        (Stage4PitCalibrateExit::BudgetExhausted, _) => "work_limit",
        (Stage4PitCalibrateExit::Stopped, _) => "host_cancelled",
        (Stage4PitCalibrateExit::Terminal(_), _) => "guest_terminal",
        (Stage4PitCalibrateExit::IntegrationError(_), _) => "integration_error",
    };
    let state = probe.state();
    let pit = probe.pit().snapshot();
    let record_json = record.map(|r| {
        json!({
            "status_a": r.status_a,
            "freq_khz_a": r.freq_khz_a,
            "delta_tsc_a": r.delta_tsc_a,
            "pitcnt": r.pitcnt,
            "tscmin": r.tscmin,
            "tscmax": r.tscmax,
            "status_b": r.status_b,
            "freq_khz_b": r.freq_khz_b,
            "delta_tsc_b": r.delta_tsc_b,
            "stage1_passes": r.stage1_passes,
            "stage2_passes": r.stage2_passes,
        })
    });
    let result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": report.exit.name(),
        "budget": options.budget,
        "steps": report.steps,
        "retired": report.retired,
        "clock_ns": probe.clock_ns(),
        "tsc_hz": STAGE4_PIT_CALIBRATE_PROBE_TSC_HZ,
        "port_b_reads": probe.port_b_reads(),
        "inject_no_port_tick": options.inject_no_port_tick,
        "rip": format!("0x{:016x}", state.rip),
        "rflags": format!("0x{:016x}", state.rflags),
        "serial_bytes": probe.serial_bytes(),
        "serial_hex": hex(probe.serial_bytes()),
        "record": record_json,
        "pit": {
            "gate2": pit.gate2,
            "speaker_data": pit.speaker_data,
            "channel2_reload": pit.channel2.run.map(|run| run.reload),
            "channel2_start_tick": pit.channel2.run.map(|run| run.start_tick),
        },
        "error": error,
    });
    let code = match status {
        "guest_completed" => 0,
        _ => EXIT_GUEST,
    };
    if code == 0 {
        Ok((result, code))
    } else {
        Err((result, code))
    }
}
