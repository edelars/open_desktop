//! S4-38L9J: the evidence-discovery checker for the APIC profile (`linux-direct-discovery`).
//!
//! Like `linux_gate`, this is a CHECK OF A REPORT, not a new behaviour of the board: it reads the JSON
//! that `linux-direct --run --set-cpl0 --cpuid-profile stage4a-apic --user-mode --checkpoint-table
//! <oracle>` wrote and decides from the COM1 journal and the checkpoint records whether Linux found
//! the platform the board describes (e820, ECAM, RSDP, one CPU, the IOAPIC, the timer on pin 2, the PIT
//! calibration of the TSC) and did not complain about it. It never starts a guest.
//!
//! The result is DIAGNOSTIC, never counted: the verdict is `PASS_DIAGNOSTIC_NOT_COUNTED` (exit 6) or
//! `FAIL` (exit 5); there is no `PASS`, the stage4a APIC profile is not the counted one (ISSUE-091,
//! ISSUE-068 stays open).
//!
//! What a pass needs (every item is a named check in the verdict):
//! * the run shape: guest started, `--user-mode`, CPUID profile `X8664V1Stage4AApic`;
//! * required markers (`marker:<name>`), each at least once in `stop.com1_text`; the `[ time] ` log
//!   prefix is ignored. The `BIOS-e820:` lines equal the `e820` array of the report (`e820_journal`);
//! * forbidden markers (`forbidden:<name>`), none of which may occur; a `Kernel panic` line is allowed
//!   only as the expected end of the scenario (`Attempted to kill init! exitcode=0x00000000`);
//! * checkpoints C13, C14, C16, C17 are `reached` in this order (`checkpoint_order`).
//!
//! `Refined TSC` lines are recorded in `observations`, they are not a requirement.
//!
//! Exit codes: 6 `PASS_DIAGNOSTIC_NOT_COUNTED`, 5 `FAIL`, 3 invalid input.

use serde_json::{Value, json};
use std::process::ExitCode;

const USAGE: &str = "linux-direct-discovery --report <json> [--out <json>]";
const GATE: &str = "s4-38l9j-discovery";
const APIC_PROFILE: &str = "X8664V1Stage4AApic";
const PANIC_LINE: &str = "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000";
/// Checkpoints that must be reached, in this order (C15 is not asked for).
const CHECKPOINT_ORDER: [&str; 4] = ["C13", "C14", "C16", "C17"];

/// Strings that must not occur in the journal (name, substring).
const FORBIDDEN: [(&str, &str); 19] = [
    ("rdmsr_apicbase", "RDMSR from 0x1b"),
    ("apicbase", "APICBASE"),
    ("firmware_bug", "Firmware Bug"),
    ("limiting_to_one_cpu", "Limiting to one CPU"),
    ("mp_bios_bug", "MP-BIOS bug"),
    ("io_apic_timer", "IO-APIC + timer doesn't work"),
    ("through_8259a", "through the 8259A"),
    ("apic_timer_disabled", "APIC timer disabled"),
    ("disabling_apic_timer", "Disabling APIC timer"),
    (
        "apic_frequency_too_slow",
        "APIC frequency too slow, disabling apic timer",
    ),
    ("fast_tsc_failed", "Fast TSC calibration failed"),
    ("tsc_unstable", "Marking TSC unstable"),
    ("spurious_lapic_timer", "Spurious LAPIC timer interrupt"),
    ("spurious_apic_interrupt", "spurious APIC interrupt"),
    ("unexpected_irq_trap", "unexpected IRQ trap"),
    ("no_irq_handler", "No irq handler for vector"),
    ("kernel_warning", "WARNING:"),
    ("kernel_bug", "BUG:"),
    ("kernel_oops", "Oops"),
];

