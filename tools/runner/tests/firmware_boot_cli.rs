//! S7-03 end-to-end tests of `od-runner boot-firmware` on synthetic `ODFWIMG1` images.

use od_testkit::sha256_hex;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const IMAGE_BYTES: usize = 0x40_0000;
static NEXT: AtomicU64 = AtomicU64::new(0);

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Header per FIRMWARE_BOOT.md 3.2; code at `entry_offset = 0x40`.
fn image(code: &[u8]) -> Vec<u8> {
    let mut image = vec![0u8; IMAGE_BYTES];
    image[..8].copy_from_slice(b"ODFWIMG1");
    image[0x08..0x0C].copy_from_slice(&1u32.to_le_bytes());
    image[0x0C..0x10].copy_from_slice(&0x40u32.to_le_bytes());
    image[0x10..0x18].copy_from_slice(&(IMAGE_BYTES as u64).to_le_bytes());
    image[0x18..0x20].copy_from_slice(&0x0020_0000u64.to_le_bytes());
    image[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
    image[0x28..0x2A].copy_from_slice(&1u16.to_le_bytes());
    image[0x40..0x40 + code.len()].copy_from_slice(code);
    let crc = crc32(&image);
    image[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    image
}

struct Run {
    output: Output,
    com1: PathBuf,
}

fn invoke(image: &[u8], budget: &str) -> Run {
    invoke_with_args(image, budget, &[])
}

fn invoke_with_args(image: &[u8], budget: &str, extra: &[&str]) -> Run {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir();
    let bin = dir.join(format!("od-runner-fw-{}-{id}.bin", std::process::id()));
    let com1 = dir.join(format!("od-runner-fw-{}-{id}.com1", std::process::id()));
    let _ = fs::remove_file(&com1);
    fs::write(&bin, image).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["boot-firmware", "--image"])
        .arg(&bin)
        .arg("--com1-out")
        .arg(&com1)
        .args(["--budget", budget])
        .args(extra)
        .output()
        .unwrap();
    fs::remove_file(&bin).unwrap();
    Run { output, com1 }
}

#[test]
fn fv_app_rejects_invalid_selection_before_launch() {
    let legacy = image(&[0xf4]);
    for (extra, error) in [
        (
            &["--fv-app", "other"][..],
            "expected hello, ebs, vars, rt, acpi, gop, blk, fs, tpm or reboot",
        ),
        (
            &["--fv-app", "hello", "--fv-app", "ebs"][..],
            "duplicate option --fv-app",
        ),
        (
            &["--fv-app", "blk"][..],
            "--fv-app blk requires --nvme-image",
        ),
        (&["--fv-app", "fs"][..], "--fv-app fs requires --nvme-image"),
    ] {
        let run = invoke_with_args(&legacy, "64", extra);
        assert_eq!(run.output.status.code(), Some(3));
        let result = report(&run);
        assert_eq!(result["status"], "invalid_input");
        assert!(result["error"]["detail"].as_str().unwrap().contains(error));
        assert!(!run.com1.exists());
    }
}

#[test]
fn storage_app_requires_a_compatible_rom_and_rejects_nvme_output_alias() {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("od-runner-disk-{}-{id}.raw", std::process::id()));
    let sector = [0xa5; 512];
    fs::write(&path, sector).unwrap();
    for (app, max_minor) in [("blk", 10u16), ("fs", 11)] {
        let mut rom = image(&[0xf4]);
        rom[0x2a..0x2c].copy_from_slice(&max_minor.to_le_bytes());
        rom[0x30..0x34].fill(0);
        let crc = crc32(&rom);
        rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
        let run = invoke_with_args(
            &rom,
            "64",
            &["--fv-app", app, "--nvme-image", path.to_str().unwrap()],
        );
        assert_eq!(run.output.status.code(), Some(4));
        let report = report(&run);
        assert_eq!(report["status"], "loader_error");
        assert!(
            report["error"]["detail"]
                .as_str()
                .unwrap()
                .contains("handoff_minor_max")
        );
        assert!(!run.com1.exists());
    }

    let output = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args([
            "boot-firmware",
            "--image",
            "unused",
            "--budget",
            "64",
            "--fv-app",
            "blk",
            "--nvme-image",
        ])
        .arg(&path)
        .arg("--com1-out")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["kind"], "paths");
    assert_eq!(fs::read(&path).unwrap(), sector);
    fs::remove_file(path).unwrap();
}

#[test]
fn fv_app_requires_a_compatible_firmware_before_launch() {
    let legacy = image(&[0xf4]);
    for app in ["ebs", "rt"] {
        let run = invoke_with_args(&legacy, "64", &["--fv-app", app]);
        assert_eq!(run.output.status.code(), Some(4));
        let result = report(&run);
        assert_eq!(result["status"], "loader_error");
        assert!(
            result["error"]["detail"]
                .as_str()
                .unwrap()
                .contains("handoff_minor_max")
        );
        assert!(!run.com1.exists());
    }
}

