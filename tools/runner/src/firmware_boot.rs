//! `boot-firmware`: start a firmware image on firmware board v1 (S7-03).
//!
//! The runner only reads the image, calls `od_machine::load_firmware_image`,
//! drives `run_boot` and reports. Loading, quanta, COM1 draining and virtual
//! time (`retired * 1 ns` per quantum) live in `od-machine`
//! (`docs/specs/FIRMWARE_BOOT.md` sections 8 and 11).

use od_machine::{FIRMWARE_IMAGE_BYTES, FirmwareBoardConfig, PagedProbeExit, load_firmware_image};
use od_testkit::sha256_hex;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const USAGE: &str = "boot-firmware --image <bin> --com1-out <file> --budget <steps>";

struct Options {
    image: String,
    com1_out: String,
    budget: u64,
}

pub fn run(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(options) => options,
        Err(detail) => return emit(failure("invalid_input", "arguments", detail), 3),
    };
    let mut image = Vec::with_capacity(FIRMWARE_IMAGE_BYTES + 1);
    let read = File::open(&options.image).and_then(|file| {
        file.take(FIRMWARE_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut image)
    });
    if let Err(error) = read {
        return emit(failure("invalid_input", "image_file", error.to_string()), 3);
    }
    let mut board = match load_firmware_image(&image, &FirmwareBoardConfig::default()) {
        Ok(board) => board,
        Err(error) => return emit(failure("loader_error", "loader", format!("{error:?}")), 4),
    };
    let handoff_sha256 = sha256_hex(board.handoff_block());
    let handoff_bytes = board.handoff_block().len();
    let report = match board.run_boot(options.budget) {
        Ok(report) => report,
        Err(error) => {
            // Keep the bytes the guest already wrote (even an empty transcript).
            let mut value = failure("run_error", "run", format!("{error:?}"));
            if let Err(write_error) = std::fs::write(&options.com1_out, board.com1_transcript()) {
                value["com1_out_error"] = json!(write_error.to_string());
            }
            value["com1_bytes"] = json!(board.com1_transcript().len());
            value["com1_out"] = json!(options.com1_out);
            return emit(value, 5);
        }
    };
    if let Err(error) = std::fs::write(&options.com1_out, &report.com1) {
        return emit(failure("invalid_input", "com1_out", error.to_string()), 3);
    }
    let fw_stop = parse_fw_stop(&report.com1);
    let exit = match report.exit {
        PagedProbeExit::Halted => "halted",
        PagedProbeExit::Shutdown => "shutdown",
        PagedProbeExit::BudgetExhausted => "budget_exhausted",
        PagedProbeExit::Stopped => "stopped",
        PagedProbeExit::Rejected(_) => "rejected",
        PagedProbeExit::Unsupported { .. } => "unsupported",
        PagedProbeExit::Fault(_) => "fault",
        PagedProbeExit::Backend(_) => "backend_error",
        PagedProbeExit::PortIo(_) => "pio_error",
        PagedProbeExit::IntegrationError(_) => "integration_error",
    };
    let halted = matches!(report.exit, PagedProbeExit::Halted);
    let (status, code) = match (halted, &fw_stop) {
        (true, None) => ("completed", 0),
        (true, Some(_)) => ("firmware_stop", 6),
        (false, _) => ("error", 5),
    };
    let state = board.state();
    let result = json!({
        "schema_version": 1,
        "command": "boot-firmware",
        "status": status,
        "exit": exit,
        "exit_detail": if halted { Value::Null } else { json!(format!("{:?}", report.exit)) },
        "retired": report.retired,
        "quanta": report.quanta,
        "rip": format!("0x{:016x}", state.rip),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "budget": options.budget,
        "tsc_hz": report.tsc_hz,
        "time_step_ns_per_instruction": 1,
        "virtual_ns": report.virtual_ns,
        "image_sha256": sha256_hex(&image),
        "handoff_sha256": handoff_sha256,
        "handoff_bytes": handoff_bytes,
        "com1_bytes": report.com1.len(),
        "com1_out": options.com1_out,
        "fw_stop": fw_stop.map(|(code, detail)| json!({
            "code": format!("0x{code:02x}"),
            "detail": format!("0x{detail:016x}"),
        })),
        "page_fault": report.page_fault.map(|fault| format!("{fault:?}")),
        "memory_error": report.memory_error.map(|error| format!("{error:?}")),
    });
    emit(result, code)
}