pub fn run(args: &[String]) -> ExitCode {
    let (report_path, out) = match parse(args) {
        Ok(parsed) => parsed,
        Err(detail) => return invalid_input("arguments", &detail),
    };
    let bytes = match std::fs::read(&report_path) {
        Ok(bytes) => bytes,
        Err(error) => return invalid_input("report", &error.to_string()),
    };
    let report: Value = match serde_json::from_slice(&bytes) {
        Ok(report) => report,
        Err(error) => return invalid_input("report", &format!("not JSON: {error}")),
    };
    let verdict = check_report(&report, &od_testkit::sha256_hex(&bytes));
    let text = format!("{verdict:#}\n");
    if let Some(out) = &out
        && let Err(error) = std::fs::write(out, &text)
    {
        return invalid_input("out_file", &error.to_string());
    }
    println!("{verdict}");
    ExitCode::from(match verdict["verdict"].as_str() {
        Some("PASS_DIAGNOSTIC_NOT_COUNTED") => 6,
        _ => 5,
    })
}

fn invalid_input(stage: &str, detail: &str) -> ExitCode {
    println!(
        "{}",
        json!({"gate": GATE, "verdict": "invalid_input", "stage": stage, "detail": detail})
    );
    ExitCode::from(3)
}

fn parse(args: &[String]) -> Result<(String, Option<String>), String> {
    let mut report = None;
    let mut out = None;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        let slot = match name {
            "--report" => &mut report,
            "--out" => &mut out,
            _ => return Err(format!("unknown option {name}; usage: {USAGE}")),
        };
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{name} needs a value; usage: {USAGE}"))?;
        if slot.replace(value.clone()).is_some() {
            return Err(format!("duplicate option {name}; usage: {USAGE}"));
        }
        index += 2;
    }
    let report = report.ok_or_else(|| format!("missing --report; usage: {USAGE}"))?;
    Ok((report, out))
}

struct Checks(Vec<Value>);

impl Checks {
    fn add(&mut self, name: &str, ok: bool, detail: String) {
        self.0
            .push(json!({"name": name, "ok": ok, "detail": detail}));
    }
}

/// `[    2.273296] text` -> `text`; a line without the `[ time] ` prefix is returned as is.
fn strip_log_prefix(line: &str) -> &str {
    if line.starts_with('[')
        && let Some(end) = line.find("] ")
    {
        return &line[end + 2..];
    }
    line
}

fn has_token(line: &str, token: &str) -> bool {
    line.split_ascii_whitespace().any(|part| part == token)
}

/// One required marker: a name and a predicate over a journal line without the time prefix.
type Marker = (&'static str, Box<dyn Fn(&str) -> bool>);

fn required_markers(cmdline: Option<String>) -> Vec<Marker> {
    let command_line = cmdline.map(|text| format!("Command line: {text}"));
    vec![
        (
            "command_line",
            Box::new(move |line| command_line.as_deref() == Some(line)),
        ),
        (
            "ecam",
            Box::new(|line| line.starts_with("PCI: ECAM [mem 0xfc100000-0xfc1fffff]")),
        ),
        (
            "pnp0c02_ecam",
            Box::new(|line| line.starts_with("acpi PNP0C02") && line.contains("0xfc100000")),
        ),
        (
            "rsdp",
            Box::new(|line| line.starts_with("ACPI: RSDP 0x00000000000E0000")),
        ),
        (
            "present_cpus",
            Box::new(|line| line.ends_with("Allowing 1 present CPUs plus 0 hotplug CPUs")),
        ),
        (
            "brought_up_one_cpu",
            Box::new(|line| line == "smp: Brought up 1 node, 1 CPU"),
        ),
        (
            "ioapic0",
            Box::new(|line| {
                line.starts_with("IOAPIC[0]:")
                    && line.contains("address 0xfec00000")
                    && line.ends_with("GSI 0-23")
            }),
        ),
        (
            "int_src_ovr_irq0_gsi2",
            Box::new(|line| {
                line.starts_with("ACPI: INT_SRC_OVR") && line.contains("bus_irq 0 global_irq 2 ")
            }),
        ),
        (
            "int_src_ovr_irq9_gsi9_low_level",
            Box::new(|line| {
                line.starts_with("ACPI: INT_SRC_OVR")
                    && line.contains("bus_irq 9 global_irq 9 low level")
            }),
        ),
        (
            "using_ioapic_routing",
            Box::new(|line| line == "ACPI: Using IOAPIC for interrupt routing"),
        ),
        (
            "ttys0_irq4",
            Box::new(|line| line.contains("ttyS0 at I/O 0x3f8 (irq = 4,")),
        ),
        (
            "timer_pin1_2",
            Box::new(|line| line.starts_with("..TIMER:") && has_token(line, "pin1=2")),
        ),
        (
            "fast_tsc_pit",
            Box::new(|line| line == "tsc: Fast TSC calibration using PIT"),
        ),
        (
            "tsc_detected",
            Box::new(|line| line.starts_with("tsc: Detected ")),
        ),
        ("run_init", Box::new(|line| line.starts_with("Run /init"))),
    ]
}

/// `BIOS-e820: [mem 0x..-0x..]  System RAM` -> (start, inclusive end, label). Gap lines give `None`.
fn parse_e820_line(line: &str) -> Option<(u64, u64, String)> {
    let rest = line.strip_prefix("BIOS-e820: [mem 0x")?;
    let (start, rest) = rest.split_once("-0x")?;
    let (end, label) = rest.split_once(']')?;
    Some((
        u64::from_str_radix(start, 16).ok()?,
        u64::from_str_radix(end, 16).ok()?,
        label.trim().to_string(),
    ))
}

fn hex(value: &Value) -> Option<u64> {
    u64::from_str_radix(value.as_str()?.strip_prefix("0x")?, 16).ok()
}

/// The Linux label of a report e820 type.
fn e820_label(kind: u64) -> Option<&'static str> {
    match kind {
        1 => Some("System RAM"),
        2 => Some("device reserved"),
        3 => Some("ACPI data"),
        4 => Some("ACPI NVS"),
        _ => None,
    }
}