#[test]
fn nvram_snapshot_round_trips_and_cannot_alias_input() {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("od-runner-nvram-{}-{id}", std::process::id()));
    let input = base.with_extension("in");
    let output = base.with_extension("out");
    let snapshot = od_machine::NvramDevice::new().nvram_snapshot();
    fs::write(&input, &snapshot).unwrap();
    let mut rom = image(&[0xf4]);
    rom[0x2a..0x2c].copy_from_slice(&7u16.to_le_bytes());
    rom[0x30..0x34].fill(0);
    let crc = crc32(&rom);
    rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());

    let input_path = input.to_str().unwrap();
    let valid = invoke_with_args(
        &rom,
        "64",
        &[
            "--nvram-in",
            input_path,
            "--nvram-out",
            output.to_str().unwrap(),
        ],
    );
    assert_eq!(valid.output.status.code(), Some(0));
    assert_eq!(
        report(&valid)["nvram_snapshot_sha256"],
        sha256_hex(&snapshot)
    );
    assert_eq!(fs::read(&output).unwrap(), snapshot);
    let alias = invoke_with_args(
        &rom,
        "64",
        &["--nvram-in", input_path, "--nvram-out", input_path],
    );
    assert_eq!(alias.output.status.code(), Some(3));
    assert_eq!(report(&alias)["error"]["kind"], "paths");
    assert_eq!(fs::read(&input).unwrap(), snapshot);
    assert!(!alias.com1.exists());

    fs::write(&input, b"short").unwrap();
    let truncated = invoke_with_args(&rom, "64", &["--nvram-in", input_path]);
    assert_eq!(truncated.output.status.code(), Some(3));
    assert_eq!(report(&truncated)["error"]["kind"], "nvram_in");
    assert!(!truncated.com1.exists());
    fs::remove_file(valid.com1).unwrap();
    fs::remove_file(input).unwrap();
    fs::remove_file(&output).unwrap();

    let alternate = output.with_extension("OUT");
    fs::write(&output, b"probe").unwrap();
    let case_insensitive = alternate.exists();
    fs::remove_file(&output).unwrap();
    if case_insensitive {
        let alias = invoke_with_args(
            &rom,
            "64",
            &[
                "--fv-app",
                "vars",
                "--nvram-out",
                output.to_str().unwrap(),
                "--nvram-reboot-out",
                alternate.to_str().unwrap(),
            ],
        );
        assert_eq!(alias.output.status.code(), Some(3));
        assert_eq!(report(&alias)["error"]["kind"], "paths");
        assert_eq!(fs::read(&output).unwrap(), snapshot);
        fs::remove_file(alias.com1).unwrap();
        fs::remove_file(output).unwrap();
    }
}

fn report(run: &Run) -> Value {
    serde_json::from_slice(&run.output.stdout).unwrap()
}

