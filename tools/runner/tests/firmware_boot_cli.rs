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
