#!/usr/bin/env python3
"""Build and verify the Stage 7 boot core and FV application transcripts."""

from __future__ import annotations

import difflib
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests" / "fixtures" / "firmware"
BUILDER = ROOT / "tools" / "build" / "build_firmware.py"
DISK_PATTERN = ROOT / "tools" / "build" / "make_test_disk.py"
DISPLAY_PATTERN = ROOT / "tools" / "build" / "display_pattern.py"
DISPLAY_COMPARE = ROOT / "tests" / "system" / "display_compare.py"


def build(args: list[str | Path]) -> None:
    subprocess.run(args, cwd=ROOT, check=True)


# The CRCs of the Boot Services and Runtime Services tables cover function pointers. Their values
# follow the code layout of the freestanding image: they change with any firmware code change and,
# for the same sources, with the absolute path of the checkout (the order of items inside the single
# codegen unit follows path-dependent hashes). The table contents are verified by field names in
# ISSUE-611, not by the CRC value, so the comparison ignores only these two values. The System
# Table CRC stays exact: it covers addresses of data, not of code.
LAYOUT_CRC_LINE = re.compile(
    rb"(OD-FW S7-05 table CRC ST=0x[0-9A-F]{16}) BS=0x[0-9A-F]{16} RT=0x[0-9A-F]{16}"
)


def mask_layout_crcs(data: bytes) -> bytes:
    return LAYOUT_CRC_LINE.sub(rb"\1 BS=0x<layout> RT=0x<layout>", data)


def check_transcript(actual: bytes, expected: bytes, label: str) -> None:
    actual, expected = mask_layout_crcs(actual), mask_layout_crcs(expected)
    if actual == expected:
        return
    expected_lines = [repr(line) + "\n" for line in expected.splitlines(keepends=True)]
    actual_lines = [repr(line) + "\n" for line in actual.splitlines(keepends=True)]
    diff = "".join(difflib.unified_diff(expected_lines, actual_lines, "fixture", label))
    raise RuntimeError(f"{label} COM1 differs from its byte fixture\n{diff}")


def run_case(
    runner: Path,
    image: Path,
    image_sha256: str,
    scratch: Path,
    case: dict,
    round_number: int,
) -> None:
    name = case["name"]
    label = f"{name} #{round_number}"
    transcript = scratch / f"{name}-{round_number}.com1"
    args = [
        runner, "boot-firmware", "--image", image,
        "--com1-out", transcript, "--budget", str(case["instruction_budget"]),
    ]
    if app := case["fv_app"]:
        args.extend(["--fv-app", app])
    if case["fv_app"] in ("blk", "fs"):
        disk = scratch / f"{name}.raw"
        if not disk.exists():
            disk_options = (
                ["--esp-fat32"] if case["fv_app"] == "fs" else ["--damage", case["nvme_damage"]]
            )
            build([sys.executable, DISK_PATTERN, *disk_options, "--out", disk])
        digest = hashlib.sha256()
        with disk.open("rb") as image_file:
            for block in iter(lambda: image_file.read(64 * 1024), b""):
                digest.update(block)
        if digest.hexdigest() != case["nvme_image_sha256"]:
            raise RuntimeError(f"{name} disk checksum differs from manifest")
        args.extend(["--nvme-image", disk])
    guest_acpi = scratch / f"acpi-memory-{round_number}.bin"
    guest_frame = scratch / f"gop-frame-{round_number}.raw"
    if case["fv_app"] == "acpi":
        args.extend(["--acpi-out", guest_acpi])
    if case["fv_app"] == "gop":
        args.extend(["--framebuffer-out", guest_frame])
    start = time.perf_counter_ns()
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True, check=False)
    elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
    expected = (FIXTURES / case["fixture"]).read_bytes()
    if hashlib.sha256(expected).hexdigest() != case["fixture_sha256"]:
        raise RuntimeError(f"{name} fixture checksum differs from manifest")
    if transcript.exists():
        check_transcript(transcript.read_bytes(), expected, label)
    if result.returncode != case["returncode"]:
        raise RuntimeError(
            f"{label} exit code {result.returncode}, expected {case['returncode']}\n"
            f"{result.stdout}{result.stderr}"
        )
    report = json.loads(result.stdout)
    if (
        report["exit"] != case.get("exit", "halted")
        or report.get("stop_reason") != case.get("stop_reason")
        or report["status"] != case["status"]
        or report["fw_stop"] != case["fw_stop"]
        or report["image_sha256"] != image_sha256
        or report["page_fault"] is not None
        or report["memory_error"] is not None
        or not 0 < report["retired"] <= case["instruction_budget"]
        or not 0 < report["quanta"] <= case["max_quanta"]
    ):
        raise RuntimeError(f"{label} did not reach its expected stop: {report}")

    if not transcript.exists():
        raise RuntimeError(f"{label} produced no COM1 transcript")
    print(f"{label} {elapsed_ms:.2f} ms, {report['retired']} instructions, {report['quanta']} quanta")
    if case["fv_app"] == "acpi":
        handoff = scratch / "acpi-handoff.bin"
        output = scratch / "acpi-blob.bin"
        build([runner, "dump-handoff-block", "--fv-app", "acpi", "--out", handoff])
        block = handoff.read_bytes()
        if (block != (FIXTURES / "acpi_handoff.bin").read_bytes()
                or hashlib.sha256(block).hexdigest() != report["handoff_sha256"]):
            raise RuntimeError(f"{label} did not boot the same handoff as the ACPI builder")
        build([
            sys.executable, BUILDER, "--acpi-blob", handoff,
            "--compare-acpi-guest", guest_acpi, "--out", output,
            "--target-dir", scratch / "acpi-target",
        ])
        if hashlib.sha256(output.read_bytes()).hexdigest() != case["acpi_blob_sha256"]:
            raise RuntimeError(f"{label} freestanding blob checksum differs from its fixture")
    if case["fv_app"] == "gop":
        frame = case["framebuffer"]
        if report.get("framebuffer") != frame:
            raise RuntimeError(f"{label} produced unexpected framebuffer metadata: {report}")
        expected_frame = scratch / "bars.raw"
        dimensions = ["--width", str(frame["width"]), "--height", str(frame["height"])]
        if not expected_frame.exists():
            build([sys.executable, DISPLAY_PATTERN, *dimensions, "--pattern", "bars",
                   "--seed", "0", "--output", expected_frame])
        build([sys.executable, DISPLAY_COMPARE, *dimensions, "--tolerance", "0",
               expected_frame, guest_frame])