#[test]
fn halting_image_reports_hashes_steps_and_writes_an_empty_transcript() {
    let image = image(&[0xf4]);
    let run = invoke(&image, "100");
    assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    let value = report(&run);
    assert_eq!(value["status"], "completed");
    assert_eq!(value["exit"], "halted");
    assert_eq!(value["retired"], 1);
    assert_eq!(value["quanta"], 1);
    assert_eq!(value["virtual_ns"], 1);
    assert_eq!(value["tsc_hz"], 1_000_000_000u64);
    assert_eq!(value["image_sha256"], sha256_hex(&image));
    assert_eq!(value["handoff_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(value["handoff_bytes"], 0x370);
    assert_eq!(value["rip"], "0x0000000000200041");
    assert_eq!(value["com1_bytes"], 0);
    assert!(value["fw_stop"].is_null());
    assert_eq!(fs::read(&run.com1).unwrap(), b"");
    fs::remove_file(&run.com1).unwrap();
}

#[test]
fn exhausted_budget_is_an_error_and_still_writes_the_transcript() {
    // Four `MOV RAX,[RBX]` (RBX = 0, identity-mapped RAM) then HLT; budget 2 stops first.
    let code = [
        0x48, 0x8b, 0x03, 0x48, 0x8b, 0x03, 0x48, 0x8b, 0x03, 0x48, 0x8b, 0x03, 0xf4,
    ];
    let run = invoke(&image(&code), "2");
    assert_eq!(run.output.status.code(), Some(5), "{:?}", run.output);
    let value = report(&run);
    assert_eq!(value["status"], "error");
    assert_eq!(value["exit"], "budget_exhausted");
    assert_eq!(value["retired"], 2);
    assert_eq!(value["virtual_ns"], 2);
    assert_eq!(fs::read(&run.com1).unwrap(), b"");
    fs::remove_file(&run.com1).unwrap();
}

#[test]
fn smoke_code_prints_x_and_halts() {
    // `build_firmware.py --smoke` code: mov al,'X'; mov dx,0x3F8; out dx,al; hlt.
    let run = invoke(
        &image(&[0xb0, 0x58, 0x66, 0xba, 0xf8, 0x03, 0xee, 0xf4]),
        "16",
    );
    assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    let value = report(&run);
    assert_eq!(value["exit"], "halted");
    assert_eq!(value["retired"], 4);
    assert_eq!(value["com1_bytes"], 1);
    assert_eq!(fs::read(&run.com1).unwrap(), b"X");
    fs::remove_file(&run.com1).unwrap();
}

#[test]
fn invalid_images_are_rejected_before_the_guest_runs_and_no_transcript_is_written() {
    let mut corrupt = image(&[0xf4]);
    corrupt[0x1000] ^= 1;
    for bad in [vec![0u8; IMAGE_BYTES - 1], corrupt] {
        let run = invoke(&bad, "10");
        assert_eq!(run.output.status.code(), Some(4), "{:?}", run.output);
        let value = report(&run);
        assert_eq!(value["status"], "loader_error");
        assert!(!run.com1.exists());
    }
}

#[test]
fn missing_arguments_and_files_exit_with_invalid_input() {
    let missing = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["boot-firmware", "--image", "x"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(3));
    let no_file = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args([
            "boot-firmware",
            "--image",
            "/nonexistent/od-fw.bin",
            "--com1-out",
            "/nonexistent/out",
            "--budget",
            "1",
        ])
        .output()
        .unwrap();
    assert_eq!(no_file.status.code(), Some(3));
}

fn scratch(tag: &str) -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("od-runner-{tag}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn stop_snapshot_describes_a_faulting_stop_and_the_run_inputs() {
    // UD2 at the entry: the machine stops with #UD. The entry page holds no PE image.
    let rom = image(&[0x0f, 0x0b]);
    let dir = scratch("snapshot");
    let snapshot = dir.join("stop.json");
    let run = invoke_with_args(
        &rom,
        "100",
        &[
            "--stop-snapshot",
            snapshot.to_str().unwrap(),
            "--peek",
            "200040:2",
        ],
    );
    assert_eq!(run.output.status.code(), Some(5), "{:?}", run.output);
    let value = report(&run);
    assert_eq!(value["exit"], "fault");
    assert_eq!(value["stop_snapshot"]["written"], true);

    let text = fs::read_to_string(&snapshot).unwrap();
    let snap: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(snap["kind"], "stop_snapshot");
    assert_eq!(snap["reason"]["exit"], "fault");
    assert!(
        snap["reason"]["exit_detail"]
            .as_str()
            .unwrap()
            .contains("vector: 6")
    );
    assert_eq!(snap["rip"], "0x0000000000200040");
    assert_eq!(snap["rip_bytes"]["hex"].as_str().unwrap()[..4], *"0f0b");
    // `--peek 200040:2` reads the same two bytes into the snapshot file.
    assert_eq!(snap["peeks"][0]["address"], "0x0000000000200040");
    assert_eq!(snap["peeks"][0]["hex"], "0f0b");
    assert_eq!(snap["peeks"][0]["read"], 2);
    assert_eq!(
        snap["module"]["found"], false,
        "no PE header below the entry"
    );
    assert_eq!(snap["counters"]["retired"], 0);
    assert_eq!(snap["inputs"]["image_sha256"], sha256_hex(&rom));
    assert_eq!(snap["inputs"]["handoff_sha256"], value["handoff_sha256"]);
    assert_eq!(snap["inputs"]["budget"], 100);
    for field in ["cr0", "cr3", "cr4", "efer", "rflags"] {
        assert!(snap[field].is_string(), "{field}");
    }
    assert_eq!(snap["gpr"].as_object().unwrap().len(), 16);
    assert_eq!(snap["stack"]["slots"].as_array().unwrap().len(), 8);

    // The same run without the flag stops identically and writes no file.
    let plain = invoke(&rom, "100");
    assert_eq!(plain.output.status.code(), Some(5));
    assert!(report(&plain).get("stop_snapshot").is_none());
    // A halt is a stop as well; only the machine's Shutdown is skipped.
    fs::remove_file(&snapshot).unwrap();
    let halted = invoke_with_args(
        &image(&[0xf4]),
        "100",
        &["--stop-snapshot", snapshot.to_str().unwrap()],
    );
    assert_eq!(halted.output.status.code(), Some(0));
    assert_eq!(report(&halted)["stop_snapshot"]["written"], true);
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&snapshot).unwrap()).unwrap()["reason"]["exit"],
        "halted"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn diagnostic_options_are_validated_before_launch() {
    let rom = image(&[0xf4]);
    let dir = scratch("diag-args");
    let (snap, phases, shots) = (
        dir.join("stop.json"),
        dir.join("phases.jsonl"),
        dir.join("shots"),
    );
    let (snap, phases, shots) = (
        snap.to_str().unwrap(),
        phases.to_str().unwrap(),
        shots.to_str().unwrap(),
    );
    for (extra, error) in [
        (&["--framebuffer-every", "1"][..], "go together"),
        (&["--framebuffer-dir", shots][..], "go together"),
        (&["--framebuffer-max-files", "3"][..], "go together"),
        (
            &["--framebuffer-every", "0", "--framebuffer-dir", shots][..],
            "invalid --framebuffer-every",
        ),
        (
            &["--framebuffer-every", "nan", "--framebuffer-dir", shots][..],
            "invalid --framebuffer-every",
        ),
        (
            &[
                "--framebuffer-every",
                "1",
                "--framebuffer-dir",
                shots,
                "--framebuffer-max-files",
                "0",
            ][..],
            "invalid --framebuffer-max-files",
        ),
        (
            &["--stop-snapshot", snap, "--sample-rip", "200040"][..],
            "cannot be combined with --stop-snapshot",
        ),
        (
            &[
                "--phase-log",
                phases,
                "--fv-app",
                "vars",
                "--nvram-reboot-out",
                "second.com1",
            ][..],
            "describe one boot",
        ),
        (
            &["--stop-snapshot", snap, "--stop-snapshot", snap][..],
            "duplicate option",
        ),
        (&["--peek", "1000"][..], "--peek requires --stop-snapshot"),
        (
            &["--stop-snapshot", snap, "--peek", "1000:0"][..],
            "invalid --peek",
        ),
    ] {
        let run = invoke_with_args(&rom, "10", extra);
        assert_eq!(run.output.status.code(), Some(3), "{extra:?}");
        let detail = report(&run)["error"]["detail"].as_str().unwrap().to_owned();
        assert!(detail.contains(error), "{extra:?}: {detail}");
        assert!(!run.com1.exists());
    }
    // The phase log must not be the dump directory or a file inside it (an empty one exists).
    fs::create_dir_all(shots).unwrap();
    for (phase_log, label) in [
        (format!("{shots}/phases.jsonl"), "inside"),
        (shots.to_owned(), "equal"),
    ] {
        let run = invoke_with_args(
            &rom,
            "10",
            &[
                "--framebuffer-every",
                "1",
                "--framebuffer-dir",
                shots,
                "--phase-log",
                &phase_log,
            ],
        );
        assert_eq!(run.output.status.code(), Some(3), "{label}");
        let detail = report(&run)["error"]["detail"].as_str().unwrap().to_owned();
        assert!(detail.contains("--framebuffer-dir"), "{label}: {detail}");
        assert_eq!(fs::read_dir(shots).unwrap().count(), 0, "{label}");
        assert!(!run.com1.exists());
    }
    let run = invoke_with_args(
        &rom,
        "10",
        &[
            "--framebuffer-every",
            "1",
            "--framebuffer-dir",
            shots,
            "--stop-snapshot",
            &format!("{shots}/stop.json"),
        ],
    );
    assert_eq!(run.output.status.code(), Some(3));

    // A run refused after the paths are validated leaves neither a phase log nor a dump
    // directory behind: the image is in an existing directory and passes the path checks, then it
    // is unreadable (exit 3, image_file) or refused by the loader (exit 4, loader).
    let (late_log, late_shots) = (dir.join("late.jsonl"), dir.join("late-shots"));
    let short = dir.join("short.bin");
    fs::write(&short, [0u8; 16]).unwrap();
    for (image_path, code, kind) in [
        (dir.join("absent.bin"), 3, "image_file"),
        (short, 4, "loader"),
    ] {
        let refused = Command::new(env!("CARGO_BIN_EXE_od-runner"))
            .args(["boot-firmware", "--image"])
            .arg(&image_path)
            .arg("--com1-out")
            .arg(dir.join("late.com1"))
            .args(["--budget", "1", "--phase-log"])
            .arg(&late_log)
            .args(["--framebuffer-every", "1", "--framebuffer-dir"])
            .arg(&late_shots)
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(code), "{kind}: {refused:?}");
        let value: Value = serde_json::from_slice(&refused.stdout).unwrap();
        assert_eq!(value["error"]["kind"], kind, "{value}");
        assert!(
            !late_log.exists(),
            "{kind}: the phase log was created by a refused run"
        );
        assert!(
            !late_shots.exists(),
            "{kind}: the dump directory was created by a refused run"
        );
    }

    // A dump directory under missing parents is accepted and created, with and without a phase
    // log (the same judgement in both cases); a phase log inside it is refused up front, since
    // the log's own directory does not exist.
    let (deep_with_log, deep_plain) = (dir.join("a/x/y/shots"), dir.join("b/x/y/shots"));
    let ok_log = dir.join("ok.jsonl");
    for (deep, log) in [(&deep_with_log, Some(&ok_log)), (&deep_plain, None)] {
        let mut extra = vec!["--framebuffer-every", "1", "--framebuffer-dir"];
        extra.push(deep.to_str().unwrap());
        if let Some(log) = log {
            extra.extend(["--phase-log", log.to_str().unwrap()]);
        }
        let run = invoke_with_args(&rom, "10", &extra);
        assert_eq!(
            run.output.status.code(),
            Some(0),
            "{extra:?}: {:?}",
            run.output
        );
        assert!(deep.is_dir(), "{extra:?}");
    }
    assert!(ok_log.exists());
    let nested_log = dir.join("c/x/shots/phases.jsonl");
    let run = invoke_with_args(
        &rom,
        "10",
        &[
            "--framebuffer-every",
            "1",
            "--framebuffer-dir",
            dir.join("c/x/shots").to_str().unwrap(),
            "--phase-log",
            nested_log.to_str().unwrap(),
        ],
    );
    assert_eq!(run.output.status.code(), Some(3));
    assert_eq!(report(&run)["error"]["kind"], "paths");
    assert!(!dir.join("c").exists());

    // The snapshot and the phase log must not be the same file, nor the transcript.
    let same = invoke_with_args(&rom, "10", &["--stop-snapshot", snap, "--phase-log", snap]);
    assert_eq!(same.output.status.code(), Some(3));
    assert_eq!(report(&same)["error"]["kind"], "paths");
    assert!(!std::path::Path::new(snap).exists());
    fs::remove_dir_all(dir).unwrap();
}

