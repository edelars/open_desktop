// Diagnostic clock frequency; this does not advertise guest CPUID/MSR timing.
const DIAGNOSTIC_TSC_HZ: u64 = 1_000_000_000;

use od_machine::{
    PagedProbeExit, PagedProbeProfile, PagedVcpuSession, STAGE4_FLAT_PROBE_DATA_GVA,
    STAGE4_FLAT_PROBE_IMAGE_BYTES,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const DEFAULT_BUDGET: u64 = 16;
const MAX_BUDGET: u64 = 1024;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;
const EXIT_PIO: u8 = 6;

pub fn run(args: &[String]) -> ExitCode {
    let (mut result, code) = match run_inner(args) {
        Ok(result) | Err(result) => result,
    };
    if args
        .first()
        .is_some_and(|arg| arg == "--stage4-uart-echo-probe")
    {
        result["profile"] = json!("Stage4UartEchoProbeV1");
    }
    emit(result, code)
}

fn run_inner(args: &[String]) -> Result<(Value, u8), (Value, u8)> {
    let (path, budget, expected, cancel_after, echo, rx) = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(detail) => {
            return Err((
                failure("invalid_input", "arguments", detail),
                EXIT_INVALID_INPUT,
            ));
        }
    };

    let image = match read_image(&path) {
        Ok(image) => image,
        Err((kind, detail)) => {
            return Err((failure("invalid_input", kind, detail), EXIT_INVALID_INPUT));
        }
    };

    let mut machine = match PagedVcpuSession::with_profile(
        &image,
        usize::from(echo),
        DIAGNOSTIC_TSC_HZ,
        if echo {
            PagedProbeProfile::UartEcho
        } else {
            PagedProbeProfile::Flat
        },
    ) {
        Ok(machine) => machine,
        Err(error) => {
            return Err((
                failure("loader_error", "loader", format!("{error:?}")),
                EXIT_LOADER,
            ));
        }
    };

    if echo && cancel_after != Some(0) {
        if let Some(byte) = rx {
            let scheduled = machine.schedule_rx(machine.rx_generation(), 0, 0, byte);
            match scheduled.and_then(|()| machine.dispatch_rx(1, || false)) {
                Ok(report) if report.delivered == 1 => {}
                outcome => {
                    return Err((
                        failure("integration_error", "rx", format!("{outcome:?}")),
                        EXIT_GUEST,
                    ));
                }
            }
        }
    }
    let mut polls = 0_u64;
    let report = match machine.run(
        budget,
        || {
            if cancel_after.is_some_and(|limit| polls >= limit) {
                true
            } else {
                polls = polls.saturating_add(1);
                false
            }
        },
        |_| {},
    ) {
        Ok(report) => report,
        Err(error) => {
            return Err((
                failure("integration_error", "session", format!("{error:?}")),
                EXIT_GUEST,
            ));
        }
    };
    let serial_bytes = machine.drain_tx();

    let state = machine.state();
    let retired = report.instructions_executed;
    let observed_rax = state.gpr[od_isa::Register::Rax as usize];
    let observed_data_qword = (!echo && retired > 0).then_some(observed_rax);
    let completion_value = (retired > 0).then_some(observed_rax);
    let exit = match &report.exit {
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
    let error = error_details(
        &report.exit,
        report.page_fault.as_ref(),
        report.memory_error.as_ref(),
    );
    let status = classify_exit(
        &report.exit,
        expected,
        retired,
        completion_value,
        &serial_bytes,
    );
    let status = if echo && status == "guest_completed" && state.rip != 0x400003 {
        "completion_mismatch"
    } else {
        status
    };
    let result = json!({
        "schema_version": 1,
        "profile": if echo { "Stage4UartEchoProbeV1" } else { "Stage4FlatProbeV1" },
        "status": status,
        "exit": exit,
        "retired": retired,
        "rip": format!("0x{:016x}", state.rip),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rflags_defined": format!("0x{:016x}", report.rflags_defined),
        "serial_bytes": serial_bytes,
        "observed_rax": format!("0x{observed_rax:016x}"),
        "observed_data_qword": observed_data_qword.map(|value| format!("0x{value:016x}")),
        "error": error,
        "data_gva": format!("0x{STAGE4_FLAT_PROBE_DATA_GVA:016x}"),
    });

    let code = exit_code(&report.exit, status);
    if code == 0 {
        Ok((result, code))
    } else {
        Err((result, code))
    }
}

/// Process exit code of a finished probe run: 0 only for a completed halt,
/// `EXIT_PIO` for a port-I/O error, `EXIT_GUEST` for every other outcome.
fn exit_code(exit: &PagedProbeExit, status: &str) -> u8 {
    match exit {
        PagedProbeExit::Halted if status == "guest_completed" => 0,
        PagedProbeExit::PortIo(_) => EXIT_PIO,
        _ => EXIT_GUEST,
    }
}

fn classify_exit(
    exit: &PagedProbeExit,
    expected: Option<u64>,
    retired: u64,
    observed_data_qword: Option<u64>,
    serial_bytes: &[u8],
) -> &'static str {
    match exit {
        PagedProbeExit::Halted => match expected {
            None => "guest_halted",
            Some(nonce)
                if retired == 3
                    && observed_data_qword == Some(nonce)
                    && serial_bytes == [nonce as u8] =>
            {
                "guest_completed"
            }
            Some(_) => "completion_mismatch",
        },
        PagedProbeExit::Stopped => "host_cancelled",
        PagedProbeExit::BudgetExhausted => "work_limit",
        PagedProbeExit::Fault(_) => "guest_fault",
        PagedProbeExit::Backend(_) => "backend_error",
        PagedProbeExit::PortIo(_) => "pio_error",
        PagedProbeExit::Rejected(_) => "rejected",
        PagedProbeExit::Unsupported { .. } => "unsupported",
        PagedProbeExit::Shutdown => "shutdown",
        PagedProbeExit::IntegrationError(_) => "integration_error",
    }
}