def check_missing_app(runner: Path, stock: Path, scratch: Path, budget: int) -> None:
    transcript = scratch / "missing-ebs.com1"
    result = subprocess.run(
        [runner, "boot-firmware", "--image", stock, "--fv-app", "ebs",
         "--com1-out", transcript, "--budget", str(budget)],
        cwd=ROOT, capture_output=True, text=True, check=False,
    )
    report = json.loads(result.stdout)
    if (
        result.returncode != 6
        or report["exit"] != "halted"
        or report["status"] != "firmware_stop"
        or report["fw_stop"] != {"code": "0x35", "detail": "0x800000000000000e"}
        or not transcript.read_bytes().endswith(
            b"\r\nOD-FW STOP code=0x35 detail=0x800000000000000E\r\n"
        )
    ):
        raise RuntimeError(f"Missing FV application did not stop with EFI_NOT_FOUND: {report}")
    print("Missing FV application stopped with EFI_NOT_FOUND")


def main() -> None:
    manifest = json.loads((FIXTURES / "manifest.json").read_text(encoding="utf-8"))
    target = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    runner = target / "release" / "od-runner"
    build(["cargo", "build", "--release", "-p", "od-runner", "--offline"])
    with tempfile.TemporaryDirectory(prefix="od-firmware-smoke-") as directory:
        scratch = Path(directory)
        stock = scratch / "firmware.bin"
        apps = scratch / "firmware-with-apps.bin"
        build([sys.executable, BUILDER, "--out", stock, "--target-dir", scratch / "stock-target"])
        build([
            sys.executable, BUILDER, "--with-test-apps", "--out", apps,
            "--target-dir", scratch / "app-target",
        ])
        image_hashes = {image: hashlib.sha256(image.read_bytes()).hexdigest() for image in (stock, apps)}
        for case in manifest["cases"]:
            image = apps if case["fv_app"] else stock
            for round_number in range(1, case["repetitions"] + 1):
                run_case(runner, image, image_hashes[image], scratch, case, round_number)
        check_missing_app(runner, stock, scratch, manifest["missing_app_instruction_budget"])


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