/// The unified loop runs through the turbo engine by default; `--no-turbo` (the general step
/// only) gives the same report, and `--turbo-stats` adds only the statistics of the engine on
/// stderr. A flag of the removed acceleration engine is refused when the arguments are parsed.
#[test]
fn the_unified_run_without_turbo_reports_the_same_and_turbo_stats_go_to_stderr() {
    #[rustfmt::skip]
    let code = [
        0xB9, 0x32, 0x00, 0x00, 0x00, // mov ecx, 50
        0x48, 0x01, 0xCA,             // L: add rdx, rcx
        0x48, 0x89, 0xD0,             // mov rax, rdx
        0xFF, 0xC9,                   // dec ecx
        0x75, 0xF6,                   // jnz L
        0xF4,                         // hlt
    ];
    // The unified loop's board gives a newer handoff: the image declares that it takes any minor.
    let mut rom = image(&code);
    rom[0x2A..0x2C].copy_from_slice(&u16::MAX.to_le_bytes());
    rom[0x30..0x34].fill(0);
    let crc = crc32(&rom);
    rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    let unified = ["--cpu-loop", "unified"];
    let default = invoke_with_args(&rom, "10000", &unified);
    let general = invoke_with_args(&rom, "10000", &[&unified[..], &["--no-turbo"]].concat());
    let counted = invoke_with_args(&rom, "10000", &[&unified[..], &["--turbo-stats"]].concat());
    for run in [&default, &general, &counted] {
        assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    }
    let stderr = String::from_utf8_lossy(&counted.output.stderr);
    assert!(stderr.contains("turbo: "), "{stderr}");
    // On a host with the engine the run really went through it (else the comparison below would
    // compare the general step with itself).
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        let retired = stderr
            .lines()
            .find(|line| line.starts_with("turbo: TurboStats"))
            .and_then(|line| line.split("retired: ").nth(1))
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|digits| digits.parse::<u64>().ok());
        assert!(
            retired.is_some_and(|retired| retired > 0),
            "the turbo engine retired nothing: {stderr}"
        );
    }
    let (mut default_report, mut general_report, mut counted_report) =
        (report(&default), report(&general), report(&counted));
    assert_eq!(default_report["exit"], "halted");
    assert_eq!(default_report["cpu_loop"], "unified");
    // The COM1 path differs between invocations by construction.
    for report in [
        &mut default_report,
        &mut general_report,
        &mut counted_report,
    ] {
        report.as_object_mut().unwrap().remove("com1_out");
    }
    assert_eq!(
        default_report, general_report,
        "the engine changes no field"
    );
    assert_eq!(default_report, counted_report);
    for flag in [
        "--fast-path",
        "--no-block-cache",
        "--block-stats",
        "--data-cache-prototype",
    ] {
        let refused = invoke_with_args(&rom, "10000", &[&unified[..], &[flag]].concat());
        assert_eq!(
            refused.output.status.code(),
            Some(3),
            "{flag}: {:?}",
            refused.output
        );
        assert_eq!(report(&refused)["status"], "invalid_input");
        assert!(!refused.com1.exists());
    }
    for run in [default, general, counted] {
        fs::remove_file(&run.com1).unwrap();
    }
}

