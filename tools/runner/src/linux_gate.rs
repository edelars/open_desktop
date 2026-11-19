//! S4-39j: the "Linux userspace confirmed" gate checker (`linux-direct-gate`).
//!
//! The gate is a CHECK OF A REPORT, not a new behaviour of the board: it reads the JSON that
//! `linux-direct --run --set-cpl0 --cpuid-profile stage4a --user-mode` wrote and decides PASS or FAIL
//! from the evidence in it. It never starts a guest, and the board, the report and `exit_kind`
//! are unchanged.
//!
//! What a PASS needs (every item is a named check in the verdict):
//! * `acceptance` is `counted` (`diagnostic_not_counted` is refused, see below);
//! * the guest was started under `--user-mode` and the report nonce is the expected one;
//! * the SHA-256 of vmlinuz and initramfs (file, and initramfs in guest memory) and the ACPI
//!   manifest hash in the report equal the expected values from the command line;
//! * `stop.com1_text` has exactly one line equal to `OD_NONCE=<nonce>` (a line, not a substring
//!   of another line);
//! * `stop.com1_text` has the line `Kernel panic - not syncing: Attempted to kill init!
//!   exitcode=0x00000000` (the normal end of the scenario: `/init` called `exit(0)`; the optional
//!   `[ time] ` log prefix is ignored);
//! * `stop.syscall_journal` is exactly mkdir(83), mount(165), openat(257), read(0), close(3),
//!   write(1), getpid(39), nanosleep(35), exit(60) as in `init.S`, every call but the last has
//!   returned, the last (`exit`) has not, nothing overflowed, and `write` returned the length of
//!   `OD_NONCE=<nonce>\n`;
//! * (ISSUE-098) `cpuid_profile`: the report names the expected CPUID profile (default
//!   `X8664V1Stage4A`, `--expected-cpuid-profile` for another one); `acceptance_consistent`: a
//!   `counted` report has `diagnostic_past_c7` false in the report and in `stop`;
//! * (ISSUE-098) `exit_kind`: `stop.exit_kind` is `budget_exhausted`, the end the board produces
//!   in the panic loop (any other end, including a missing field, fails); `no_machine_fault`:
//!   `machine_stop_reason`, `memory_error`, `unsupported_opcode`, `page_fault` and
//!   `cpuid_divergence_leaf` are present and `null`;
//! * (ISSUE-098) `timer_irq`: `irq0_edges`, `irqs_delivered` and `cpl3_retired` are each at least 1,
//!   and `journal_nanosleep`: `nanosleep` returned 0. `irq0_edges` counts edges of the IRQ0 source,
//!   not deliveries to the CPU, and `irqs_delivered` counts every external IRQ (UART IRQ4 too), and
//!   neither is split by CPL: this is evidence that the timer ticked and some external IRQs were
//!   delivered, NOT that IRQ0 was delivered or that an IRQ was taken from CPL3 (S4-39h, ISSUE-090);
//! * (ISSUE-092) `nonce_fresh`: with `--used-nonces <file>` (one nonce per line, `#` comments) the
//!   run's nonce is not in the list of earlier accepted runs. The gate only reads the file; a
//!   malformed entry (`,`, `;`, whitespace, control characters) is an argument error, a trailing
//!   `# comment` and a BOM are tolerated. Without the option the check does not fail and its detail
//!   says `NOT CHECKED` (the verdict's `expected.used_nonces_checked` is false). The gate cannot
//!   tell a fresh nonce from a reused one on its own: the operator keeps the list.
//!
//! The journal entry has no `rdi`: the exit code of `exit()` is checked through the kernel's
//! `exitcode=0x00000000` in the panic line (for `exit(n)` the kernel prints `(n & 0xff) << 8`),
//! and an entry that does carry an `rdi` field must have `0x0`.
//!
//! `acceptance`: since S4-39j the runner marks a `--cpuid-profile stage4a` run without
//! `--diagnostic-past-c7` as `counted`, so such a report can get `PASS`; a run with the flag stays
//! `diagnostic_not_counted` and the gate says FAIL for it. `--allow-diagnostic-acceptance` relaxes
//! only this check: the verdict is then `PASS_DIAGNOSTIC_NOT_COUNTED` (exit 6, `counted: false`),
//! never `PASS`.
//!
//! Run ID: SHA-256 of `od-run-id-v1\nnonce=..\nvmlinuz=..\ninitramfs=..\nacpi_manifest=..\n
//! report=<sha256 of the report file bytes>\n` over the EXPECTED values; no time, no paths.
//!
//! Exit codes: 0 PASS, 5 FAIL, 6 PASS_DIAGNOSTIC_NOT_COUNTED, 3 invalid input (arguments, report
//! unreadable or not JSON).

use od_testkit::sha256_hex;
use serde_json::{Value, json};
use std::process::ExitCode;

const USAGE: &str = "linux-direct-gate --report <json> --nonce <token> \
--expected-vmlinuz-sha256 <hex> --expected-initramfs-sha256 <hex> \
--expected-acpi-manifest-hash <hex> [--expected-cpuid-profile <name>] [--used-nonces <file>] \
[--out <json>] [--allow-diagnostic-acceptance]";

/// The syscalls of `/init` (tests/fixtures/guest/stage4/initramfs/init.S) in order.
const EXPECTED_SYSCALLS: [u64; 9] = [83, 165, 257, 0, 3, 1, 39, 35, 60];
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 60;
const PANIC_LINE: &str = "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000";
const SYS_NANOSLEEP: u64 = 35;
/// The profile of the counted S4-39j run (`X8664V1Stage4A`, 8259 PIC, no APIC).
const DEFAULT_CPUID_PROFILE: &str = "X8664V1Stage4A";
/// The only accepted end of the scenario: the instruction budget runs out while the kernel loops
/// in the `Attempted to kill init!` panic. A terminal `user_exit` stop is not produced by the
/// board; if it is added, it is added here together with its evidence.
const ACCEPTED_EXIT_KIND: &str = "budget_exhausted";
/// `stop` fields that name a machine failure; each must be present and `null` in a PASS.
const FAULT_FIELDS: [&str; 5] = [
    "machine_stop_reason",
    "memory_error",
    "unsupported_opcode",
    "page_fault",
    "cpuid_divergence_leaf",
];

struct Options {
    report: String,
    nonce: String,
    vmlinuz_sha256: String,
    initramfs_sha256: String,
    acpi_manifest_hash: String,
    out: Option<String>,
    allow_diagnostic: bool,
    /// The CPUID profile the report must name (`--expected-cpuid-profile`, default stage4a).
    cpuid_profile: String,
    /// Nonces of earlier accepted runs (`--used-nonces <file>`, one per line); the run's nonce
    /// must not be among them. The gate only reads the file.
    used_nonces: Option<Vec<String>>,
}

pub fn run(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(options) => options,
        Err(detail) => return invalid_input("arguments", &detail),
    };
    let bytes = match std::fs::read(&options.report) {
        Ok(bytes) => bytes,
        Err(error) => return invalid_input("report", &error.to_string()),
    };
    let report: Value = match serde_json::from_slice(&bytes) {
        Ok(report) => report,
        Err(error) => return invalid_input("report", &format!("not JSON: {error}")),
    };
    let verdict = check_report(&report, &sha256_hex(&bytes), &options);
    let text = format!("{verdict:#}\n");
    if let Some(out) = &options.out
        && let Err(error) = std::fs::write(out, &text)
    {
        return invalid_input("out_file", &error.to_string());
    }
    println!("{verdict}");
    ExitCode::from(match verdict["verdict"].as_str() {
        Some("PASS") => 0,
        Some("PASS_DIAGNOSTIC_NOT_COUNTED") => 6,
        _ => 5,
    })
}

