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
//!   `OD_NONCE=<nonce>\n`.
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
--expected-acpi-manifest-hash <hex> [--out <json>] [--allow-diagnostic-acceptance]";

/// The syscalls of `/init` (tests/fixtures/guest/stage4/initramfs/init.S) in order.
const EXPECTED_SYSCALLS: [u64; 9] = [83, 165, 257, 0, 3, 1, 39, 35, 60];
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 60;
const PANIC_LINE: &str = "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000";

struct Options {
    report: String,
    nonce: String,
    vmlinuz_sha256: String,
    initramfs_sha256: String,
    acpi_manifest_hash: String,
    out: Option<String>,
    allow_diagnostic: bool,
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
    if nonce.is_empty()
        || nonce
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err("--nonce must be a non-empty token without whitespace".to_string());
    }
    Ok(Options {
        report: need(report, "--report")?,
        nonce,
        vmlinuz_sha256: parse_sha256(&need(vmlinuz, "--expected-vmlinuz-sha256")?)?,
        initramfs_sha256: parse_sha256(&need(initramfs, "--expected-initramfs-sha256")?)?,
        acpi_manifest_hash: parse_sha256(&need(acpi, "--expected-acpi-manifest-hash")?)?,
        out,
        allow_diagnostic,
    })
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
            "inputs": {"vmlinuz": {"sha256": VMLINUZ}, "initramfs": {"sha256": INITRAMFS}},
            "memory_sha256": {"initrd": INITRAMFS},
            "acpi": {"manifest_hash": ACPI},
            "stop": {
                "user_mode": true,
                "exit_kind": "budget_exhausted",
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
}
