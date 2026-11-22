#!/usr/bin/env python3
"""S7-34 slice A: the firmware ROM enumerates the board's xHCI keyboard.

Boots the ROM with the xHCI controller (runner --xhci: PCI ECAM, controller at 00:05.0, HID keyboard
on port 1, absolute pointer on port 2) and checks that COM1 carries the single line
'OD-FW xhci: keyboard enumerated' and, apart from it and the memory map (the controller adds its
MMIO windows), the transcript of the same boot without the controller. A failed enumeration
prints 'OD-FW xhci: enumeration failed step=... detail=...' (likewise 'stop at exit failed' and
'ConInEx install failed'); it is reported here verbatim and fails the check. Key input (slice B)
is covered by host tests; feeding HID events into the ROM needs the echo application (slice C). A board without the controller must stay silent. The ebs application leaves
through ExitBootServices, which runs the handler that stops the controller.
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

ROOT = Path(__file__).resolve().parents[2]
BUILDER = ROOT / "tools" / "build" / "build_firmware.py"

ENUMERATED = b"OD-FW xhci: keyboard enumerated\r\n"
FAILED = re.compile(rb"OD-FW xhci: (?:enumeration failed|stop at exit failed|ConInEx install failed)[^\r\n]*")
# The port reset takes 50 ms of virtual time (50e6 instructions at 1 ns each) plus the enumeration.
BUDGET = 400_000_000
LAYOUT_CRC_LINE = re.compile(
    rb"(OD-FW S7-05 table CRC ST=0x[0-9A-F]{16}) BS=0x[0-9A-F]{16} RT=0x[0-9A-F]{16}"
)
# The controller adds its windows to the memory map and the map shifts with the code size.
MEMORY_MAP_LINE = re.compile(rb"OD-FW (MEM type=|S7-04 memory map )[^\r\n]*\r\n")
# Application, expected runner exit code and report status/fw_stop (as in the boot manifest).
APPS = {
    "hello": (6, "firmware_stop", {"code": "0x21", "detail": "0x0000000000000000"}),
    "ebs": (0, "completed", None),
}


def comparable(transcript: bytes) -> bytes:
    transcript = LAYOUT_CRC_LINE.sub(rb"\1 BS=0x<layout> RT=0x<layout>", transcript)
    return MEMORY_MAP_LINE.sub(b"", transcript)


def boot(runner: Path, rom: Path, work: Path, app: str, xhci: bool) -> tuple[bytes, dict]:
    name = f"{app}-{'xhci' if xhci else 'plain'}"
    transcript = work / f"{name}.com1"
    run = subprocess.run(
        [runner, "boot-firmware", "--image", rom, "--com1-out", transcript, "--budget", str(BUDGET),
         "--fv-app", app, *(["--xhci"] if xhci else [])],
        capture_output=True, text=True, check=False,
    )
    com1 = transcript.read_bytes() if transcript.exists() else b""
    failure = FAILED.search(com1)
    if failure:
        raise RuntimeError(f"{name}: {failure.group(0).decode()}")
    code, status, fw_stop = APPS[app]
    if run.returncode != code:
        raise RuntimeError(f"{name}: runner exit code {run.returncode}, expected {code}\n{run.stdout}{run.stderr}")
    report = json.loads(run.stdout)
    if (
        report["exit"] != "halted"
        or report["status"] != status
        or report["fw_stop"] != fw_stop
        or report["page_fault"] is not None
        or report["memory_error"] is not None
        or not 0 < report["retired"] <= BUDGET
    ):
        raise RuntimeError(f"{name}: unexpected stop: {report}")
    return com1, report


def check(runner: Path, rom: Path, work: Path) -> None:
    for app in APPS:
        plain, _ = boot(runner, rom, work, app, xhci=False)
        if b"xhci" in plain:
            raise RuntimeError(f"{app}: a board without the controller printed an xhci line")
        com1, report = boot(runner, rom, work, app, xhci=True)
        if com1.count(ENUMERATED) != 1:
            raise RuntimeError(f"{app}: COM1 lacks exactly one {ENUMERATED!r}")
        if comparable(com1.replace(ENUMERATED, b"")) != comparable(plain):
            raise RuntimeError(f"{app}: COM1 differs from the boot without the controller apart from the xhci line")
        print(f"{app}: keyboard enumerated, {report['retired']} instructions, {report['quanta']} quanta, "
              f"{report['virtual_ns']} virtual ns")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, help="od-runner (default: built with cargo)")
    parser.add_argument("--rom", type=Path, help="firmware built with --with-test-apps (default: built)")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="od-firmware-xhci-") as directory:
        work = Path(directory)
        runner = args.runner
        if runner is None:
            target = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target"))
            runner = target / "release" / "od-runner"
            subprocess.run(["cargo", "build", "--release", "-p", "od-runner", "--offline"], cwd=ROOT, check=True)
        rom = args.rom
        if rom is None:
            rom = work / "firmware.bin"
            subprocess.run(
                [sys.executable, BUILDER, "--with-test-apps", "--out", rom, "--target-dir", work / "target"],
                cwd=ROOT, check=True,
            )
        check(runner.resolve(), rom.resolve(), work)
        print("firmware xhci smoke passed (image sha256 "
              f"{hashlib.sha256(rom.read_bytes()).hexdigest()})")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
