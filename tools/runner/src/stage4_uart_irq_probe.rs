//! `--stage4-uart-irq-probe` (S4-24e): COM1 RX -> IRQ4 -> IOAPIC -> LAPIC ->
//! guest handler on the flat `Stage4UartIrqProbeV1` kernel.

use od_machine::{
    STAGE4_UART_IRQ_PROBE_IMAGE_BYTES, STAGE4_UART_IRQ_PROBE_VECTOR, Stage4UartIrqProbeExit,
    UartIrqTraceKind, load_stage4_uart_irq_probe,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const PROFILE: &str = "Stage4UartIrqProbeV1";
const DEFAULT_BUDGET: u64 = 4096;
const MAX_BUDGET: u64 = 100_000;
const MAX_RX: usize = 8;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_GUEST: u8 = 5;

struct Options {
    path: String,
    budget: u64,
    rx: Vec<(u64, u8)>,
    expect_echo: Vec<u8>,
    expect_irqs: Option<u64>,
    cancel_after: Option<u64>,
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

fn parse_byte(value: &str) -> Result<u8, String> {
    u8::try_from(parse_number(value)?).map_err(|_| format!("byte out of range '{value}'"))
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut path = None;
    let mut budget = None;
    let mut rx = Vec::new();
    let mut expect_echo = Vec::new();
    let mut expect_irqs = None;
    let mut cancel_after = None;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--stage4-uart-irq-probe" if path.is_none() => path = Some(value.clone()),
            "--stage4-uart-irq-probe" => return Err("probe may be specified only once".into()),
            "--budget" if budget.is_none() => {
                let parsed = parse_number(value)?;
                if parsed > MAX_BUDGET {
                    return Err(format!("--budget must be between 0 and {MAX_BUDGET}"));
                }
                budget = Some(parsed);
            }
            "--budget" => return Err("--budget may be specified only once".into()),
            "--rx" => {
                // BYTE@DEADLINE_NS: a scheduled RX byte at an absolute virtual time.
                let (byte, deadline) = value
                    .split_once('@')
                    .ok_or_else(|| format!("--rx expects BYTE@DEADLINE_NS, got '{value}'"))?;
                if rx.len() >= MAX_RX {
                    return Err(format!("at most {MAX_RX} --rx events are accepted"));
                }
                rx.push((parse_number(deadline)?, parse_byte(byte)?));
            }
            "--expect-echo" => {
                if expect_echo.len() >= MAX_RX {
                    return Err(format!("at most {MAX_RX} --expect-echo bytes are accepted"));
                }
                expect_echo.push(parse_byte(value)?);
            }
            "--expect-irqs" if expect_irqs.is_none() => expect_irqs = Some(parse_number(value)?),
            "--expect-irqs" => return Err("--expect-irqs may be specified only once".into()),
            "--cancel-after" if cancel_after.is_none() => cancel_after = Some(parse_number(value)?),
            "--cancel-after" => return Err("--cancel-after may be specified only once".into()),
            other => return Err(format!("unexpected option '{other}'")),
        }
        index += 2;
    }
    Ok(Options {
        path: path.ok_or_else(|| "--stage4-uart-irq-probe IMAGE is required".to_string())?,
        budget: budget.unwrap_or(DEFAULT_BUDGET),
        rx,
        expect_echo,
        expect_irqs,
        cancel_after,
    })
}