/// S7-32: `--guest windows` gives the unified run the Windows CPUID profile (vendor `GenuineIntel`
/// in EBX of leaf 0, `OpenDesktop ` for the default Linux guest), `--ram-mib` sets the RAM; the report
/// records both. Invalid values and `--guest` without the unified loop are refused before launch.
#[test]
fn the_unified_guest_selects_the_cpuid_profile_and_the_report_records_guest_and_ram() {
    #[rustfmt::skip]
    let code = [
        0x31, 0xC0,       // xor eax, eax
        0x0F, 0xA2,       // cpuid
        0x89, 0xD8,       // mov eax, ebx
        0xF4,             // hlt
    ];
    let mut rom = image(&code);
    rom[0x2A..0x2C].copy_from_slice(&u16::MAX.to_le_bytes());
    rom[0x30..0x34].fill(0);
    let crc = crc32(&rom);
    rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    let unified = ["--cpu-loop", "unified"];
    let windows = invoke_with_args(
        &rom,
        "100",
        &[&unified[..], &["--guest", "windows", "--ram-mib", "256"]].concat(),
    );
    let linux = invoke_with_args(&rom, "100", &unified);
    for run in [&windows, &linux] {
        assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    }
    let (windows_report, linux_report) = (report(&windows), report(&linux));
    // "Genu" and "Open" as little-endian dwords (CPUID.0:EBX).
    assert_eq!(
        windows_report["unified"]["final_state"]["rax"],
        "0x756e6547"
    );
    assert_eq!(linux_report["unified"]["final_state"]["rax"], "0x6e65704f");
    assert_eq!(windows_report["guest"], "windows");
    assert_eq!(linux_report["guest"], "linux");
    assert_eq!(windows_report["ram_bytes"], 256u64 << 20);
    assert_eq!(linux_report["ram_bytes"], 128u64 << 20);
    let members = windows_report["unified"]["members"].as_array().unwrap();
    assert!(
        members
            .iter()
            .any(|member| member == "cpuid:OdX8664V2Windows"),
        "{members:?}"
    );
    for run in [windows, linux] {
        fs::remove_file(&run.com1).unwrap();
    }
    for extra in [
        &["--guest", "windows"][..],
        &["--cpu-loop", "unified", "--guest", "macos"],
        &["--ram-mib", "3"],
        &["--ram-mib", "0"],
        &["--ram-mib", "17592186044544"],
    ] {
        let refused = invoke_with_args(&rom, "100", extra);
        assert_eq!(
            refused.output.status.code(),
            Some(3),
            "{extra:?}: {:?}",
            refused.output
        );
        assert_eq!(report(&refused)["status"], "invalid_input", "{extra:?}");
        assert!(!refused.com1.exists(), "{extra:?}");
    }
}