fn parse_args(
    args: &[String],
) -> Result<(String, u64, Option<u64>, Option<u64>, bool, Option<u8>), String> {
    let mut path = None;
    let mut budget = None;
    let mut expected = None;
    let mut cancel_after = None;
    let mut echo = false;
    let mut rx = None;
    let mut saw_expect_nonce = false;
    let mut saw_echo_byte_option = false;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-uart-echo-probe" if path.is_none() => {
                path = Some(value.clone());
                echo = true;
            }
            "--stage4-uart-echo-probe" => return Err("probe may be specified only once".into()),
            "--stage4-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-probe" => return Err("--stage4-probe may be specified only once".into()),
            "--budget" if budget.is_none() => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid budget '{value}'"))?;
                if parsed > MAX_BUDGET {
                    return Err(format!("--budget must be between 0 and {MAX_BUDGET}"));
                }
                budget = Some(parsed);
            }
            "--budget" => return Err("--budget may be specified only once".into()),
            "--expect-nonce" if expected.is_none() => {
                saw_expect_nonce = true;
                expected = Some(
                    if let Some(hex) = value.strip_prefix("0x") {
                        u64::from_str_radix(hex, 16)
                    } else {
                        value.parse::<u64>()
                    }
                    .map_err(|_| format!("invalid nonce '{value}'"))?,
                );
            }
            "--expect-nonce" => return Err("--expect-nonce may be specified only once".into()),
            "--rx-byte" if rx.is_none() => {
                saw_echo_byte_option = true;
                rx = Some(parse_byte(value)?);
            }
            "--rx-byte" => return Err("--rx-byte may be specified only once".into()),
            "--expect-byte" if expected.is_none() => {
                saw_echo_byte_option = true;
                expected = Some(u64::from(parse_byte(value)?));
            }
            "--expect-byte" => return Err("expectation may be specified only once".into()),
            "--cancel-after" if cancel_after.is_none() => {
                cancel_after = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid cancellation count '{value}'"))?,
                );
            }
            "--cancel-after" => return Err("--cancel-after may be specified only once".into()),
            other => return Err(format!("unexpected option '{other}'")),
        }
        index += 2;
    }
    if (echo && saw_expect_nonce) || (!echo && saw_echo_byte_option) {
        return Err("options do not match probe profile".into());
    }
    let path = path.ok_or_else(|| "--stage4-probe IMAGE is required".to_string())?;
    Ok((
        path,
        budget.unwrap_or(DEFAULT_BUDGET),
        expected,
        cancel_after,
        echo,
        rx,
    ))
}

fn parse_byte(value: &str) -> Result<u8, String> {
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        u8::from_str_radix(hex, 16)
    } else {
        value.parse::<u8>()
    };
    parsed.map_err(|_| format!("invalid byte '{value}'"))
}