fn read_image(path: &str) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("could not open '{path}': {error}"))?;
    let mut image = Vec::with_capacity(STAGE4_UART_IRQ_PROBE_IMAGE_BYTES + 1);
    file.take((STAGE4_UART_IRQ_PROBE_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut image)
        .map_err(|error| format!("could not read '{path}': {error}"))?;
    if image.len() != STAGE4_UART_IRQ_PROBE_IMAGE_BYTES {
        return Err(format!(
            "raw image must be exactly {STAGE4_UART_IRQ_PROBE_IMAGE_BYTES} bytes; read {}",
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

fn trace_json(event: &od_machine::UartIrqTraceEvent) -> Value {
    let mut value = json!({
        "seq": event.seq,
        "clock_ns": event.clock_ns,
        "rip": format!("0x{:016x}", event.rip),
        "event": event.kind.name(),
    });
    match &event.kind {
        UartIrqTraceKind::RxDelivered { byte } => value["byte"] = json!(byte),
        UartIrqTraceKind::UartLine { high } => value["high"] = json!(high),
        UartIrqTraceKind::Irr { vector }
        | UartIrqTraceKind::IrqEntry { vector }
        | UartIrqTraceKind::Eoi { vector } => value["vector"] = json!(vector),
        UartIrqTraceKind::PortIn { port, value: v }
        | UartIrqTraceKind::PortOut { port, value: v } => {
            value["port"] = json!(format!("0x{port:03x}"));
            value["value"] = json!(v);
        }
        UartIrqTraceKind::IdleAdvance { to_ns } => value["to_ns"] = json!(to_ns),
        UartIrqTraceKind::Lidt
        | UartIrqTraceKind::Sti
        | UartIrqTraceKind::Hlt
        | UartIrqTraceKind::Iretq => {}
    }
    value
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
    let mut probe = load_stage4_uart_irq_probe(&image).map_err(|error| {
        (
            failure("loader_error", "loader", format!("{error:?}")),
            EXIT_LOADER,
        )
    })?;
    for (key, (deadline, byte)) in options.rx.iter().enumerate() {
        probe
            .schedule_rx(key as u64, *deadline, *byte)
            .map_err(|error| {
                (
                    failure("integration_error", "rx", format!("{error:?}")),
                    EXIT_GUEST,
                )
            })?;
    }
    let mut polls = 0_u64;
    let report = probe.run(options.budget, || {
        if options.cancel_after.is_some_and(|limit| polls >= limit) {
            true
        } else {
            polls = polls.saturating_add(1);
            false
        }
    });

    let count = |f: fn(&UartIrqTraceKind) -> bool| probe.count(f);
    let entries = count(|k| matches!(k, UartIrqTraceKind::IrqEntry { .. }));
    let eois = count(|k| matches!(k, UartIrqTraceKind::Eoi { .. }));
    let iretqs = count(|k| matches!(k, UartIrqTraceKind::Iretq));
    let irrs = count(|k| matches!(k, UartIrqTraceKind::Irr { .. }));
    let rx_delivered = count(|k| matches!(k, UartIrqTraceKind::RxDelivered { .. }));
    let (vector_irr, vector_isr) = probe.vector_state();
    let verify = !options.expect_echo.is_empty() || options.expect_irqs.is_some();
    let expected_irqs = options
        .expect_irqs
        .unwrap_or(options.expect_echo.len() as u64);
    let serial = probe.serial_bytes().to_vec();

    let (exit_name, error) = match &report.exit {
        Stage4UartIrqProbeExit::IdleHalt => ("idle_halt", Value::Null),
        Stage4UartIrqProbeExit::BudgetExhausted => (
            "budget_exhausted",
            json!({"kind": "budget_exhausted", "detail": "step budget exhausted before the guest went idle"}),
        ),
        Stage4UartIrqProbeExit::Stopped => (
            "stopped",
            json!({"kind": "stopped", "detail": "probe stop predicate returned true"}),
        ),
        Stage4UartIrqProbeExit::Terminal(cause) => (
            "terminal",
            json!({"kind": "guest_terminal", "detail": format!("{cause:?}")}),
        ),
        Stage4UartIrqProbeExit::Rejected(detail) => {
            ("rejected", json!({"kind": "rejected", "detail": detail}))
        }
        Stage4UartIrqProbeExit::IntegrationError(detail) => (
            "integration_error",
            json!({"kind": "integration_error", "detail": detail}),
        ),
    };
    let completed = report.exit == Stage4UartIrqProbeExit::IdleHalt
        && serial == options.expect_echo
        && entries as u64 == expected_irqs
        && eois as u64 == expected_irqs
        && iretqs as u64 == expected_irqs
        && (vector_irr, vector_isr) == (false, false);
    let status = match &report.exit {
        Stage4UartIrqProbeExit::IdleHalt if !verify => "guest_idle",
        Stage4UartIrqProbeExit::IdleHalt if completed => "guest_completed",
        Stage4UartIrqProbeExit::IdleHalt => "completion_mismatch",
        Stage4UartIrqProbeExit::BudgetExhausted => "work_limit",
        Stage4UartIrqProbeExit::Stopped => "host_cancelled",
        Stage4UartIrqProbeExit::Terminal(_) => "guest_terminal",
        Stage4UartIrqProbeExit::Rejected(_) => "rejected",
        Stage4UartIrqProbeExit::IntegrationError(_) => "integration_error",
    };
    let state = probe.state();
    let reg = |r: od_isa::Register| format!("0x{:016x}", state.gpr[r as usize]);
    let uart = probe.uart();
    let result = json!({
        "schema_version": 1,
        "profile": PROFILE,
        "status": status,
        "exit": exit_name,
        "budget": options.budget,
        "quanta": report.quanta,
        "retired": report.retired,
        "retired_total": probe.retired_total(),
        "clock_ns": probe.clock_ns(),
        "rip": format!("0x{:016x}", state.rip),
        "rflags": format!("0x{:016x}", state.rflags),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rax": reg(od_isa::Register::Rax),
        "rdx": reg(od_isa::Register::Rdx),
        "rdi": reg(od_isa::Register::Rdi),
        "rsp": reg(od_isa::Register::Rsp),
        "serial_bytes": serial,
        "counts": {
            "rx_delivered": rx_delivered,
            "irr": irrs,
            "irq_entry": entries,
            "eoi": eois,
            "iretq": iretqs,
        },
        "apic": {
            "vector": STAGE4_UART_IRQ_PROBE_VECTOR,
            "irr": vector_irr,
            "isr": vector_isr,
        },
        "uart": {
            "iir": uart.peek_iir(),
            "interrupt_line": uart.interrupt_line(),
            "rbr_pending": uart.received_byte(),
        },
        "pending_rx": probe.pending_rx(),
        "expected": {
            "verify": verify,
            "echo": options.expect_echo,
            "irqs": expected_irqs,
        },
        "trace": probe.trace().iter().map(trace_json).collect::<Vec<_>>(),
        "error": error,
    });
    let code = match status {
        "guest_completed" | "guest_idle" => 0,
        _ => EXIT_GUEST,
    };
    if code == 0 {
        Ok((result, code))
    } else {
        Err((result, code))
    }
}