/// S7-32 diagnostic: `--device-errors stop` is the default and changes no field of the report;
/// `--device-errors log` completes the refused port accesses as an open bus, marks the report as
/// a diagnostic run and lists the groups; `log:<n>` stops on the refusal after the n-th; the stop
/// snapshot of a log run names the mode in its inputs. The option is checked before launch: only
/// with the unified loop, once, and with a limit from 1 to 10^9.
#[test]
fn device_errors_log_is_a_diagnostic_of_the_unified_loop_and_stop_changes_nothing() {
    #[rustfmt::skip]
    let code = [
        0x66, 0xBA, 0x34, 0x12, // mov dx, 0x1234 (no device)
        0xEC,                   // in al, dx
        0xEE,                   // out dx, al
        0xF4,                   // hlt
    ];
    let mut rom = image(&code);
    rom[0x2A..0x2C].copy_from_slice(&u16::MAX.to_le_bytes());
    rom[0x30..0x34].fill(0);
    let crc = crc32(&rom);
    rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    let unified = ["--cpu-loop", "unified"];
    let with = |extra: &[&str]| invoke_with_args(&rom, "100", &[&unified[..], extra].concat());
    let default = with(&[]);
    let stop = with(&["--device-errors", "stop"]);
    for run in [&default, &stop] {
        assert_eq!(run.output.status.code(), Some(5), "{:?}", run.output);
    }
    let (mut default_report, mut stop_report) = (report(&default), report(&stop));
    assert_eq!(default_report["exit"], "pio_error");
    for report in [&mut default_report, &mut stop_report] {
        report.as_object_mut().unwrap().remove("com1_out");
        assert!(report.get("device_errors").is_none());
        assert!(report.get("diagnostic_run").is_none());
    }
    assert_eq!(default_report, stop_report, "stop is the default");

    let log = with(&["--device-errors", "log"]);
    // A run that absorbed a refusal is not a completed run: its own status and exit code 7.
    assert_eq!(log.output.status.code(), Some(7), "{:?}", log.output);
    let log_report = report(&log);
    assert_eq!(
        log_report["status"],
        "completed_with_absorbed_device_errors"
    );
    assert_eq!(log_report["exit"], "halted");
    assert_eq!(log_report["retired"], 4);
    assert_eq!(log_report["diagnostic_run"], true);
    let errors = &log_report["device_errors"];
    assert_eq!(errors["mode"], "log");
    assert_eq!(errors["diagnostic"], true);
    assert_eq!(errors["compatibility_result"], false);
    assert_eq!(errors["event_limit"], 100_000);
    assert_eq!(errors["events"], 2);
    assert_eq!(errors["stopped"], Value::Null);
    let groups = errors["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{groups:?}");
    for (group, direction, retired, value) in [
        (&groups[0], "read", 1, Value::Null),
        (&groups[1], "write", 2, Value::from("0xff")),
    ] {
        assert_eq!(group["space"], "pio");
        assert_eq!(group["device_index"], Value::Null);
        assert_eq!(group["region_base"], Value::Null);
        assert_eq!(group["address"], "0x1234");
        assert_eq!(group["direction"], direction);
        assert_eq!(group["width"], 1);
        assert_eq!(group["kind"], "UnmappedPort");
        assert_eq!(group["count"], 1);
        assert_eq!(group["first_retired"], retired);
        assert_eq!(group["last_retired"], retired);
        assert_eq!(group["first_cpl"], 0);
        assert_eq!(group["first_value"], value);
    }
    assert_eq!(groups[0]["first_rip"], "0x200044");
    assert_eq!(log_report["unified"]["final_state"]["rax"], "0xff");

    let dir = scratch("device-errors");
    let snapshot = dir.join("stop.json");
    let snapshot_arg = snapshot.to_str().unwrap();
    let limited = with(&["--device-errors", "log:1", "--stop-snapshot", snapshot_arg]);
    assert_eq!(
        limited.output.status.code(),
        Some(5),
        "{:?}",
        limited.output
    );
    let limited_report = report(&limited);
    assert_eq!(limited_report["exit"], "pio_error");
    assert_eq!(limited_report["retired"], 2, "stopped at the write");
    assert_eq!(limited_report["device_errors"]["events"], 1);
    assert_eq!(limited_report["device_errors"]["stopped"], "event_limit");
    let snap: Value = serde_json::from_str(&fs::read_to_string(&snapshot).unwrap()).unwrap();
    assert_eq!(snap["inputs"]["device_errors"], "log:1");
    fs::remove_file(&snapshot).unwrap();
    let plain = with(&["--stop-snapshot", snapshot_arg]);
    let snap: Value = serde_json::from_str(&fs::read_to_string(&snapshot).unwrap()).unwrap();
    assert!(snap["inputs"].get("device_errors").is_none(), "{snap}");
    for run in [default, stop, log, limited, plain] {
        fs::remove_file(&run.com1).unwrap();
    }
    fs::remove_dir_all(dir).unwrap();

    for extra in [
        &["--device-errors", "log"][..],
        &["--cpu-loop", "unified", "--device-errors", "log:0"],
        &["--cpu-loop", "unified", "--device-errors", "log:"],
        &["--cpu-loop", "unified", "--device-errors", "log:x"],
        &["--cpu-loop", "unified", "--device-errors", "log:+5"],
        &["--cpu-loop", "unified", "--device-errors", "log:1000000001"],
        &["--cpu-loop", "unified", "--device-errors", "ignore"],
        &[
            "--cpu-loop",
            "unified",
            "--device-errors",
            "stop",
            "--device-errors",
            "log",
        ],
        &["--cpu-loop", "unified", "--device-errors"],
    ] {
        let refused = invoke_with_args(&rom, "100", extra);
        assert_eq!(
            refused.output.status.code(),
            Some(3),
            "{extra:?}: {:?}",
            refused.output
        );
        assert_eq!(report(&refused)["status"], "invalid_input", "{extra:?}");
        assert!(!refused.com1.exists(), "{extra:?}");
    }
    // `stop` is the default, so it is accepted with either loop.
    let firmware = invoke_with_args(&image(&[0xf4]), "100", &["--device-errors", "stop"]);
    assert_eq!(
        firmware.output.status.code(),
        Some(0),
        "{:?}",
        firmware.output
    );
    fs::remove_file(&firmware.com1).unwrap();

    // A log run that absorbed nothing completes as usual, still marked as a diagnostic.
    let mut halting = image(&[0xf4]);
    halting[0x2A..0x2C].copy_from_slice(&u16::MAX.to_le_bytes());
    halting[0x30..0x34].fill(0);
    let crc = crc32(&halting);
    halting[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    let clean = invoke_with_args(
        &halting,
        "100",
        &[&unified[..], &["--device-errors", "log"]].concat(),
    );
    assert_eq!(clean.output.status.code(), Some(0), "{:?}", clean.output);
    let clean_report = report(&clean);
    assert_eq!(clean_report["status"], "completed");
    assert_eq!(clean_report["diagnostic_run"], true);
    assert_eq!(clean_report["device_errors"]["events"], 0);
    fs::remove_file(&clean.com1).unwrap();
}

/// S7-32 diagnostic: a `--device-errors log` run whose output fails after the boot (here the
/// `--framebuffer-out` and `--nvram-out` paths are directories) keeps the diagnostic mark and
/// the log in the failure report; the same failure of a `stop` run has neither field.
#[test]
fn a_failure_after_a_device_errors_log_boot_keeps_the_diagnostic_mark() {
    #[rustfmt::skip]
    let code = [
        0x66, 0xBA, 0x34, 0x12, // mov dx, 0x1234 (no device)
        0xEC,                   // in al, dx
        0xF4,                   // hlt
    ];
    let mut rom = image(&code);
    rom[0x2A..0x2C].copy_from_slice(&u16::MAX.to_le_bytes());
    rom[0x30..0x34].fill(0);
    let crc = crc32(&rom);
    rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    let dir = scratch("device-errors-out");
    let target = dir.to_str().unwrap();
    for (option, kind) in [
        ("--framebuffer-out", "framebuffer_out"),
        ("--nvram-out", "nvram_out"),
    ] {
        for mode in ["log", "stop"] {
            let run = invoke_with_args(
                &rom,
                "100",
                &[
                    "--cpu-loop",
                    "unified",
                    "--device-errors",
                    mode,
                    option,
                    target,
                ],
            );
            assert_eq!(
                run.output.status.code(),
                Some(5),
                "{option} {mode}: {:?}",
                run.output
            );
            let value = report(&run);
            assert_eq!(value["status"], "run_error", "{option} {mode}: {value}");
            assert_eq!(value["error"]["kind"], kind, "{option} {mode}: {value}");
            if mode == "log" {
                assert_eq!(value["diagnostic_run"], true, "{option}: {value}");
                assert_eq!(value["device_errors"]["events"], 1, "{option}: {value}");
                assert_eq!(value["device_errors"]["groups"][0]["kind"], "UnmappedPort");
            } else {
                assert!(value.get("diagnostic_run").is_none(), "{option}: {value}");
                assert!(value.get("device_errors").is_none(), "{option}: {value}");
            }
            let _ = fs::remove_file(&run.com1);
        }
    }
    fs::remove_dir_all(dir).unwrap();
}

/// A temporary file removed when the test ends, also when an assertion fails.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// S7-32: `--nvme-pci` needs the unified loop and an NVMe source; the refusals come before launch.
/// With both, a unified run puts the controller on PCI and the report says so.
#[test]
fn nvme_pci_needs_the_unified_loop_and_an_nvme_source() {
    let disk = TempFile(
        std::env::temp_dir().join(format!("od-runner-nvme-pci-{}.raw", std::process::id())),
    );
    fs::write(&disk.0, vec![0u8; 1 << 20]).unwrap();
    let disk_arg = disk.0.to_str().unwrap();
    let rom = image(&[0xf4]);
    for extra in [
        &["--nvme-pci"][..],
        &["--nvme-pci", "--disk", disk_arg],
        &["--cpu-loop", "unified", "--nvme-pci"],
    ] {
        let refused = invoke_with_args(&rom, "100", extra);
        assert_eq!(
            refused.output.status.code(),
            Some(3),
            "{extra:?}: {:?}",
            refused.output
        );
        let value = report(&refused);
        assert_eq!(value["status"], "invalid_input", "{extra:?}");
        assert!(
            value["error"]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("--nvme-pci")),
            "{extra:?}: {value}"
        );
        assert!(!refused.com1.exists(), "{extra:?}");
    }
    // The unified loop's board gives a newer handoff: the image declares that it takes any minor.
    let mut unified_rom = image(&[0xf4]);
    unified_rom[0x2A..0x2C].copy_from_slice(&u16::MAX.to_le_bytes());
    unified_rom[0x30..0x34].fill(0);
    let crc = crc32(&unified_rom);
    unified_rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    let run = invoke_with_args(
        &unified_rom,
        "100",
        &[
            "--cpu-loop",
            "unified",
            "--nvme-pci",
            "--nvme-image",
            disk_arg,
        ],
    );
    assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    let value = report(&run);
    assert_eq!(value["exit"], "halted", "{value}");
    assert_eq!(value["nvme_pci"], true, "{value}");
    fs::remove_file(&run.com1).unwrap();
}