fn parse(args: &[String]) -> Result<Options, String> {
    let (mut image, mut com1_out, mut budget) = (None, None, None);
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {name}; usage: {USAGE}"))?;
        match name {
            "--image" if image.is_none() => image = Some(value.clone()),
            "--com1-out" if com1_out.is_none() => com1_out = Some(value.clone()),
            "--budget" if budget.is_none() => {
                budget = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --budget '{value}'"))?,
                );
            }
            other => {
                return Err(format!(
                    "unexpected or duplicate option {other}; usage: {USAGE}"
                ));
            }
        }
        index += 2;
    }
    Ok(Options {
        image: image.ok_or_else(|| format!("--image is required; usage: {USAGE}"))?,
        com1_out: com1_out.ok_or_else(|| format!("--com1-out is required; usage: {USAGE}"))?,
        budget: budget.ok_or_else(|| format!("--budget is required; usage: {USAGE}"))?,
    })
}

/// `(code, detail)` of the `OD-FW STOP code=0xNN detail=0xDDDDDDDDDDDDDDDD`
/// line, but only when it is the LAST non-empty line of the COM1 transcript
/// (FIRMWARE_BOOT.md section 12: the runner parses the last line). A STOP line
/// followed by more text is not a STOP.
fn parse_fw_stop(com1: &[u8]) -> Option<(u8, u64)> {
    let text = String::from_utf8_lossy(com1);
    let line = text.lines().rev().find(|line| !line.is_empty())?;
    let rest = line.strip_prefix("OD-FW STOP code=0x")?;
    let (code, detail) = rest.split_once(" detail=0x")?;
    if code.len() != 2 || detail.len() != 16 {
        return None;
    }
    Some((
        u8::from_str_radix(code, 16).ok()?,
        u64::from_str_radix(detail, 16).ok()?,
    ))
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema_version": 1,
        "command": "boot-firmware",
        "status": status,
        "exit": status,
        "error": {"kind": kind, "detail": detail},
    })
}

fn emit(value: Value, code: u8) -> ExitCode {
    println!("{value}");
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn fw_stop_is_recognised_only_as_the_last_non_empty_line() {
        // Earlier STOP lines do not count; only the last line is parsed.
        let transcript = b"boot\r\nOD-FW STOP code=0x02 detail=0x0000000000000000\r\n\
            OD-FW STOP code=0x10 detail=0x000000000000000e\r\n";
        assert_eq!(parse_fw_stop(transcript), Some((0x10, 0xe)));
        // Trailing blank lines are ignored.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x10 detail=0x000000000000000e\r\n\r\n\r\n"),
            Some((0x10, 0xe))
        );
        // A STOP followed by more text is NOT a STOP.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x10 detail=0x000000000000000e\r\nmore\r\n"),
            None
        );
        assert_eq!(
            parse_fw_stop(
                b"OD-FW STOP code=0x10 detail=0x000000000000000e\r\n\
                  OD-FW STOP code=0x10 detail=0x000000000000000e\r\nx"
            ),
            None
        );
        assert_eq!(parse_fw_stop(b"no stop here\r\n"), None);
        assert_eq!(parse_fw_stop(b""), None);
        // Malformed widths and prefixes are not a STOP line.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x1 detail=0x0000000000000000\r\n"),
            None
        );
        assert_eq!(parse_fw_stop(b"OD-FW STOP code=0x10 detail=0x1\r\n"), None);
        assert_eq!(
            parse_fw_stop(b"xOD-FW STOP code=0x10 detail=0x0000000000000000\r\n"),
            None
        );
        // A STOP line without the trailing newline still counts.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x30 detail=0xffffffffffffffff"),
            Some((0x30, u64::MAX))
        );
    }

    #[test]
    fn arguments_are_strict() {
        let ok = parse(&args(&["--image", "a", "--com1-out", "b", "--budget", "7"])).unwrap();
        assert_eq!(
            (ok.image.as_str(), ok.com1_out.as_str(), ok.budget),
            ("a", "b", 7)
        );
        for bad in [
            args(&[]),
            args(&["--image", "a", "--com1-out", "b"]),
            args(&["--image", "a", "--com1-out", "b", "--budget", "x"]),
            args(&[
                "--image",
                "a",
                "--image",
                "c",
                "--com1-out",
                "b",
                "--budget",
                "1",
            ]),
            args(&["--image", "a", "--com1-out", "b", "--budget"]),
            args(&[
                "--image",
                "a",
                "--com1-out",
                "b",
                "--budget",
                "1",
                "--extra",
                "2",
            ]),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }
}