fn read_image(path: &str) -> Result<Vec<u8>, (&'static str, String)> {
    let file =
        File::open(path).map_err(|error| ("file", format!("could not open '{path}': {error}")))?;
    let mut image = Vec::with_capacity(STAGE4_FLAT_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_FLAT_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| ("file", format!("could not read '{path}': {error}")))?;
    if image.len() != STAGE4_FLAT_PROBE_IMAGE_BYTES {
        return Err((
            "image_size",
            format!(
                "raw image must be exactly {STAGE4_FLAT_PROBE_IMAGE_BYTES} bytes; read {}",
                image.len()
            ),
        ));
    }
    Ok(image)
}

fn error_details(
    exit: &PagedProbeExit,
    page_fault: Option<&od_isa::memory::PageFaultRecord>,
    memory_error: Option<&od_machine::TrackedAccessError>,
) -> Value {
    match exit {
        PagedProbeExit::Halted => Value::Null,
        PagedProbeExit::Shutdown => {
            json!({"kind": "shutdown", "detail": "the CPU entered terminal shutdown"})
        }
        PagedProbeExit::PortIo(error) => json!({"kind": "pio", "detail": format!("{error:?}")}),
        PagedProbeExit::BudgetExhausted => {
            json!({"kind": "budget_exhausted", "detail": "instruction budget was exhausted before HLT"})
        }
        PagedProbeExit::Stopped => {
            json!({"kind": "stopped", "detail": "probe stop predicate returned true"})
        }
        PagedProbeExit::Rejected(error) => {
            json!({"kind": "rejected", "detail": format!("{error:?}")})
        }
        PagedProbeExit::IntegrationError(error) => {
            json!({"kind": "integration_error", "detail": format!("{error:?}")})
        }
        PagedProbeExit::Unsupported { opcode } => {
            json!({"kind": "unsupported", "detail": format!("unsupported opcode 0x{opcode:02x}")})
        }
        PagedProbeExit::Fault(error) => {
            json!({"kind": "guest_fault", "detail": format!("{error:?}"), "page_fault": page_fault.map(|fault| format!("{fault:?}"))})
        }
        PagedProbeExit::Backend(error) => {
            json!({"kind": "memory_backend", "detail": format!("{error:?}"), "memory_error": memory_error.map(|memory_error| format!("{memory_error:?}"))})
        }
    }
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema_version": 1,
        "profile": "Stage4FlatProbeV1",
        "status": status,
        "exit": status,
        "retired": Value::Null,
        "rip": Value::Null,
        "activity": Value::Null,
        "rflags_defined": Value::Null,
        "serial_bytes": [],
        "observed_rax": Value::Null,
        "observed_data_qword": Value::Null,
        "error": {"kind": kind, "detail": detail},
    })
}

fn emit(result: Value, code: u8) -> ExitCode {
    println!("{result}");
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_categories_remain_distinct() {
        let fault = PagedProbeExit::Fault(od_isa::memory::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x1000),
        });
        let backend = PagedProbeExit::Backend(od_isa::memory::MemoryError::Backend("test".into()));
        let pio = PagedProbeExit::PortIo(od_machine::BusAccessError::AddressOverflow);
        for (exit, expected_status) in [
            (fault, "guest_fault"),
            (backend, "backend_error"),
            (pio, "pio_error"),
            (PagedProbeExit::BudgetExhausted, "work_limit"),
        ] {
            assert_eq!(classify_exit(&exit, None, 0, None, &[]), expected_status);
            // The CLI cannot reach a real port error: the loader accepts only the
            // fixed probe code, whose ports (COM1) never fail since S4-38L9z.
            // The PortIo -> exit 6 mapping is therefore pinned here.
            assert_eq!(
                exit_code(&exit, expected_status),
                if expected_status == "pio_error" {
                    EXIT_PIO
                } else {
                    EXIT_GUEST
                }
            );
            assert_eq!(
                error_details(&exit, None, None)["kind"],
                match expected_status {
                    "guest_fault" => "guest_fault",
                    "backend_error" => "memory_backend",
                    "pio_error" => "pio",
                    _ => "budget_exhausted",
                }
            );
        }
        assert_eq!(exit_code(&PagedProbeExit::Halted, "guest_completed"), 0);
        assert_eq!(
            exit_code(&PagedProbeExit::Halted, "completion_mismatch"),
            EXIT_GUEST
        );
    }
}