/// MOV RBX,imm64; MOV DWORD [RBX],imm32 (the guest-side register or pixel write).
fn store_dword(code: &mut Vec<u8>, address: u64, value: u32) {
    code.extend_from_slice(&[0x48, 0xbb]);
    code.extend_from_slice(&address.to_le_bytes());
    code.extend_from_slice(&[0xc7, 0x03]);
    code.extend_from_slice(&value.to_le_bytes());
}

/// A guest that commits a 4x4 mode, draws one pixel and rings the doorbell, prints "A", draws a
/// different pixel and rings again, prints "B" and "C", halts. Pixel 0 of the two frames: AABBCCDD
/// and 44332211 (little-endian dwords).
fn two_frame_program() -> Vec<u8> {
    const REGISTERS: u64 = 0xfc00_0000;
    const FRAMEBUFFER: u64 = 0xf800_0000;
    let mut code = vec![0x66, 0xba, 0xf8, 0x03];
    for (register, value) in [(0x40, 4), (0x44, 4), (0x48, 1), (0x4c, 1)] {
        store_dword(&mut code, REGISTERS + register, value);
    }
    store_dword(&mut code, FRAMEBUFFER, 0xddcc_bbaa);
    store_dword(&mut code, REGISTERS + 0x80, 0);
    code.extend_from_slice(&[0xb0, b'A', 0xee]);
    store_dword(&mut code, FRAMEBUFFER, 0x4433_2211);
    store_dword(&mut code, REGISTERS + 0x80, 0);
    code.extend_from_slice(&[0xb0, b'B', 0xee, 0xb0, b'C', 0xee, 0xf4]);
    code
}