fn invalid_input(stage: &str, detail: &str) -> ExitCode {
    println!(
        "{}",
        json!({"gate": "s4-39j", "verdict": "invalid_input", "stage": stage, "detail": detail})
    );
    ExitCode::from(3)
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut report = None;
    let mut nonce = None;
    let mut vmlinuz = None;
    let mut initramfs = None;
    let mut acpi = None;
    let mut out = None;
    let mut cpuid_profile = None;
    let mut used_nonces_file = None;
    let mut allow_diagnostic = false;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--allow-diagnostic-acceptance" {
            if std::mem::replace(&mut allow_diagnostic, true) {
                return Err(format!("duplicate option {name}; usage: {USAGE}"));
            }
            index += 1;
            continue;
        }
        let slot = match name {
            "--report" => &mut report,
            "--nonce" => &mut nonce,
            "--expected-vmlinuz-sha256" => &mut vmlinuz,
            "--expected-initramfs-sha256" => &mut initramfs,
            "--expected-acpi-manifest-hash" => &mut acpi,
            "--out" => &mut out,
            "--expected-cpuid-profile" => &mut cpuid_profile,
            "--used-nonces" => &mut used_nonces_file,
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
    let need = |value: Option<String>, name: &str| {
        value.ok_or_else(|| format!("missing {name}; usage: {USAGE}"))
    };
    let nonce = need(nonce, "--nonce")?;
    if !is_token(&nonce) {
        return Err("--nonce must be a non-empty token without whitespace".to_string());
    }
    let cpuid_profile = cpuid_profile.unwrap_or_else(|| DEFAULT_CPUID_PROFILE.to_string());
    if cpuid_profile.is_empty() || cpuid_profile.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err("--expected-cpuid-profile must be a non-empty token".to_string());
    }
    let used_nonces = match used_nonces_file {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .map_err(|error| format!("--used-nonces {path}: {error}"))?;
            Some(
                parse_used_nonces(&text)
                    .map_err(|error| format!("--used-nonces {path}: {error}"))?,
            )
        }
        None => None,
    };
    Ok(Options {
        report: need(report, "--report")?,
        nonce,
        vmlinuz_sha256: parse_sha256(&need(vmlinuz, "--expected-vmlinuz-sha256")?)?,
        initramfs_sha256: parse_sha256(&need(initramfs, "--expected-initramfs-sha256")?)?,
        acpi_manifest_hash: parse_sha256(&need(acpi, "--expected-acpi-manifest-hash")?)?,
        out,
        allow_diagnostic,
        cpuid_profile,
        used_nonces,
    })
}

/// A non-empty token without whitespace and control characters.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && !text
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
}

/// The `--used-nonces` file: one nonce per line; a leading BOM is dropped; everything from `#` to
/// the end of the line is a comment; blank lines are skipped. An entry that is not a token, or
/// that holds a list separator (`,` or `;`), is an error: a malformed entry must not turn into a
/// nonce that silently matches nothing.
fn parse_used_nonces(text: &str) -> Result<Vec<String>, String> {
    let mut nonces = Vec::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let entry = line.split('#').next().unwrap_or("").trim();
        if entry.is_empty() {
            continue;
        }
        if !is_token(entry) || entry.contains([',', ';']) {
            return Err(format!(
                "line {}: '{entry}' is not a single nonce token (one nonce per line, `#` starts a comment)",
                index + 1
            ));
        }
        nonces.push(entry.to_string());
    }
    Ok(nonces)
}

/// A lowercase 64-digit hex string.
fn parse_sha256(text: &str) -> Result<String, String> {
    let lower = text.to_ascii_lowercase();
    if lower.len() != 64 || !lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("'{text}' is not a 64-digit hex SHA-256"));
    }
    Ok(lower)
}

/// The run ID of a (nonce, input hashes, report) tuple; deterministic, no time.
fn run_id(options: &Options, report_sha256: &str) -> String {
    sha256_hex(
        format!(
            "od-run-id-v1\nnonce={}\nvmlinuz={}\ninitramfs={}\nacpi_manifest={}\nreport={}\n",
            options.nonce,
            options.vmlinuz_sha256,
            options.initramfs_sha256,
            options.acpi_manifest_hash,
            report_sha256
        )
        .as_bytes(),
    )
}

struct Checks(Vec<Value>);

impl Checks {
    fn add(&mut self, name: &str, ok: bool, detail: String) {
        self.0
            .push(json!({"name": name, "ok": ok, "detail": detail}));
    }
}