fn check_e820(report: &Value, lines: &[&str], checks: &mut Checks) {
    let Some(table) = report["e820"].as_array() else {
        checks.add("e820_journal", false, "report.e820 is missing".to_string());
        return;
    };
    let expected: Option<Vec<(u64, u64, String)>> = table
        .iter()
        .map(|entry| {
            let start = hex(&entry["addr"])?;
            let end = hex(&entry["end"])?;
            let label = e820_label(entry["type"].as_u64()?)?;
            Some((start, end.checked_sub(1)?, label.to_string()))
        })
        .collect();
    let Some(expected) = expected else {
        checks.add(
            "e820_journal",
            false,
            "report.e820 has an unreadable entry".to_string(),
        );
        return;
    };
    let seen: Vec<(u64, u64, String)> = lines
        .iter()
        .filter_map(|line| parse_e820_line(line))
        .collect();
    checks.add(
        "e820_journal",
        !expected.is_empty() && seen == expected,
        format!(
            "journal has {} BIOS-e820 ranges, report.e820 has {}; equal (order, bounds, type): {}",
            seen.len(),
            expected.len(),
            seen == expected
        ),
    );
}

fn check_checkpoints(report: &Value, checks: &mut Checks) {
    let Some(records) = report["checkpoints"]["records"].as_array() else {
        checks.add(
            "checkpoint_order",
            false,
            "report.checkpoints.records is missing (run without --checkpoint-table)".to_string(),
        );
        return;
    };
    let mut retired = Vec::new();
    let mut problems = Vec::new();
    for id in CHECKPOINT_ORDER {
        let record = records.iter().find(|record| record["id"] == json!(id));
        match record {
            Some(record) if record["status"] == json!("reached") => {
                match record["retired"].as_u64() {
                    Some(count) => retired.push((id, count)),
                    None => problems.push(format!("{id} has no retired count")),
                }
            }
            Some(record) => problems.push(format!("{id} status is {}", record["status"])),
            None => problems.push(format!("{id} is missing")),
        }
    }
    if problems.is_empty() && !retired.windows(2).all(|pair| pair[0].1 < pair[1].1) {
        problems.push(format!("not in increasing retired order: {retired:?}"));
    }
    checks.add(
        "checkpoint_order",
        problems.is_empty(),
        if problems.is_empty() {
            format!("C13 -> C14 -> C16 -> C17 reached in order: {retired:?}")
        } else {
            problems.join("; ")
        },
    );
}

