#!/usr/bin/env python3
"""Build and verify the Stage 7 boot core and FV application transcripts."""

from __future__ import annotations

import difflib
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests" / "fixtures" / "firmware"
BUILDER = ROOT / "tools" / "build" / "build_firmware.py"


def build(args: list[str | Path]) -> None:
    subprocess.run(args, cwd=ROOT, check=True)


def check_transcript(actual: bytes, expected: bytes, label: str) -> None:
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
        or not 0 < report["retired"] <= case["instruction_budget"]
        or not 0 < report["quanta"] <= case["max_quanta"]
    ):
        raise RuntimeError(f"{label} did not reach its expected stop: {report}")

    if not transcript.exists():
        raise RuntimeError(f"{label} produced no COM1 transcript")
    print(f"{label} {elapsed_ms:.2f} ms, {report['retired']} instructions, {report['quanta']} quanta")


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