/// Runs every check on a parsed report and returns the verdict JSON.
fn check_report(report: &Value, report_sha256: &str, options: &Options) -> Value {
    let mut checks = Checks(Vec::new());
    let stop = &report["stop"];
    let mut counted = false;

    // Acceptance.
    let acceptance = report["acceptance"].as_str();
    match acceptance {
        Some("counted") => {
            counted = true;
            checks.add("acceptance", true, "counted".to_string());
        }
        Some("diagnostic_not_counted") if options.allow_diagnostic => checks.add(
            "acceptance",
            true,
            "diagnostic_not_counted allowed by --allow-diagnostic-acceptance; NOT counted"
                .to_string(),
        ),
        Some("diagnostic_not_counted") => checks.add(
            "acceptance",
            false,
            "acceptance is diagnostic_not_counted: the report cannot be counted".to_string(),
        ),
        other => checks.add(
            "acceptance",
            false,
            format!("acceptance is {other:?}, expected \"counted\""),
        ),
    }

    // Run shape.
    let started = report["guest_started"] == json!(true) && stop.is_object();
    checks.add(
        "guest_started",
        started,
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

    // Nonce in the report and the command line.
    let token = format!("od.nonce={}", options.nonce);
    let cmdline_ok = report["cmdline"]
        .as_str()
        .is_some_and(|line| line.split_ascii_whitespace().any(|part| part == token))
        && report["cmdline_in_memory_matches"] == json!(true);
    checks.add(
        "nonce_in_report",
        report["nonce"].as_str() == Some(options.nonce.as_str()) && cmdline_ok,
        format!(
            "report nonce={}, cmdline has {token} and matches guest memory: {cmdline_ok}",
            report["nonce"]
        ),
    );

    // Nonce freshness (ISSUE-092): the nonce was not used by an earlier accepted run.
    // Without `--used-nonces` nothing is checked: the check does not fail, and says so.
    let reused = options
        .used_nonces
        .as_ref()
        .is_some_and(|used| used.iter().any(|used| *used == options.nonce));
    checks.add(
        "nonce_fresh",
        !reused,
        match &options.used_nonces {
            None => format!(
                "NOT CHECKED: --used-nonces not given, the freshness of nonce {} is unknown",
                options.nonce
            ),
            Some(_) if reused => format!("nonce {} is already in --used-nonces", options.nonce),
            Some(used) => format!(
                "nonce {} is not among the {} nonce(s) in --used-nonces",
                options.nonce,
                used.len()
            ),
        },
    );

    // The CPUID profile of the run (ISSUE-098): a PASS is a claim about one profile.
    checks.add(
        "cpuid_profile",
        report["cpuid_profile"].as_str() == Some(options.cpuid_profile.as_str()),
        format!(
            "report cpuid_profile={}, expected \"{}\"",
            report["cpuid_profile"], options.cpuid_profile
        ),
    );
    // A `counted` report cannot come from a run that went past C7: the flag must be present and
    // false in the report and in `stop`. A diagnostic report is not held to this (it is the
    // diagnostic path that `--allow-diagnostic-acceptance` opens).
    let consistent = acceptance != Some("counted")
        || (report["diagnostic_past_c7"] == json!(false)
            && stop["diagnostic_past_c7"] == json!(false));
    checks.add(
        "acceptance_consistent",
        consistent,
        format!(
            "acceptance={:?}, diagnostic_past_c7: report={}, stop={}; counted needs both false",
            acceptance, report["diagnostic_past_c7"], stop["diagnostic_past_c7"]
        ),
    );

    // How the run ended (ISSUE-098): the accepted end, and no machine failure behind it.
    checks.add(
        "exit_kind",
        stop["exit_kind"].as_str() == Some(ACCEPTED_EXIT_KIND),
        format!(
            "stop.exit_kind={}, expected \"{ACCEPTED_EXIT_KIND}\"",
            stop["exit_kind"]
        ),
    );
    let faults: Vec<String> = FAULT_FIELDS
        .iter()
        .filter(|field| stop.get(**field) != Some(&Value::Null))
        .map(|field| format!("{field}={}", stop.get(*field).unwrap_or(&json!("<absent>"))))
        .collect();
    checks.add(
        "no_machine_fault",
        faults.is_empty(),
        if faults.is_empty() {
            format!("{FAULT_FIELDS:?} are all present and null")
        } else {
            format!("fields that are absent or not null: {}", faults.join(", "))
        },
    );

    // Timer and user-code evidence (ISSUE-098). The report counts external interrupts, not their
    // CPL, so this is evidence that the timer ran and was delivered, and that CPL3 code retired;
    // an IRQ taken from CPL3 is not isolated here (S4-39h, ISSUE-090).
    let counter = |name: &str| stop[name].as_u64().unwrap_or(0);
    let timer_ok = counter("irq0_edges") >= 1
        && counter("irqs_delivered") >= 1
        && counter("cpl3_retired") >= 1;
    checks.add(
        "timer_irq",
        timer_ok,
        format!(
            "irq0_edges={}, irqs_delivered={}, cpl3_retired={}, each expected >= 1",
            stop["irq0_edges"], stop["irqs_delivered"], stop["cpl3_retired"]
        ),
    );

    // Hashes of the inputs.
    let hash_checks: [(&str, &Value, &str); 7] = [
        (
            "inputs.vmlinuz.sha256",
            &report["inputs"]["vmlinuz"]["sha256"],
            &options.vmlinuz_sha256,
        ),
        (
            "stop.inputs.vmlinuz_sha256",
            &stop["inputs"]["vmlinuz_sha256"],
            &options.vmlinuz_sha256,
        ),
        (
            "inputs.initramfs.sha256",
            &report["inputs"]["initramfs"]["sha256"],
            &options.initramfs_sha256,
        ),
        (
            "stop.inputs.initramfs_sha256_in_memory",
            &stop["inputs"]["initramfs_sha256_in_memory"],
            &options.initramfs_sha256,
        ),
        (
            "memory_sha256.initrd",
            &report["memory_sha256"]["initrd"],
            &options.initramfs_sha256,
        ),
        (
            "acpi.manifest_hash",
            &report["acpi"]["manifest_hash"],
            &options.acpi_manifest_hash,
        ),
        (
            "stop.inputs.acpi_manifest_hash",
            &stop["inputs"]["acpi_manifest_hash"],
            &options.acpi_manifest_hash,
        ),
    ];
    for (name, value, expected) in hash_checks {
        checks.add(
            &format!("hash:{name}"),
            value.as_str().map(str::to_ascii_lowercase).as_deref() == Some(expected),
            format!("report {value}, expected \"{expected}\""),
        );
    }

    // COM1 lines.
    let com1 = stop["com1_text"].as_str().unwrap_or("");
    let lines: Vec<&str> = com1
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let nonce_line = format!("OD_NONCE={}", options.nonce);
    let nonce_lines = lines.iter().filter(|line| **line == nonce_line).count();
    checks.add(
        "com1_nonce_line",
        nonce_lines == 1,
        format!("lines equal to \"{nonce_line}\": {nonce_lines}, expected exactly 1"),
    );
    let panic_lines = lines
        .iter()
        .filter(|line| strip_log_prefix(line) == PANIC_LINE)
        .count();
    checks.add(
        "com1_panic_line",
        panic_lines >= 1,
        format!("lines equal to \"{PANIC_LINE}\": {panic_lines}, expected at least 1"),
    );

    // Journal.
    check_journal(stop, options, &mut checks);

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
    let verdict = match (ok, counted) {
        (true, true) => "PASS",
        (true, false) => "PASS_DIAGNOSTIC_NOT_COUNTED",
        (false, _) => "FAIL",
    };
    json!({
        "gate": "s4-39j",
        "verdict": verdict,
        "counted": ok && counted,
        "run_id": run_id(options, report_sha256),
        "report_sha256": report_sha256,
        "nonce": options.nonce,
        "expected": {
            "vmlinuz_sha256": options.vmlinuz_sha256,
            "initramfs_sha256": options.initramfs_sha256,
            "acpi_manifest_hash": options.acpi_manifest_hash,
            "cpuid_profile": options.cpuid_profile,
            "used_nonces_checked": options.used_nonces.is_some(),
            "used_nonces_count": options.used_nonces.as_ref().map(Vec::len),
        },
        "acceptance": report["acceptance"],
        "exit_kind": stop["exit_kind"],
        "checks": checks.0,
        "failures": failures,
    })
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

fn parse_hex(value: &Value) -> Option<u64> {
    u64::from_str_radix(value.as_str()?.strip_prefix("0x")?, 16).ok()
}

fn check_journal(stop: &Value, options: &Options, checks: &mut Checks) {
    let journal = &stop["syscall_journal"];
    let Some(entries) = journal["entries"].as_array() else {
        checks.add(
            "journal_sequence",
            false,
            "stop.syscall_journal.entries is missing".to_string(),
        );
        return;
    };
    let numbers: Vec<Option<u64>> = entries.iter().map(|entry| entry["nr"].as_u64()).collect();
    let expected: Vec<Option<u64>> = EXPECTED_SYSCALLS.iter().map(|nr| Some(*nr)).collect();
    checks.add(
        "journal_sequence",
        numbers == expected,
        format!("syscall numbers {numbers:?}, expected {EXPECTED_SYSCALLS:?}"),
    );
    let expected_len = EXPECTED_SYSCALLS.len() as u64;
    checks.add(
        "journal_counters",
        journal["entered"].as_u64() == Some(expected_len)
            && journal["returned"].as_u64() == Some(expected_len - 1)
            && entries.len() as u64 == expected_len,
        format!(
            "entered={}, returned={}, entries={}, expected {}/{}/{} (no overflow)",
            journal["entered"],
            journal["returned"],
            entries.len(),
            expected_len,
            expected_len - 1,
            expected_len
        ),
    );
    let (last, rest) = match entries.split_last() {
        Some(split) => split,
        None => {
            checks.add("journal_exit", false, "the journal is empty".to_string());
            return;
        }
    };
    let rest_returned = rest.iter().all(|entry| !entry["rax_return"].is_null());
    checks.add(
        "journal_returns",
        rest_returned,
        "every call before the last has returned (rax_return set)".to_string(),
    );
    let exit_ok = last["nr"].as_u64() == Some(SYS_EXIT)
        && last["rax_return"].is_null()
        && (last.get("rdi").is_none() || parse_hex(&last["rdi"]) == Some(0));
    checks.add(
        "journal_exit",
        exit_ok,
        format!("last entry {last}, expected exit(60) that did not return, rdi absent or 0x0"),
    );
    // `nanosleep` is the user-visible timer effect: it returns 0 only after the sleep finished.
    let sleep = entries
        .iter()
        .find(|entry| entry["nr"].as_u64() == Some(SYS_NANOSLEEP));
    checks.add(
        "journal_nanosleep",
        sleep.and_then(|entry| parse_hex(&entry["rax_return"])) == Some(0),
        format!(
            "nanosleep rax_return={}, expected \"0x0\"",
            sleep.map_or(Value::Null, |entry| entry["rax_return"].clone())
        ),
    );
    let want = (format!("OD_NONCE={}\n", options.nonce)).len() as u64;
    let write = entries
        .iter()
        .find(|entry| entry["nr"].as_u64() == Some(SYS_WRITE));
    let got = write.and_then(|entry| parse_hex(&entry["rax_return"]));
    checks.add(
        "journal_write_length",
        got == Some(want),
        format!("write returned {got:?}, expected {want} (length of the nonce line)"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "s4-38l4a";
    const VMLINUZ: &str = "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e";
    const INITRAMFS: &str = "ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588";
    const ACPI: &str = "2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2";

    fn options() -> Options {
        Options {
            report: "report.json".to_string(),
            nonce: NONCE.to_string(),
            vmlinuz_sha256: VMLINUZ.to_string(),
            initramfs_sha256: INITRAMFS.to_string(),
            acpi_manifest_hash: ACPI.to_string(),
            out: None,
            allow_diagnostic: false,
            cpuid_profile: DEFAULT_CPUID_PROFILE.to_string(),
            used_nonces: None,
        }
    }

    fn journal() -> Value {
        let rows: [(u64, Option<&str>); 9] = [
            (83, Some("0x0")),
            (165, Some("0x0")),
            (257, Some("0x3")),
            (0, Some("0x5a")),
            (3, Some("0x0")),
            (1, Some("0x12")),
            (39, Some("0x1")),
            (35, Some("0x0")),
            (60, None),
        ];
        let entries: Vec<Value> = rows
            .iter()
            .map(|(nr, ret)| json!({"nr": nr, "rax_return": ret, "rip": "0x400000"}))
            .collect();
        json!({"capacity": 32, "entered": 9, "returned": 8, "entries": entries})
    }

    /// A report that passes every check (acceptance `counted`).
    fn good() -> Value {
        let com1 = format!(
            "[    0.1] boot\r\nOD_NONCE={NONCE}\r\n[    2.2] Run /init as init process\r\n\
[    2.3] {PANIC_LINE}\r\n[    2.4] ---[ end {PANIC_LINE} ]---\r\n"
        );
        json!({
            "acceptance": "counted",
            "guest_started": true,
            "nonce": NONCE,
            "cmdline": format!("console=ttyS0 od.nonce={NONCE} nokaslr"),
            "cmdline_in_memory_matches": true,
            "cpuid_profile": DEFAULT_CPUID_PROFILE,
            "diagnostic_past_c7": false,
            "inputs": {"vmlinuz": {"sha256": VMLINUZ}, "initramfs": {"sha256": INITRAMFS}},
            "memory_sha256": {"initrd": INITRAMFS},
            "acpi": {"manifest_hash": ACPI},
            "stop": {
                "user_mode": true,
                "exit_kind": "budget_exhausted",
                "diagnostic_past_c7": false,
                "machine_stop_reason": null,
                "memory_error": null,
                "unsupported_opcode": null,
                "page_fault": null,
                "cpuid_divergence_leaf": null,
                "irq0_edges": 34799,
                "irqs_delivered": 34804,
                "cpl3_retired": 796,
                "com1_text": com1,
                "syscall_journal": journal(),
                "inputs": {
                    "vmlinuz_sha256": VMLINUZ,
                    "initramfs_sha256_in_memory": INITRAMFS,
                    "acpi_manifest_hash": ACPI,
                },
            },
        })
    }

    fn verdict(report: &Value) -> Value {
        check_report(report, "00", &options())
    }

    fn failed(report: &Value, check: &str) {
        let value = verdict(report);
        assert_eq!(value["verdict"], "FAIL", "{value:#}");
        let failures = value["failures"].as_array().unwrap();
        assert!(
            failures
                .iter()
                .any(|f| f.as_str().unwrap().starts_with(&format!("{check}:"))),
            "no failure for {check}: {value:#}"
        );
    }

    #[test]
    fn a_complete_counted_report_passes() {
        let value = verdict(&good());
        assert_eq!(value["verdict"], "PASS", "{value:#}");
        assert_eq!(value["counted"], true);
        assert!(value["failures"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_wrong_nonce_fails() {
        let mut report = good();
        let text = report["stop"]["com1_text"]
            .as_str()
            .unwrap()
            .replace(NONCE, "other");
        report["stop"]["com1_text"] = json!(text);
        failed(&report, "com1_nonce_line");
        let mut report = good();
        report["nonce"] = json!("other");
        failed(&report, "nonce_in_report");
    }

    #[test]
    fn a_nonce_inside_another_line_is_not_a_nonce_line() {
        for line in [
            format!("xOD_NONCE={NONCE}"),
            format!("OD_NONCE={NONCE}-extra"),
            format!("[    1.0] OD_NONCE={NONCE}"),
            format!("Kernel command line: od.nonce={NONCE}"),
        ] {
            let mut report = good();
            let text = format!("{line}\r\n[    2.3] {PANIC_LINE}\r\n");
            report["stop"]["com1_text"] = json!(text);
            failed(&report, "com1_nonce_line");
        }
    }

    #[test]
    fn a_repeated_nonce_line_fails() {
        let mut report = good();
        let text = format!(
            "{}OD_NONCE={NONCE}\r\n",
            report["stop"]["com1_text"].as_str().unwrap()
        );
        report["stop"]["com1_text"] = json!(text);
        failed(&report, "com1_nonce_line");
    }

    #[test]
    fn a_missing_panic_line_fails() {
        let mut report = good();
        report["stop"]["com1_text"] = json!(format!("OD_NONCE={NONCE}\r\n"));
        failed(&report, "com1_panic_line");
        // The closing line alone ("---[ end Kernel panic ...") is not the panic line.
        let mut report = good();
        report["stop"]["com1_text"] = json!(format!(
            "OD_NONCE={NONCE}\r\n[ 2.4] ---[ end {PANIC_LINE} ]---\r\n"
        ));
        failed(&report, "com1_panic_line");
    }

    #[test]
    fn a_panic_with_a_nonzero_exit_code_fails() {
        let mut report = good();
        let text = report["stop"]["com1_text"]
            .as_str()
            .unwrap()
            .replace("exitcode=0x00000000", "exitcode=0x00000100");
        report["stop"]["com1_text"] = json!(text);
        failed(&report, "com1_panic_line");
    }

    #[test]
    fn a_journal_without_exit_fails() {
        // The run ran out of budget before /init called exit.
        let mut report = good();
        let journal = &mut report["stop"]["syscall_journal"];
        journal["entries"].as_array_mut().unwrap().pop();
        journal["entered"] = json!(8);
        journal["returned"] = json!(8);
        failed(&report, "journal_sequence");
        failed(&report, "journal_exit");
    }

    #[test]
    fn an_exit_that_returned_fails() {
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][8]["rax_return"] = json!("0x0");
        failed(&report, "journal_exit");
    }

    #[test]
    fn an_exit_with_a_nonzero_rdi_fails() {
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][8]["rdi"] = json!("0x1");
        failed(&report, "journal_exit");
        // rdi = 0 is accepted when a future journal carries it.
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][8]["rdi"] = json!("0x0");
        assert_eq!(verdict(&report)["verdict"], "PASS");
    }

    #[test]
    fn a_journal_that_is_not_the_init_sequence_fails() {
        // Swapped order, an extra call (the `missing` path writes again), a missing call.
        let mut report = good();
        let entries = report["stop"]["syscall_journal"]["entries"]
            .as_array_mut()
            .unwrap();
        entries.swap(2, 3);
        failed(&report, "journal_sequence");

        let mut report = good();
        let journal = &mut report["stop"]["syscall_journal"];
        let extra = json!({"nr": 1, "rax_return": "0x5", "rip": "0x400000"});
        journal["entries"].as_array_mut().unwrap().insert(5, extra);
        journal["entered"] = json!(10);
        journal["returned"] = json!(9);
        failed(&report, "journal_sequence");

        let mut report = good();
        let journal = &mut report["stop"]["syscall_journal"];
        journal["entries"].as_array_mut().unwrap().remove(2);
        journal["entered"] = json!(8);
        journal["returned"] = json!(7);
        failed(&report, "journal_sequence");
    }

    #[test]
    fn a_journal_with_a_broken_counter_or_return_fails() {
        let mut report = good();
        report["stop"]["syscall_journal"]["entered"] = json!(40); // overflowed past capacity
        failed(&report, "journal_counters");
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][3]["rax_return"] = Value::Null;
        failed(&report, "journal_returns");
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][5]["rax_return"] = json!("0x11");
        failed(&report, "journal_write_length");
        let mut report = good();
        report["stop"]["syscall_journal"] = Value::Null;
        failed(&report, "journal_sequence");
    }

    #[test]
    fn a_hash_that_does_not_match_fails_on_every_input() {
        let other = "0".repeat(64);
        for (path, check) in [
            ("/inputs/vmlinuz/sha256", "hash:inputs.vmlinuz.sha256"),
            (
                "/stop/inputs/vmlinuz_sha256",
                "hash:stop.inputs.vmlinuz_sha256",
            ),
            ("/inputs/initramfs/sha256", "hash:inputs.initramfs.sha256"),
            (
                "/stop/inputs/initramfs_sha256_in_memory",
                "hash:stop.inputs.initramfs_sha256_in_memory",
            ),
            ("/memory_sha256/initrd", "hash:memory_sha256.initrd"),
            ("/acpi/manifest_hash", "hash:acpi.manifest_hash"),
            (
                "/stop/inputs/acpi_manifest_hash",
                "hash:stop.inputs.acpi_manifest_hash",
            ),
        ] {
            let mut report = good();
            *report.pointer_mut(path).unwrap() = json!(other);
            failed(&report, check);
        }
        // A missing hash is a failure too, and the expected values are the ones from the command.
        let mut report = good();
        report["inputs"] = Value::Null;
        failed(&report, "hash:inputs.vmlinuz.sha256");
        let mut wrong = options();
        wrong.vmlinuz_sha256 = other;
        let value = check_report(&good(), "00", &wrong);
        assert_eq!(value["verdict"], "FAIL");
    }

    #[test]
    fn diagnostic_not_counted_is_refused_and_the_flag_never_gives_pass() {
        let mut report = good();
        report["acceptance"] = json!("diagnostic_not_counted");
        failed(&report, "acceptance");

        let mut allowing = options();
        allowing.allow_diagnostic = true;
        let value = check_report(&report, "00", &allowing);
        assert_eq!(value["verdict"], "PASS_DIAGNOSTIC_NOT_COUNTED", "{value:#}");
        assert_eq!(value["counted"], false);

        // Any other or missing value fails even with the flag; so does a broken journal.
        for acceptance in [json!("accepted"), Value::Null, json!("counted ")] {
            report["acceptance"] = acceptance;
            let value = check_report(&report, "00", &allowing);
            assert_eq!(value["verdict"], "FAIL", "{value:#}");
        }
        report["acceptance"] = json!("diagnostic_not_counted");
        report["stop"]["syscall_journal"]["entries"][0]["nr"] = json!(84);
        assert_eq!(check_report(&report, "00", &allowing)["verdict"], "FAIL");
    }

    #[test]
    fn a_report_without_user_mode_or_start_fails() {
        let mut report = good();
        report["stop"]["user_mode"] = json!(false);
        failed(&report, "user_mode");
        let mut report = good();
        report["guest_started"] = json!(false);
        failed(&report, "guest_started");
        failed(&json!({}), "guest_started");
    }

    #[test]
    fn the_run_id_depends_on_nonce_hashes_and_report_and_nothing_else() {
        let base = run_id(&options(), "aa");
        assert_eq!(base, run_id(&options(), "aa"));
        assert_eq!(base.len(), 64);
        assert_ne!(base, run_id(&options(), "ab"));
        let mut other = options();
        other.nonce = "n2".to_string();
        assert_ne!(base, run_id(&other, "aa"));
        for field in 0..3 {
            let mut other = options();
            let slot = match field {
                0 => &mut other.vmlinuz_sha256,
                1 => &mut other.initramfs_sha256,
                _ => &mut other.acpi_manifest_hash,
            };
            *slot = "1".repeat(64);
            assert_ne!(base, run_id(&other, "aa"), "field {field}");
        }
        // The path of the report and the allow flag do not enter the ID.
        let mut other = options();
        other.report = "elsewhere.json".to_string();
        other.allow_diagnostic = true;
        assert_eq!(base, run_id(&other, "aa"));
        assert_eq!(verdict(&good())["run_id"], run_id(&options(), "00"));
    }

    #[test]
    fn arguments_are_strict() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let full = [
            "--report",
            "r.json",
            "--nonce",
            NONCE,
            "--expected-vmlinuz-sha256",
            VMLINUZ,
            "--expected-initramfs-sha256",
            INITRAMFS,
            "--expected-acpi-manifest-hash",
            ACPI,
        ];
        assert!(parse(&args(&full)).is_ok());
        let mut with_flags = full.to_vec();
        with_flags.extend(["--out", "o.json", "--allow-diagnostic-acceptance"]);
        let parsed = parse(&args(&with_flags)).unwrap();
        assert!(parsed.allow_diagnostic && parsed.out.as_deref() == Some("o.json"));
        assert!(parse(&args(&full[2..])).is_err(), "missing --report");
        assert!(parse(&args(&full[..9])).is_err(), "dangling option");
        let mut bad = full.to_vec();
        bad[3] = "has space";
        assert!(parse(&args(&bad)).is_err());
        let mut bad = full.to_vec();
        bad[5] = "abc";
        assert!(parse(&args(&bad)).is_err());
        let mut dup = full.to_vec();
        dup.extend(["--report", "x"]);
        assert!(parse(&args(&dup)).is_err());
        let mut unknown = full.to_vec();
        unknown.push("--bogus");
        assert!(parse(&args(&unknown)).is_err());
    }

    #[test]
    fn run_reads_a_report_file_writes_the_verdict_and_returns_the_exit_code() {
        let dir = std::env::temp_dir().join(format!("od-runner-linux-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("report.json");
        let out = dir.join("verdict.json");
        let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let args = |extra: &[&str]| {
            let mut list: Vec<String> = [
                "--report",
                &path(&report),
                "--nonce",
                NONCE,
                "--expected-vmlinuz-sha256",
                VMLINUZ,
                "--expected-initramfs-sha256",
                INITRAMFS,
                "--expected-acpi-manifest-hash",
                ACPI,
                "--out",
                &path(&out),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            list.extend(extra.iter().map(|s| s.to_string()));
            list
        };
        let text = serde_json::to_vec(&good()).unwrap();
        std::fs::write(&report, &text).unwrap();
        assert_eq!(run(&args(&[])), ExitCode::from(0));
        let written: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(written["verdict"], "PASS");
        assert_eq!(written["report_sha256"], sha256_hex(&text));

        let mut diagnostic = good();
        diagnostic["acceptance"] = json!("diagnostic_not_counted");
        std::fs::write(&report, serde_json::to_vec(&diagnostic).unwrap()).unwrap();
        assert_eq!(run(&args(&[])), ExitCode::from(5));
        assert_eq!(
            run(&args(&["--allow-diagnostic-acceptance"])),
            ExitCode::from(6)
        );

        std::fs::write(&report, b"not json").unwrap();
        assert_eq!(run(&args(&[])), ExitCode::from(3));
        std::fs::remove_file(&report).unwrap();
        assert_eq!(run(&args(&[])), ExitCode::from(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_s4_39i_diagnostic_report_is_diagnostic_pass_only() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reports/data/stage4/s4-39i-uart-irq4-stage4a-run.json");
        let bytes = std::fs::read(path).unwrap();
        let report: Value = serde_json::from_slice(&bytes).unwrap();
        let sha = sha256_hex(&bytes);
        let value = check_report(&report, &sha, &options());
        assert_eq!(value["verdict"], "FAIL", "{value:#}");
        assert_eq!(value["failures"].as_array().unwrap().len(), 1, "{value:#}");
        let mut allowing = options();
        allowing.allow_diagnostic = true;
        let value = check_report(&report, &sha, &allowing);
        assert_eq!(value["verdict"], "PASS_DIAGNOSTIC_NOT_COUNTED", "{value:#}");
    }

    // S4-40: negative reports for every failure class; none may be PASS or
    // PASS_DIAGNOSTIC_NOT_COUNTED, with and without --allow-diagnostic-acceptance.

    fn never_a_pass(report: &Value, expected: &[&str]) {
        for allow in [false, true] {
            let mut options = options();
            options.allow_diagnostic = allow;
            let value = check_report(report, "00", &options);
            assert_eq!(value["verdict"], "FAIL", "allow={allow}: {value:#}");
            assert_eq!(value["counted"], false);
            let failures = value["failures"].as_array().unwrap();
            for name in expected {
                assert!(
                    failures
                        .iter()
                        .any(|f| f.as_str().unwrap().starts_with(&format!("{name}:"))),
                    "no failure for {name} (allow={allow}): {value:#}"
                );
            }
        }
    }

    /// The journal cut after `kept` calls (the guest never reached exit).
    fn cut_journal(report: &mut Value, kept: usize) {
        let journal = &mut report["stop"]["syscall_journal"];
        journal["entries"].as_array_mut().unwrap().truncate(kept);
        journal["entered"] = json!(kept);
        journal["returned"] = json!(kept);
    }

    fn set_com1(report: &mut Value, text: String) {
        report["stop"]["com1_text"] = json!(text);
    }

    #[test]
    fn s4_40_a_timeout_before_exit_is_not_a_pass() {
        // Budget exhausted while /init sleeps: the journal ends at nanosleep, no exit, no panic.
        let mut report = good();
        report["stop"]["exit_kind"] = json!("budget_exhausted");
        cut_journal(&mut report, 8);
        set_com1(
            &mut report,
            format!("OD_NONCE={NONCE}\r\n[ 2.0] Run /init as init process\r\n"),
        );
        never_a_pass(
            &report,
            &["journal_sequence", "journal_exit", "com1_panic_line"],
        );
        // The same cut with the panic line still in the text (the journal alone is enough).
        let mut report = good();
        cut_journal(&mut report, 8);
        never_a_pass(&report, &["journal_sequence", "journal_exit"]);
    }

    #[test]
    fn s4_40_a_panic_that_is_not_an_init_kill_is_not_a_pass() {
        let mut report = good();
        let other =
            "Kernel panic - not syncing: VFS: Unable to mount root fs on unknown-block(0,0)";
        set_com1(
            &mut report,
            format!("OD_NONCE={NONCE}\r\n[ 2.3] {other}\r\n[ 2.4] ---[ end {other} ]---\r\n"),
        );
        never_a_pass(&report, &["com1_panic_line"]);
        // The kernel died before init: no calls in the journal either.
        cut_journal(&mut report, 0);
        never_a_pass(
            &report,
            &["com1_panic_line", "journal_sequence", "journal_exit"],
        );
    }

    #[test]
    fn s4_40_a_shutdown_before_init_is_not_a_pass() {
        for kind in ["halted", "shutdown", "stopped"] {
            let mut report = good();
            report["stop"]["exit_kind"] = json!(kind);
            report["stop"]["machine_stop_reason"] = json!("shutdown");
            cut_journal(&mut report, 0);
            set_com1(&mut report, "[ 0.5] ACPI: PM-Timer\r\n".to_string());
            never_a_pass(
                &report,
                &["com1_nonce_line", "com1_panic_line", "journal_sequence"],
            );
            // Shorter journal, the nonce line already printed, still no exit.
            let mut report = good();
            report["stop"]["exit_kind"] = json!(kind);
            cut_journal(&mut report, 5);
            set_com1(&mut report, format!("OD_NONCE={NONCE}\r\n"));
            never_a_pass(
                &report,
                &["com1_panic_line", "journal_sequence", "journal_exit"],
            );
        }
    }

    #[test]
    fn s4_40_an_unsupported_instruction_is_not_a_pass() {
        let mut report = good();
        report["stop"]["exit_kind"] = json!("unsupported");
        report["stop"]["unsupported_opcode"] = json!("0xfc");
        cut_journal(&mut report, 3);
        set_com1(&mut report, "[ 1.0] Booting\r\n".to_string());
        never_a_pass(
            &report,
            &[
                "com1_nonce_line",
                "com1_panic_line",
                "journal_sequence",
                "journal_exit",
            ],
        );
    }

    #[test]
    fn s4_40_a_wrong_or_missing_nonce_in_com1_is_not_a_pass() {
        let mut report = good();
        let text = report["stop"]["com1_text"]
            .as_str()
            .unwrap()
            .replace(NONCE, "s4-38l4b");
        set_com1(&mut report, text);
        never_a_pass(&report, &["com1_nonce_line"]);
        // Missing: the line is gone, everything else is intact.
        let mut report = good();
        set_com1(
            &mut report,
            format!("[ 2.2] Run /init as init process\r\n[ 2.3] {PANIC_LINE}\r\n"),
        );
        never_a_pass(&report, &["com1_nonce_line"]);
        // Empty or absent COM1.
        let mut report = good();
        set_com1(&mut report, String::new());
        never_a_pass(&report, &["com1_nonce_line", "com1_panic_line"]);
        report["stop"]["com1_text"] = Value::Null;
        never_a_pass(&report, &["com1_nonce_line", "com1_panic_line"]);
    }

    #[test]
    fn s4_40_exit_with_another_panic_line_or_a_nonzero_exitcode_is_not_a_pass() {
        // exit(60) is in the journal, but the kernel's last words are different.
        let mut report = good();
        let other = "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000100";
        set_com1(
            &mut report,
            format!("OD_NONCE={NONCE}\r\n[ 2.3] {other}\r\n"),
        );
        never_a_pass(&report, &["com1_panic_line"]);
        let mut report = good();
        let other = "Kernel panic - not syncing: Fatal exception in interrupt";
        set_com1(
            &mut report,
            format!("OD_NONCE={NONCE}\r\n[ 2.3] {other}\r\n"),
        );
        never_a_pass(&report, &["com1_panic_line"]);
        // Look-alikes inside another line are not the panic line either.
        let mut report = good();
        set_com1(
            &mut report,
            format!("OD_NONCE={NONCE}\r\nnote: {PANIC_LINE}\r\nx{PANIC_LINE}\r\n"),
        );
        never_a_pass(&report, &["com1_panic_line"]);
    }

    #[test]
    fn s4_40_a_nonce_without_a_write_of_the_right_length_is_not_a_pass() {
        // COM1 has the line, the journal has a short write.
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][5]["rax_return"] = json!("0x5");
        never_a_pass(&report, &["journal_write_length"]);
        // No write at all in the journal (the call is replaced, the sequence breaks too).
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][5]["nr"] = json!(2);
        never_a_pass(&report, &["journal_write_length", "journal_sequence"]);
        // A write that failed (negative errno as unsigned).
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][5]["rax_return"] = json!("0xfffffffffffffff7");
        never_a_pass(&report, &["journal_write_length"]);
    }

    #[test]
    fn s4_40_a_refused_or_failed_run_report_is_not_a_pass() {
        let refused = json!({
            "acceptance": "counted",
            "status": "refused",
            "guest_started": true,
            "nonce": NONCE,
            "error": {"code": "run_failed"},
        });
        never_a_pass(&refused, &["journal_sequence", "com1_nonce_line"]);
        never_a_pass(&json!({}), &["acceptance", "journal_sequence"]);
        never_a_pass(&Value::Null, &["acceptance", "journal_sequence"]);
    }

    #[test]
    fn s4_40_the_real_counted_cold_reports_pass_without_flags() {
        for name in [
            "s4-39j-gate-counted-cold1-stage4a-run.json",
            "s4-39j-gate-counted-cold2-stage4a-run.json",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/reports/data/stage4")
                .join(name);
            let bytes = std::fs::read(path).unwrap();
            let report: Value = serde_json::from_slice(&bytes).unwrap();
            let value = check_report(&report, &sha256_hex(&bytes), &options());
            assert_eq!(value["verdict"], "PASS", "{name}: {value:#}");
            assert_eq!(value["counted"], true);
            assert_eq!(value["exit_kind"], "budget_exhausted");
        }
    }

    // ---- ISSUE-098 / ISSUE-092: exit kind, CPUID profile, machine faults, timer, nonce reuse ----

    #[test]
    fn s4_40_an_end_other_than_the_accepted_one_fails_exit_kind() {
        for kind in [
            "user_exit",
            "halted",
            "shutdown",
            "stopped",
            "unsupported",
            "",
        ] {
            let mut report = good();
            report["stop"]["exit_kind"] = json!(kind);
            failed(&report, "exit_kind");
        }
        let mut report = good();
        report["stop"]["exit_kind"] = Value::Null;
        failed(&report, "exit_kind");
        report["stop"].as_object_mut().unwrap().remove("exit_kind");
        failed(&report, "exit_kind");
    }

    #[test]
    fn s4_40_a_machine_failure_field_fails_even_with_a_clean_log() {
        // COM1 and the journal are perfect; the machine nevertheless reports a failure.
        for (field, value) in [
            ("machine_stop_reason", json!("shutdown")),
            ("memory_error", json!({"gpa": "0x1000"})),
            ("unsupported_opcode", json!("0xfc")),
            ("page_fault", json!({"cr2": "0x0"})),
            ("cpuid_divergence_leaf", json!("0x1")),
        ] {
            let mut report = good();
            report["stop"][field] = value;
            failed(&report, "no_machine_fault");
        }
        // A field that is absent is not "no failure": a schema change must not pass silently.
        for field in FAULT_FIELDS {
            let mut report = good();
            report["stop"].as_object_mut().unwrap().remove(field);
            failed(&report, "no_machine_fault");
        }
    }

    #[test]
    fn s4_40_a_wrong_cpuid_profile_fails() {
        for profile in [
            json!("X8664V1Stage4AApic"),
            json!("X8664V1"),
            json!(""),
            Value::Null,
        ] {
            let mut report = good();
            report["cpuid_profile"] = profile;
            failed(&report, "cpuid_profile");
        }
        let mut report = good();
        report.as_object_mut().unwrap().remove("cpuid_profile");
        failed(&report, "cpuid_profile");
    }

    #[test]
    fn s4_40_a_counted_report_from_a_run_past_c7_fails() {
        // `acceptance: counted` contradicts a run that went past C7 (or does not say).
        let mut report = good();
        report["diagnostic_past_c7"] = json!(true);
        failed(&report, "acceptance_consistent");
        let mut report = good();
        report["stop"]["diagnostic_past_c7"] = json!(true);
        failed(&report, "acceptance_consistent");
        let mut report = good();
        report.as_object_mut().unwrap().remove("diagnostic_past_c7");
        failed(&report, "acceptance_consistent");
        let mut report = good();
        report["stop"]
            .as_object_mut()
            .unwrap()
            .remove("diagnostic_past_c7");
        failed(&report, "acceptance_consistent");
        // The same flags are the normal content of a diagnostic report: allowed, not counted.
        let mut report = good();
        report["acceptance"] = json!("diagnostic_not_counted");
        report["diagnostic_past_c7"] = json!(true);
        report["stop"]["diagnostic_past_c7"] = json!(true);
        let mut allowing = options();
        allowing.allow_diagnostic = true;
        let value = check_report(&report, "00", &allowing);
        assert_eq!(value["verdict"], "PASS_DIAGNOSTIC_NOT_COUNTED", "{value:#}");
        assert_eq!(value["counted"], false);
    }

    #[test]
    fn s4_40_the_expected_cpuid_profile_is_the_one_named_on_the_command_line() {
        let mut report = good();
        report["cpuid_profile"] = json!("X8664V1Stage4AApic");
        let mut apic = options();
        apic.cpuid_profile = "X8664V1Stage4AApic".to_string();
        assert_eq!(check_report(&report, "00", &apic)["verdict"], "PASS");
        // The default expectation does not accept the APIC profile, and the other way round.
        assert_eq!(check_report(&report, "00", &options())["verdict"], "FAIL");
        assert_eq!(check_report(&good(), "00", &apic)["verdict"], "FAIL");
    }

    #[test]
    fn s4_40_a_run_without_timer_interrupts_fails() {
        // The disabled IRQ route: the PIT never reached the CPU.
        let mut report = good();
        report["stop"]["irq0_edges"] = json!(0);
        report["stop"]["irqs_delivered"] = json!(0);
        failed(&report, "timer_irq");
        // Edges counted but nothing delivered (masked, shadowed, refused).
        let mut report = good();
        report["stop"]["irqs_delivered"] = json!(0);
        failed(&report, "timer_irq");
        // Delivered interrupts of another source do not stand for the timer.
        let mut report = good();
        report["stop"]["irq0_edges"] = json!(0);
        failed(&report, "timer_irq");
        // No user code retired at all.
        let mut report = good();
        report["stop"]["cpl3_retired"] = json!(0);
        failed(&report, "timer_irq");
        for field in ["irq0_edges", "irqs_delivered", "cpl3_retired"] {
            let mut report = good();
            report["stop"].as_object_mut().unwrap().remove(field);
            failed(&report, "timer_irq");
        }
    }

    #[test]
    fn s4_40_a_nanosleep_that_did_not_complete_fails() {
        // Not returned: the journal check for returns fails as well.
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][7]["rax_return"] = Value::Null;
        failed(&report, "journal_nanosleep");
        failed(&report, "journal_returns");
        // Returned with an error (EINTR, -4).
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][7]["rax_return"] = json!("0xfffffffffffffffc");
        failed(&report, "journal_nanosleep");
        // No nanosleep in the journal.
        let mut report = good();
        report["stop"]["syscall_journal"]["entries"][7]["nr"] = json!(2);
        failed(&report, "journal_nanosleep");
    }

    #[test]
    fn s4_40_a_reused_nonce_fails_and_a_fresh_one_passes() {
        let mut used = options();
        used.used_nonces = Some(vec!["s4-38l4z".to_string(), NONCE.to_string()]);
        let value = check_report(&good(), "00", &used);
        assert_eq!(value["verdict"], "FAIL", "{value:#}");
        assert!(
            value["failures"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f.as_str().unwrap().starts_with("nonce_fresh:")),
            "{value:#}"
        );
        // A similar nonce is not the same nonce.
        let mut other = options();
        other.used_nonces = Some(vec![format!("{NONCE}x"), "s4-38l4".to_string()]);
        assert_eq!(check_report(&good(), "00", &other)["verdict"], "PASS");
        // No list given: the check is reported and passes (the operator did not ask for it).
        let value = verdict(&good());
        let check = value["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "nonce_fresh")
            .unwrap();
        assert_eq!(check["ok"], true);
        assert!(
            check["detail"].as_str().unwrap().starts_with("NOT CHECKED"),
            "{check}"
        );
        assert_eq!(value["expected"]["used_nonces_checked"], false);
        assert_eq!(value["expected"]["cpuid_profile"], DEFAULT_CPUID_PROFILE);
        // With a list the verdict says so.
        let mut listed = options();
        listed.used_nonces = Some(vec!["other".to_string()]);
        let value = check_report(&good(), "00", &listed);
        assert_eq!(value["expected"]["used_nonces_checked"], true);
        assert_eq!(value["expected"]["used_nonces_count"], 1);
    }

    #[test]
    fn the_used_nonces_file_has_one_strict_format() {
        // Comments, blank lines, CRLF, surrounding spaces and a BOM are accepted.
        let text = "\u{feff}# accepted runs\r\ns4-39-run-1\r\n\r\n  s4-39-run-2  # cold2\n";
        assert_eq!(
            parse_used_nonces(text).unwrap(),
            ["s4-39-run-1", "s4-39-run-2"]
        );
        // A trailing comment must not hide the nonce (it would pass as fresh).
        assert_eq!(parse_used_nonces("s4-38l4a # cold1").unwrap(), [NONCE]);
        assert_eq!(parse_used_nonces("\u{feff}s4-38l4a").unwrap(), [NONCE]);
        assert!(parse_used_nonces("").unwrap().is_empty());
        // Anything that is not one nonce per line is an error, never a silent non-match.
        for bad in [
            "s4-38l4a,s4-x",
            "s4-38l4a;s4-x",
            "s4-38l4a s4-x",
            "s4\u{0}x",
        ] {
            assert!(parse_used_nonces(bad).is_err(), "{bad:?}");
        }
        // End to end: a reused nonce hidden behind a comment is refused by the gate.
        let mut used = options();
        used.used_nonces = Some(parse_used_nonces("s4-38l4a # cold1\n").unwrap());
        assert_eq!(check_report(&good(), "00", &used)["verdict"], "FAIL");
    }

    fn args(extra: &[&str]) -> Vec<String> {
        let hex = "ab".repeat(32);
        let mut all: Vec<String> = [
            "--report",
            "report.json",
            "--nonce",
            NONCE,
            "--expected-vmlinuz-sha256",
            &hex,
            "--expected-initramfs-sha256",
            &hex,
            "--expected-acpi-manifest-hash",
            &hex,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        all.extend(extra.iter().map(|s| s.to_string()));
        all
    }

    #[test]
    fn the_new_options_are_parsed_and_validated() {
        let options = parse(&args(&[])).unwrap();
        assert_eq!(options.cpuid_profile, DEFAULT_CPUID_PROFILE);
        assert!(options.used_nonces.is_none());

        let options = parse(&args(&["--expected-cpuid-profile", "X8664V1Stage4AApic"])).unwrap();
        assert_eq!(options.cpuid_profile, "X8664V1Stage4AApic");
        assert!(parse(&args(&["--expected-cpuid-profile", ""])).is_err());
        assert!(parse(&args(&["--expected-cpuid-profile", "a b"])).is_err());
        assert!(
            parse(&args(&[
                "--expected-cpuid-profile",
                "A",
                "--expected-cpuid-profile",
                "B"
            ]))
            .is_err()
        );

        let path = std::env::temp_dir().join(format!("od-gate-used-nonces-{}", std::process::id()));
        std::fs::write(&path, "# accepted runs\ns4-39-run-1\n\n  s4-39-run-2  \n").unwrap();
        let path_text = path.to_str().unwrap();
        let options = parse(&args(&["--used-nonces", path_text])).unwrap();
        assert_eq!(
            options.used_nonces.as_deref(),
            Some(&["s4-39-run-1".to_string(), "s4-39-run-2".to_string()][..])
        );
        std::fs::remove_file(&path).unwrap();
        // An unreadable list is an invalid argument, not an empty list (no silent skip).
        assert!(parse(&args(&["--used-nonces", path_text])).is_err());
    }
}
