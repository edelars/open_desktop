#!/usr/bin/env python3
"""S7-34: the firmware ROM enumerates the board's xHCI keyboard, feeds it to ConIn, and stops it.

Slice A: boots the ROM with the xHCI controller (runner --xhci: PCI ECAM, controller at 00:05.0,
HID keyboard on port 1, absolute pointer on port 2) and checks that COM1 carries the single line
'OD-FW xhci: keyboard enumerated' and, apart from it and the memory map (the controller adds its
MMIO windows), the transcript of the same boot without the controller. A failed enumeration
prints 'OD-FW xhci: enumeration failed step=... detail=...' (likewise 'stop at exit failed' and
'ConInEx install failed'); it is reported here verbatim and fails the check. A board without the
controller must stay silent.

Slice C, input: the echo application (a test application booted from an NVMe ESP) reads ConIn and
prints what it reads to COM1. The runner feeds HID events (--xhci-keys) to the keyboard while the
ROM runs; the typed text must come back through the whole chain: model keyboard, Interrupt IN,
xHCI driver, key queue, ConIn, application, ConOut, COM1. The sequence covers letters, Shift, a
digit, an arrow, and CapsLock twice (which makes the driver send the LED report).

Slice C, section 12.3 of XHCI_DEVICE.md (ISSUE-648): after the ebs application left Boot Services
the runner's xHCI snapshot must show the controller stopped (HCH = 1, R/S = 0, CRR = 0,
DCBAAP = 0, IMAN.IE = 0, BAR0 unchanged, PCI Command restored) and no guest RAM access by the
controller after the stop. The hello application never leaves Boot Services: its snapshot shows
the controller still running, which proves the check can tell the two states apart.
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
DISK_BUILDER = ROOT / "tools" / "build" / "make_test_disk.py"
ECHO_RELATIVE = Path("test-apps") / "x86_64-unknown-uefi" / "release" / "echo.efi"

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

# Usage IDs (HID Usage Tables, keyboard page): h e l l o space, LeftShift down, w, LeftShift up,
# 1, UpArrow, CapsLock, a, CapsLock, a, Enter (press only: the application leaves on it).
ECHO_KEYS = "0b,08,0f,0f,12,2c,e1+,1a,e1-,1e,52,39,04,39,04,28+"
ECHO_EXPECTED = b"OD-UEFI echo\r\nhello W1[UP]Aa\r\n"


def comparable(transcript: bytes) -> bytes:
    transcript = LAYOUT_CRC_LINE.sub(rb"\1 BS=0x<layout> RT=0x<layout>", transcript)
    return MEMORY_MAP_LINE.sub(b"", transcript)


def run_runner(runner: Path, rom: Path, work: Path, name: str, args: list) -> tuple[bytes, subprocess.CompletedProcess]:
    transcript = work / f"{name}.com1"
    run = subprocess.run(
        [runner, "boot-firmware", "--image", rom, "--com1-out", transcript, "--budget", str(BUDGET), *args],
        capture_output=True, text=True, check=False,
    )
    com1 = transcript.read_bytes() if transcript.exists() else b""
    failure = FAILED.search(com1)
    if failure:
        raise RuntimeError(f"{name}: {failure.group(0).decode()}")
    return com1, run


def boot(runner: Path, rom: Path, work: Path, app: str, xhci: bool) -> tuple[bytes, dict]:
    name = f"{app}-{'xhci' if xhci else 'plain'}"
    com1, run = run_runner(runner, rom, work, name, ["--fv-app", app, *(["--xhci"] if xhci else [])])
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


def check_enumeration(runner: Path, rom: Path, work: Path) -> dict:
    reports = {}
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
        reports[app] = report["xhci"]
    return reports


def check_echo(runner: Path, rom: Path, echo_app: Path, work: Path) -> None:
    disk = work / "echo.raw"
    subprocess.run(
        [sys.executable, DISK_BUILDER, "--esp-fat32", "--bootx64", echo_app, "--out", disk], check=True,
    )
    com1, run = run_runner(
        runner, rom, work, "echo",
        ["--xhci", "--nvme-image", disk, "--xhci-keys", ECHO_KEYS],
    )
    # With no boot option left after the application returns the firmware stops with 0x21/1.
    report = json.loads(run.stdout)
    if (
        run.returncode != 6
        or report["fw_stop"] != {"code": "0x21", "detail": "0x0000000000000001"}
        or report["page_fault"] is not None
        or report["memory_error"] is not None
    ):
        raise RuntimeError(f"echo: unexpected stop (exit code {run.returncode}): {report}")
    keys = report["xhci"]
    if keys["keys_injected"] != keys["keys_total"] or keys["keys_refused"] != 0:
        raise RuntimeError(f"echo: not every key event reached the keyboard: {keys}")
    if ENUMERATED not in com1:
        raise RuntimeError("echo: the keyboard was not enumerated")
    if ECHO_EXPECTED not in com1:
        tail = com1[-400:]
        raise RuntimeError(f"echo: COM1 lacks {ECHO_EXPECTED!r}; tail: {tail!r}")
    print(f"echo: typed text arrived through ConIn: {ECHO_EXPECTED[len(b'OD-UEFI echo' + bytes([13, 10])):]!r}, "
          f"{report['retired']} instructions, {keys['keys_injected']} key events")


def check_stopped_controller(reports: dict) -> None:
    """XHCI_DEVICE.md 12.3 after ExitBootServices (ebs) against a controller that never stopped (hello)."""
    ebs, hello = reports["ebs"], reports["hello"]
    final = {key: int(value, 16) for key, value in ebs["final"].items() if key != "ram_accesses"}
    expected = {
        "usbcmd R/S": (final["usbcmd"] & 1, 0),
        "usbsts HCH": (final["usbsts"] & 1, 1),
        "crcr CRR": (final["crcr"] & 8, 0),
        "dcbaap": (final["dcbaap"], 0),
        "iman IE": (final["iman"] & 2, 0),
        "erstba": (final["erstba"], 0),
        "bar0": (final["bar0"], 0xFC00_8000),
        # The firmware gives Command back as it found it (0: memory decoding and bus mastering off).
        "pci command": (final["pci_command"], 0),
    }
    wrong = {name: pair for name, pair in expected.items() if pair[0] != pair[1]}
    if wrong or not ebs["ran"]:
        raise RuntimeError(f"ebs: the controller is not in the 12.3 state (value, expected): {wrong}; {ebs}")
    # No guest RAM access by the controller after it stopped (the runner samples the counter at the
    # first quantum boundary after R/S went 0) -- and it did use RAM before (the counter is not 0).
    stop = ebs["ram_accesses_at_stop"]
    if stop is None or stop == 0 or stop != ebs["final"]["ram_accesses"]:
        raise RuntimeError(f"ebs: guest RAM accesses after the stop: at stop {stop}, final {ebs['final']['ram_accesses']}")
    # Control: without ExitBootServices the controller is still running with its rings set up.
    running = {key: int(value, 16) for key, value in hello["final"].items() if key != "ram_accesses"}
    if (running["usbcmd"] & 1, running["usbsts"] & 1) != (1, 0) or running["dcbaap"] == 0 or hello["ram_accesses_at_stop"] is not None:
        raise RuntimeError(f"hello: the snapshot does not show a running controller: {hello}")
    print(f"ebs: controller stopped at exit (HCH=1, R/S=0, CRR=0, DCBAAP=0, IMAN.IE=0, BAR0 kept, Command restored), "
          f"{stop} RAM accesses before the stop and none after; hello: still running")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, help="od-runner (default: built with cargo)")
    parser.add_argument("--rom", type=Path, help="firmware built with --with-test-apps (default: built)")
    parser.add_argument("--echo-app", type=Path, help="echo.efi of that build (required with --rom)")
    args = parser.parse_args()
    if (args.rom is None) != (args.echo_app is None):
        parser.error("--rom and --echo-app go together")
    with tempfile.TemporaryDirectory(prefix="od-firmware-xhci-") as directory:
        work = Path(directory)
        runner = args.runner
        if runner is None:
            target = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target"))
            runner = target / "release" / "od-runner"
            subprocess.run(["cargo", "build", "--release", "-p", "od-runner", "--offline"], cwd=ROOT, check=True)
        rom, echo_app = args.rom, args.echo_app
        if rom is None:
            rom = work / "firmware.bin"
            subprocess.run(
                [sys.executable, BUILDER, "--with-test-apps", "--out", rom, "--target-dir", work / "target"],
                cwd=ROOT, check=True,
            )
            echo_app = work / "target" / ECHO_RELATIVE
        if not echo_app.is_file():
            raise RuntimeError(f"missing {echo_app}")
        reports = check_enumeration(runner.resolve(), rom.resolve(), work)
        check_stopped_controller(reports)
        check_echo(runner.resolve(), rom.resolve(), echo_app.resolve(), work)
        print("firmware xhci smoke passed (image sha256 "
              f"{hashlib.sha256(rom.read_bytes()).hexdigest()})")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