#[test]
fn the_report_with_the_journal_equals_the_report_without_it_except_the_journal_field() {
    let rom = image(&two_frame_program());
    let dir = scratch("same-report");
    let plain = invoke(&rom, "10000");
    let journaled = invoke_with_args(
        &rom,
        "10000",
        &[
            "--framebuffer-every",
            "0.000000001",
            "--framebuffer-dir",
            dir.join("shots").to_str().unwrap(),
            "--phase-log",
            dir.join("phases.jsonl").to_str().unwrap(),
        ],
    );
    assert_eq!(plain.output.status.code(), Some(0), "{:?}", plain.output);
    assert_eq!(
        journaled.output.status.code(),
        Some(0),
        "{:?}",
        journaled.output
    );
    let (plain_report, mut journaled_report) = (report(&plain), report(&journaled));
    assert!(plain_report.get("journal").is_none());
    let journal = journaled_report.as_object_mut().unwrap().remove("journal");
    assert!(journal.is_some(), "the journaled run reports its journal");
    // The COM1 path differs between invocations by construction; everything else must match.
    let mut left = plain_report;
    for report in [&mut left, &mut journaled_report] {
        report.as_object_mut().unwrap().remove("com1_out");
    }
    assert_eq!(left, journaled_report);
    assert_eq!(
        fs::read(&plain.com1).unwrap(),
        fs::read(&journaled.com1).unwrap()
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn periodic_dumps_and_the_phase_log_work_on_any_boot_and_skip_unchanged_frames() {
    let rom = image(&two_frame_program());
    let dir = scratch("dumps");
    let (shots, phases) = (dir.join("shots"), dir.join("phases.jsonl"));
    let run = invoke_with_args(
        &rom,
        "10000",
        &[
            "--framebuffer-every",
            "0.000000001",
            "--framebuffer-dir",
            shots.to_str().unwrap(),
            "--phase-log",
            phases.to_str().unwrap(),
        ],
    );
    assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    let value = report(&run);
    assert_eq!(value["exit"], "halted");
    assert_eq!(fs::read(&run.com1).unwrap(), b"ABC");

    // Two generations were drawn, so exactly two files: the boundary after "C" and the final
    // look found the second frame already written.
    let mut files: Vec<_> = fs::read_dir(&shots)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");
    assert_eq!(value["journal"]["framebuffer_dumps"]["files"], 2);
    assert!(
        value["journal"]["framebuffer_dumps"]["skipped_unchanged"]
            .as_u64()
            .unwrap()
            >= 1
    );
    for (path, (generation, first_pixel)) in
        files.iter().zip([(1u64, 0xddcc_bbaau32), (2, 0x4433_2211)])
    {
        let bytes = fs::read(path).unwrap();
        assert_eq!(bytes.len(), 64 + 4 * 4 * 4);
        assert_eq!(&bytes[..8], b"ODFBRAW1");
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(
            [word(8), word(12), word(16), word(20), word(24)],
            [64, 4, 4, 16, 1]
        );
        assert_eq!(
            u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            generation
        );
        assert_eq!(word(64), first_pixel);
    }

    let events: Vec<Value> = fs::read_to_string(&phases)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let names: Vec<_> = events
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["first_com1_write", "stop"]);
    assert_eq!(events[1]["exit"], "halted");
    assert_eq!(events[1]["instructions"], value["retired"]);
    assert!(events.iter().all(|e| e["restart"] == 0));

    // The limit: one file, the rest counted.
    let limited_dir = dir.join("limited");
    let limited = invoke_with_args(
        &rom,
        "10000",
        &[
            "--framebuffer-every",
            "0.000000001",
            "--framebuffer-dir",
            limited_dir.to_str().unwrap(),
            "--framebuffer-max-files",
            "1",
        ],
    );
    assert_eq!(limited.output.status.code(), Some(0));
    assert_eq!(fs::read_dir(&limited_dir).unwrap().count(), 1);
    assert!(
        report(&limited)["journal"]["framebuffer_dumps"]["skipped_by_limit"]
            .as_u64()
            .unwrap()
            >= 1
    );

    // An occupied directory is refused before the guest runs.
    let again = invoke_with_args(
        &rom,
        "10000",
        &[
            "--framebuffer-every",
            "1",
            "--framebuffer-dir",
            shots.to_str().unwrap(),
        ],
    );
    assert_eq!(again.output.status.code(), Some(3));
    assert!(
        report(&again)["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("not empty")
    );
    assert!(!again.com1.exists());
    fs::remove_dir_all(dir).unwrap();
}

/// `--disk <path>,ro` end to end through the binary: the ROM halts at once (the guest write
/// path is covered by the runner unit test and the od-machine tests), the file is opened
/// read-only under a shared lock and is byte for byte the same afterwards.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn read_only_disk_is_attached_and_the_file_stays_unchanged() {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("od-runner-ro-{}-{id}.raw", std::process::id()));
    let content: Vec<u8> = (0..4096usize)
        .map(|index| (index * 7 % 253) as u8)
        .collect();
    fs::write(&path, &content).unwrap();
    let mut rom = image(&[0xf4]);
    rom[0x2a..0x2c].copy_from_slice(&12u16.to_le_bytes());
    rom[0x30..0x34].fill(0);
    let crc = crc32(&rom);
    rom[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
    let spec = format!("{},ro", path.to_str().unwrap());
    let run = invoke_with_args(&rom, "100", &["--disk", &spec]);
    assert_eq!(run.output.status.code(), Some(0), "{:?}", run.output);
    assert_eq!(report(&run)["exit"], "halted");
    assert_eq!(fs::read(&path).unwrap(), content);
    fs::remove_file(&run.com1).unwrap();

    // A read-only run does not create or extend a missing image.
    let missing = format!("{}.missing,ro", path.to_str().unwrap());
    let run = invoke_with_args(&rom, "100", &["--disk", &missing]);
    assert_eq!(run.output.status.code(), Some(3));
    assert_eq!(report(&run)["error"]["kind"], "disk");
    assert!(!std::path::Path::new(missing.strip_suffix(",ro").unwrap()).exists());
    fs::remove_file(path).unwrap();
}
