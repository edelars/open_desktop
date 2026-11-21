#!/usr/bin/env python3
"""S7-26: machine reset inside one runner process (`boot-firmware --reboot-cycles N`).

The `reboot` FV application reads and raises `ODBootCount` in NVRAM, prints it with PCR0 on COM1
and calls `ResetSystem`. The runner resets the board (`LoadedFirmwareBoard::platform_reset`) and
boots again, N times, without rebuilding anything. The script checks that

- every cycle ends with a `Reset` stop and the virtual clock keeps running across resets;
- `BootCount` is exactly the cycle number (the NVRAM bank committed before the reset survives);
- PCR0 is the same on every cycle and is not the initial value (the volatile TPM state is reset
  by `_TPM_Init` and rebuilt by the firmware's `TPM2_Startup(CLEAR)` and measurements);
- COM1 is byte-identical on every cycle except the `BootCount` line;
- the memory of the runner (physical footprint, which counts compressed pages; the `ps` RSS where
  the `footprint` tool is missing) differs between the start and the end by no more than the limit.

One cycle costs about 3.7e8 guest instructions (the firmware hashes its ROM for PCR0), roughly
2.7 minutes in a release build: the default 5 cycles take about 14 minutes, 50 cycles about
2.3 hours. Usage: `firmware_reset_smoke.py [--cycles N] [--kind cold|warm] [--rss-limit-kib K]`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from firmware_boot_smoke import BUILDER, ROOT, build

COUNT_LINE = re.compile(rb"OD-UEFI reboot BootCount=(\d+)\r\n")
PCR0_LINE = re.compile(rb"OD-UEFI PCR0=([0-9a-f]{64})\r\n")
# The TPM boot of one cycle retires about 3.66e8 instructions; the budget is per cycle.
BUDGET = 700_000_000
# Change of the runner memory between the start and the end of the cycles that is accepted, in KiB.
# Measured (docs/testing/FIRMWARE.md, S7-26a): the footprint grew by 3024 KiB over 50 cycles and by
# 2656 KiB over 5 (almost all of it in the first two cycles), the `ps` RSS by 5136 KiB over 50.
# The limit is the measured 50-cycle growth rounded up to the next MiB.
RSS_LIMIT_KIB = 4096


def check(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def check_cycles(report: dict, com1: bytes, cycles: int, rss_limit_kib: int) -> dict:
    check(report["status"] == "completed", f"reboot run did not complete: {report}")
    boots = report["boots"]
    check(report["reboot_cycles"] == cycles and len(boots) == cycles, f"expected {cycles} boots: {report}")
    check(report["com1_bytes"] == len(com1), "COM1 length differs from the report")
    previous_ns = 0
    segments = []
    pcr0 = None
    for number, boot in enumerate(boots, start=1):
        check(
            boot["cycle"] == number
            and boot["exit"] == "stopped"
            and boot["stop_reason"] == "Reset"
            and 0 < boot["retired"] <= BUDGET,
            f"cycle {number} did not end with a Reset stop: {boot}",
        )
        # The virtual clock is not reset: it advances by at least the retired instructions.
        check(
            boot["virtual_ns"] - previous_ns >= boot["retired"],
            f"cycle {number}: virtual time did not keep running: {boot}",
        )
        previous_ns = boot["virtual_ns"]
        segment = com1[boot["com1_offset"] : boot["com1_offset"] + boot["com1_bytes"]]
        counts = COUNT_LINE.findall(segment)
        check(counts == [str(number).encode()], f"cycle {number}: BootCount lines {counts}")
        digests = PCR0_LINE.findall(segment)
        check(len(digests) == 1, f"cycle {number}: PCR0 lines {digests}")
        pcr0 = pcr0 or digests[0]
        check(digests[0] == pcr0, f"cycle {number}: PCR0 {digests[0]!r} differs from cycle 1 {pcr0!r}")
        segments.append(COUNT_LINE.sub(b"OD-UEFI reboot BootCount=N\r\n", segment))
    check(pcr0 != b"0" * 64, "PCR0 is the initial value: nothing was measured")
    for number, segment in enumerate(segments[1:], start=2):
        check(
            segment == segments[0],
            f"cycle {number}: COM1 differs from cycle 1 beyond the BootCount line "
            f"({hashlib.sha256(segment).hexdigest()} vs {hashlib.sha256(segments[0]).hexdigest()})",
        )
    # RSS (ps) on macOS shrinks when the compressor takes idle pages, so the limit applies to the
    # physical footprint (it counts compressed memory) where the runner could read it.
    metric = "footprint" if report["footprint_kib_start"] is not None else "rss"
    start, end = report[f"{metric}_kib_start"], report[f"{metric}_kib_end"]
    check(start is not None and end is not None, "the runner could not read its own memory use")
    check(
        abs(end - start) <= rss_limit_kib,
        f"runner {metric} changed by {end - start} KiB over {cycles} cycles (limit {rss_limit_kib}): "
        f"{report[f'{metric}_kib_after_cycle']}",
    )
    return {"pcr0": pcr0.decode(), "metric": metric, "delta_kib": end - start}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--cycles", type=int, default=5)
    parser.add_argument("--kind", choices=("cold", "warm"), default="cold")
    parser.add_argument("--rss-limit-kib", type=int, default=RSS_LIMIT_KIB)
    options = parser.parse_args()
    check(options.cycles >= 2, "at least two cycles are needed to compare them")
    runner = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "release" / "od-runner"
    build(["cargo", "build", "--release", "-p", "od-runner", "--offline"])
    with tempfile.TemporaryDirectory(prefix="od-firmware-reset-") as directory:
        scratch = Path(directory)
        image = scratch / "firmware-with-apps.bin"
        build([sys.executable, BUILDER, "--with-test-apps", "--with-reboot-app", "--out", image,
               "--target-dir", scratch / "target"])
        transcript = scratch / "reboot.com1"
        result = subprocess.run(
            [runner, "boot-firmware", "--image", image, "--fv-app", "reboot",
             "--reboot-cycles", str(options.cycles), "--reboot-kind", options.kind,
             "--budget", str(BUDGET), "--com1-out", transcript],
            cwd=ROOT, capture_output=True, text=True, check=False,
        )
        check(result.returncode == 0, f"reboot run exited {result.returncode}\n{result.stdout}{result.stderr}")
        report = json.loads(result.stdout)
        summary = check_cycles(report, transcript.read_bytes(), options.cycles, options.rss_limit_kib)
        retired = [boot["retired"] for boot in report["boots"]]
        print(
            f"{options.cycles} {options.kind} reboot cycles in one process: BootCount 1..{options.cycles}, "
            f"PCR0 {summary['pcr0']}, retired {min(retired)}..{max(retired)} per cycle, "
            f"RSS {report['rss_kib_start']} -> {report['rss_kib_end']} KiB, "
            f"footprint {report['footprint_kib_start']} -> {report['footprint_kib_end']} KiB "
            f"({summary['metric']} delta {summary['delta_kib']}, limit {options.rss_limit_kib}), "
            f"{report['wall_ms']} ms"
        )


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
