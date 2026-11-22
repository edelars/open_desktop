//! `display-probe`: run the Stage 10 guest display probe (S10-08) and report.
//!
//! `od-runner display-probe --image IMAGE [--budget N] [--frame-out FILE]`
//!
//! The image is a probe image built by `tools/build/build_stage10_display_probe.py`; it runs
//! on the firmware board (see `od_machine::stage10_display_probe`). One JSON object is printed to
//! stdout. `--frame-out` saves the frame the guest drew (raw, 4 bytes per pixel, rows top to
//! bottom, no padding), read through `DisplayHandle::read_frame_into`.
//!
//! Exit codes: 0 the guest halted and a frame was read; 3 invalid input; 4 loader error;
//! 5 the guest did not halt within the budget, failed, or left no readable frame.

use od_machine::{Stage10ProbeOutcome, load_stage10_display_probe};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::ExitCode;

const DEFAULT_BUDGET: u64 = 10_000_000;

struct Options {
    image: String,
    budget: u64,
    frame_out: Option<String>,
}

pub fn run(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(options) => options,
        Err(detail) => return emit(failure("invalid_input", "arguments", detail), 3),
    };
    if let Some(frame_out) = &options.frame_out
        && same_file(&options.image, frame_out)
    {
        return emit(
            failure(
                "invalid_input",
                "paths",
                "the image and the frame output path must be distinct".to_owned(),
            ),
            3,
        );
    }
    let image = match fs::read(&options.image) {
        Ok(image) => image,
        Err(error) => return emit(failure("invalid_input", "image_file", error.to_string()), 3),
    };
    let mut probe = match load_stage10_display_probe(&image) {
        Ok(probe) => probe,
        Err(error) => return emit(failure("loader_error", "loader", format!("{error:?}")), 4),
    };
    let step = match probe.step(options.budget) {
        Ok(step) => step,
        Err(error) => return emit(failure("loader_error", "run", format!("{error:?}")), 4),
    };
    let report = probe.report();
    let mut frame_saved = false;
    if let (Some(path), Ok((_, frame))) = (&options.frame_out, probe.read_frame()) {
        if let Err(error) = fs::write(path, &frame) {
            return emit(failure("invalid_input", "frame_out", error.to_string()), 3);
        }
        frame_saved = true;
    }
    let halted = step.outcome == Stage10ProbeOutcome::Halted;
    let ok = halted && report.frame_sha256.is_some();
    let result = json!({
        "schema_version": 1,
        "profile": "Stage10DisplayProbeV1",
        "status": if ok { "completed" } else { "error" },
        "outcome": match step.outcome {
            Stage10ProbeOutcome::Halted => "halted",
            Stage10ProbeOutcome::BudgetExhausted => "budget_exhausted",
            Stage10ProbeOutcome::Failed => "failed",
        },
        "retired": report.retired,
        "status_after_commit": format!("0x{:08x}", report.status_after_commit),
        "status_after_frame": format!("0x{:08x}", report.status_after_frame),
        "generation": report.generation,
        "mode_change_count": report.mode_change_count,
        "mode": {
            "width": report.mode.width,
            "height": report.mode.height,
            "stride": report.mode.stride,
            "format": report.mode.format,
        },
        "frame_sha256": report.frame_sha256_hex(),
        "frame_bytes": report.mode.frame_bytes(),
        "frame_out": options.frame_out.as_ref().filter(|_| frame_saved),
        "error": if ok {
            Value::Null
        } else {
            json!({
                "kind": "probe_not_completed",
                "detail": step.failure.clone().or(report.frame_error.clone()),
            })
        },
    });
    emit(result, if ok { 0 } else { 5 })
}

fn same_file(first: &str, second: &str) -> bool {
    if first == second {
        return true;
    }
    match (
        fs::canonicalize(Path::new(first)),
        fs::canonicalize(Path::new(second)),
    ) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut image = None;
    let mut budget = None;
    let mut frame_out = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--image" if image.is_none() => {
                image = Some(args.get(index + 1).ok_or("missing image path")?.clone());
            }
            "--budget" if budget.is_none() => {
                budget = Some(
                    args.get(index + 1)
                        .ok_or("missing budget")?
                        .parse::<u64>()
                        .map_err(|_| "invalid budget")?,
                );
            }
            "--frame-out" if frame_out.is_none() => {
                frame_out = Some(args.get(index + 1).ok_or("missing frame path")?.clone());
            }
            option => return Err(format!("unexpected or duplicate option {option}")),
        }
        index += 2;
    }
    Ok(Options {
        image: image.ok_or("--image IMAGE is required")?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        frame_out,
    })
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema_version": 1,
        "profile": "Stage10DisplayProbeV1",
        "status": status,
        "outcome": null,
        "retired": null,
        "frame_sha256": null,
        "error": {"kind": kind, "detail": detail},
    })
}

fn emit(value: Value, code: u8) -> ExitCode {
    println!("{value}");
    ExitCode::from(code)
}