/// Runs every check on a parsed report and returns the verdict JSON.
fn check_report(report: &Value, report_sha256: &str) -> Value {
    let mut checks = Checks(Vec::new());
    let stop = &report["stop"];

    // Run shape.
    checks.add(
        "guest_started",
        report["guest_started"] == json!(true) && stop.is_object(),
        format!(
            "guest_started={}, stop is object: {}",
            report["guest_started"],
            stop.is_object()
        ),
    );
    checks.add(
        "user_mode",
        stop["user_mode"] == json!(true),
        format!("stop.user_mode={}", stop["user_mode"]),
    );
    checks.add(
        "cpuid_profile",
        report["cpuid_profile"].as_str() == Some(APIC_PROFILE),
        format!(
            "cpuid_profile={}, expected \"{APIC_PROFILE}\"",
            report["cpuid_profile"]
        ),
    );

    // COM1 journal.
    let com1 = stop["com1_text"].as_str().unwrap_or("");
    let lines: Vec<&str> = com1
        .split('\n')
        .map(|line| strip_log_prefix(line.trim_end_matches('\r')))
        .collect();

    check_e820(report, &lines, &mut checks);
    for (name, matcher) in required_markers(report["cmdline"].as_str().map(str::to_string)) {
        let count = lines.iter().filter(|line| matcher(line)).count();
        checks.add(
            &format!("marker:{name}"),
            count >= 1,
            format!("matching journal lines: {count}, expected at least 1"),
        );
    }
    for (name, text) in FORBIDDEN.iter() {
        let hits: Vec<&&str> = lines.iter().filter(|line| line.contains(text)).collect();
        checks.add(
            &format!("forbidden:{name}"),
            hits.is_empty(),
            match hits.first() {
                None => format!("\"{text}\" does not occur"),
                Some(first) => format!(
                    "\"{text}\" occurs in {} line(s), first: {first}",
                    hits.len()
                ),
            },
        );
    }
    // A `Kernel panic` line is allowed only as the expected end of the scenario (the `---[ end ...`
    // closing line carries the same text).
    let stray: Vec<&&str> = lines
        .iter()
        .filter(|line| line.contains("Kernel panic") && !line.contains(PANIC_LINE))
        .collect();
    checks.add(
        "forbidden:kernel_panic",
        stray.is_empty(),
        match stray.first() {
            None => format!("no Kernel panic other than \"{PANIC_LINE}\""),
            Some(first) => format!("unexpected Kernel panic line: {first}"),
        },
    );

    check_checkpoints(report, &mut checks);

    let observations: Vec<&str> = lines
        .iter()
        .filter(|line| line.contains("Refined TSC"))
        .copied()
        .collect();

    let ok = checks.0.iter().all(|check| check["ok"] == json!(true));
    let failures: Vec<String> = checks
        .0
        .iter()
        .filter(|check| check["ok"] != json!(true))
        .map(|check| {
            format!(
                "{}: {}",
                check["name"].as_str().unwrap_or(""),
                check["detail"]
            )
        })
        .collect();
    json!({
        "gate": GATE,
        "verdict": if ok { "PASS_DIAGNOSTIC_NOT_COUNTED" } else { "FAIL" },
        "counted": false,
        "report_sha256": report_sha256,
        "acceptance": report["acceptance"],
        "cpuid_profile": report["cpuid_profile"],
        "exit_kind": stop["exit_kind"],
        "checks": checks.0,
        "failures": failures,
        "observations": {"refined_tsc_lines": observations},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMDLINE: &str =
        "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=s4-38l9j nokaslr";

    /// The platform lines of the journal; each marker below appears on exactly one of them.
    fn journal_lines() -> Vec<String> {
        vec![
            format!("[    0.000000] Command line: {CMDLINE}"),
            "[    0.000000] BIOS-e820: [mem 0x0000000000000000-0x000000000009efff]  System RAM".to_string(),
            "[    0.000000] BIOS-e820: [mem 0x000000000009f000-0x00000000000dffff]  device reserved".to_string(),
            "[    0.000000] BIOS-e820: [mem 0x00000000000e0000-0x00000000000eefff]  ACPI data".to_string(),
            "[    0.000000] BIOS-e820: [mem 0x00000000000ef000-0x00000000000effff]  ACPI NVS".to_string(),
            "[    0.000000] BIOS-e820: [gap 0x0000000008000000-0x00000000fc0fffff]".to_string(),
            "[    0.000000] BIOS-e820: [mem 0x00000000fc100000-0x00000000fc1fffff]  device reserved".to_string(),
            "[    0.000000] tsc: Fast TSC calibration using PIT".to_string(),
            "[    0.000000] tsc: Detected 999.951 MHz processor".to_string(),
            "[    0.014699] ACPI: RSDP 0x00000000000E0000 000024 (v02 OPNDSK)".to_string(),
            "[    0.018325] IOAPIC[0]: apic_id 1, version 32, address 0xfec00000, GSI 0-23".to_string(),
            "[    0.018358] ACPI: INT_SRC_OVR (bus 0 bus_irq 0 global_irq 2 dfl dfl)".to_string(),
            "[    0.018382] ACPI: INT_SRC_OVR (bus 0 bus_irq 9 global_irq 9 low level)".to_string(),
            "[    0.018636] CPU topo: Allowing 1 present CPUs plus 0 hotplug CPUs".to_string(),
            "[    1.448457] ..TIMER: vector=0x30 apic1=0 pin1=2 apic2=-1 pin2=-1".to_string(),
            "[    1.679667] smp: Brought up 1 node, 1 CPU".to_string(),
            "[    1.698511] PCI: ECAM [mem 0xfc100000-0xfc1fffff] (base 0xfc100000) for domain 0000 [bus 00-00]".to_string(),
            "[    1.820380] ACPI: Using IOAPIC for interrupt routing".to_string(),
            "[    1.861096] acpi PNP0C02:00: Reserved [mem 0xfc100000-0xfc1fffff]".to_string(),
            "[    2.255116] 00:00: ttyS0 at I/O 0x3f8 (irq = 4, base_baud = 115200) is a 16450".to_string(),
            "[    2.376237] Run /init as init process".to_string(),
            format!("[    2.400000] {PANIC_LINE}"),
            format!("[    2.400001] ---[ end {PANIC_LINE} ]---"),
        ]
    }

    fn e820() -> Value {
        json!([
            {"addr": "0x0", "end": "0x9f000", "type": 1},
            {"addr": "0x9f000", "end": "0xe0000", "type": 2},
            {"addr": "0xe0000", "end": "0xef000", "type": 3},
            {"addr": "0xef000", "end": "0xf0000", "type": 4},
            {"addr": "0xfc100000", "end": "0xfc200000", "type": 2},
        ])
    }

    fn records() -> Value {
        json!([
            {"id": "C12", "status": "reached", "retired": 10},
            {"id": "C13", "status": "reached", "retired": 20},
            {"id": "C14", "status": "reached", "retired": 30},
            {"id": "C15", "status": "reached", "retired": 35},
            {"id": "C16", "status": "reached", "retired": 40},
            {"id": "C17", "status": "reached", "retired": 50},
        ])
    }

    fn report_with(lines: &[String]) -> Value {
        json!({
            "guest_started": true,
            "cpuid_profile": APIC_PROFILE,
            "acceptance": "diagnostic_not_counted",
            "cmdline": CMDLINE,
            "e820": e820(),
            "checkpoints": {"records": records()},
            "stop": {
                "user_mode": true,
                "exit_kind": "budget_exhausted",
                "com1_text": lines.join("\r\n") + "\r\n",
            },
        })
    }

    fn good() -> Value {
        report_with(&journal_lines())
    }

    fn verdict(report: &Value) -> Value {
        check_report(report, "00")
    }

    fn failed(report: &Value, check: &str) {
        let value = verdict(report);
        assert_eq!(value["verdict"], "FAIL", "{check}: {value:#}");
        let failures = value["failures"].as_array().unwrap();
        assert!(
            failures
                .iter()
                .any(|f| f.as_str().unwrap().starts_with(&format!("{check}:"))),
            "no failure for {check}: {value:#}"
        );
    }

    #[test]
    fn a_complete_apic_journal_passes_as_diagnostic_never_counted() {
        let value = verdict(&good());
        assert_eq!(value["verdict"], "PASS_DIAGNOSTIC_NOT_COUNTED", "{value:#}");
        assert_eq!(value["counted"], false);
        assert!(value["failures"].as_array().unwrap().is_empty());
    }

    /// Every required marker, taken out of the journal, fails exactly its own check.
    #[test]
    fn each_missing_required_marker_fails() {
        let names: Vec<&str> = required_markers(Some(CMDLINE.to_string()))
            .iter()
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(names.len(), 15);
        for (name, matcher) in required_markers(Some(CMDLINE.to_string())) {
            let kept: Vec<String> = journal_lines()
                .into_iter()
                .filter(|line| !matcher(strip_log_prefix(line)))
                .collect();
            assert!(kept.len() < journal_lines().len(), "{name} matched nothing");
            failed(&report_with(&kept), &format!("marker:{name}"));
        }
    }

    #[test]
    fn the_command_line_must_equal_the_report_cmdline() {
        let lines: Vec<String> = journal_lines()
            .into_iter()
            .map(|line| line.replace("nokaslr", "other"))
            .collect();
        failed(&report_with(&lines), "marker:command_line");
        // `Kernel command line:` is not `Command line:`.
        let lines: Vec<String> = journal_lines()
            .into_iter()
            .map(|line| line.replace("Command line:", "Kernel command line:"))
            .collect();
        failed(&report_with(&lines), "marker:command_line");
    }

    #[test]
    fn marker_variants_that_are_not_the_marker_fail() {
        for (from, line, check) in [
            (
                "smp: Brought up 1 node, 1 CPU",
                "smp: Brought up 1 node, 2 CPUs",
                "marker:brought_up_one_cpu",
            ),
            ("pin1=2", "pin1=0", "marker:timer_pin1_2"),
            ("GSI 0-23", "GSI 0-15", "marker:ioapic0"),
            ("address 0xfec00000", "address 0xfec10000", "marker:ioapic0"),
            ("irq = 4", "irq = 3", "marker:ttys0_irq4"),
            (
                "bus_irq 9 global_irq 9 low level",
                "bus_irq 9 global_irq 9 dfl dfl",
                "marker:int_src_ovr_irq9_gsi9_low_level",
            ),
            (
                "bus_irq 0 global_irq 2 ",
                "bus_irq 0 global_irq 0 ",
                "marker:int_src_ovr_irq0_gsi2",
            ),
            (
                "Allowing 1 present CPUs",
                "Allowing 2 present CPUs",
                "marker:present_cpus",
            ),
            (
                "plus 0 hotplug CPUs",
                "plus 3 hotplug CPUs",
                "marker:present_cpus",
            ),
        ] {
            assert!(journal_lines().iter().any(|l| l.contains(from)), "{from}");
            let lines: Vec<String> = journal_lines()
                .iter()
                .map(|l| l.replace(from, line))
                .collect();
            failed(&report_with(&lines), check);
        }
    }

    /// Every forbidden marker, added to a good journal, fails exactly its own check.
    #[test]
    fn each_present_forbidden_marker_fails() {
        for (name, text) in FORBIDDEN.iter() {
            let mut lines = journal_lines();
            lines.insert(10, format!("[    0.017] {text} at rIP: 0xffffffff81409342"));
            failed(&report_with(&lines), &format!("forbidden:{name}"));
        }
    }

    #[test]
    fn an_unexpected_kernel_panic_fails_but_the_expected_one_does_not() {
        let mut lines = journal_lines();
        lines
            .push("[    3.0] Kernel panic - not syncing: VFS: Unable to mount root fs".to_string());
        failed(&report_with(&lines), "forbidden:kernel_panic");
        let mut lines = journal_lines();
        lines.push(
            "[    3.0] Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000100"
                .to_string(),
        );
        failed(&report_with(&lines), "forbidden:kernel_panic");
        assert_eq!(verdict(&good())["verdict"], "PASS_DIAGNOSTIC_NOT_COUNTED");
    }

    #[test]
    fn the_e820_lines_must_equal_the_report_array() {
        // A changed bound.
        let lines: Vec<String> = journal_lines()
            .into_iter()
            .map(|line| line.replace("0x000000000009efff", "0x000000000009dfff"))
            .collect();
        failed(&report_with(&lines), "e820_journal");
        // A missing range.
        let lines: Vec<String> = journal_lines()
            .into_iter()
            .filter(|line| !line.contains("ACPI NVS"))
            .collect();
        failed(&report_with(&lines), "e820_journal");
        // A changed type label.
        let lines: Vec<String> = journal_lines()
            .into_iter()
            .map(|line| line.replace("ACPI data", "System RAM"))
            .collect();
        failed(&report_with(&lines), "e820_journal");
        // A report array that differs from the journal.
        let mut report = good();
        report["e820"][0]["end"] = json!("0xa0000");
        failed(&report, "e820_journal");
        let mut report = good();
        report["e820"] = json!(null);
        failed(&report, "e820_journal");
    }

    #[test]
    fn checkpoints_must_be_reached_in_order() {
        for index in [1usize, 2, 4, 5] {
            let mut report = good();
            report["checkpoints"]["records"][index]["status"] = json!("not_reached");
            failed(&report, "checkpoint_order");
            let mut report = good();
            report["checkpoints"]["records"]
                .as_array_mut()
                .unwrap()
                .remove(index);
            failed(&report, "checkpoint_order");
        }
        // C16 before C14.
        let mut report = good();
        report["checkpoints"]["records"][4]["retired"] = json!(25);
        failed(&report, "checkpoint_order");
        // No table at all.
        let mut report = good();
        report["checkpoints"] = json!(null);
        failed(&report, "checkpoint_order");
    }

    #[test]
    fn a_wrong_run_shape_fails() {
        let mut report = good();
        report["cpuid_profile"] = json!("X8664V1Stage4A");
        failed(&report, "cpuid_profile");
        let mut report = good();
        report["stop"]["user_mode"] = json!(false);
        failed(&report, "user_mode");
        let mut report = good();
        report["guest_started"] = json!(false);
        failed(&report, "guest_started");
    }

    #[test]
    fn refined_tsc_is_an_observation_not_a_requirement() {
        let value = verdict(&good());
        assert!(
            value["observations"]["refined_tsc_lines"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let mut lines = journal_lines();
        lines.push("[    9.9] tsc: Refined TSC clocksource calibration: 999.980 MHz".to_string());
        let value = verdict(&report_with(&lines));
        assert_eq!(value["verdict"], "PASS_DIAGNOSTIC_NOT_COUNTED", "{value:#}");
        assert_eq!(
            value["observations"]["refined_tsc_lines"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn arguments_are_validated() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(parse(&args(&["--report", "a.json"])).is_ok());
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["--report"])).is_err());
        assert!(parse(&args(&["--report", "a", "--report", "b"])).is_err());
        assert!(parse(&args(&["--report", "a", "--nonce", "x"])).is_err());
    }

    #[test]
    fn run_exit_codes_and_out_file() {
        let dir = std::env::temp_dir().join(format!("od-discovery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let code = |code: ExitCode| format!("{code:?}");

        std::fs::write(path("good.json"), good().to_string()).unwrap();
        let out = path("verdict.json");
        let result = run(&args(&["--report", &path("good.json"), "--out", &out]));
        assert_eq!(code(result), code(ExitCode::from(6)));
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(written["verdict"], "PASS_DIAGNOSTIC_NOT_COUNTED");

        let mut bad = good();
        bad["stop"]["user_mode"] = json!(false);
        std::fs::write(path("bad.json"), bad.to_string()).unwrap();
        assert_eq!(
            code(run(&args(&["--report", &path("bad.json")]))),
            code(ExitCode::from(5))
        );

        std::fs::write(path("garbage.json"), "not json").unwrap();
        for list in [
            vec!["--report", "/nonexistent/report.json"],
            vec!["--bogus"],
            vec!["--report", &path("garbage.json")],
        ] {
            assert_eq!(code(run(&args(&list))), code(ExitCode::from(3)));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
