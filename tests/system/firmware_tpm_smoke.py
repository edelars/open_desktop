#!/usr/bin/env python3
"""Exercise the TPM probe through the firmware volume and a bootable NVMe ESP."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[2]
DISK_BUILDER = ROOT / "tools" / "build" / "make_test_disk.py"
EXPECTED = (
    b"OD-UEFI TPM2: CRB method=7 control=0xfed40040",
    b"OD-UEFI TCG2: SHA-256 and SHA-384 banks",
    b"OD-UEFI TCG2: chunked 4991-byte command reached TPM parameter validation",
    b"OD-UEFI TCG2: Spec ID Event03",
    b"OD-UEFI TCG2: final events verified",
    b"OD-UEFI PCR0/1/4/5 log replay: match",
    b"OD-UEFI TPM2_GetRandom: 16 bytes",
    b"OD-UEFI TPM2_TestParms: ECDSA-SHA256 P-256",
    b"OD-UEFI TCG2: final events retained after EBS",
)


def boot(runner: Path, rom: Path, work: Path, name: str, options: list[str]) -> None:
    trace = work / f"{name}.com1"
    run = subprocess.run(
        [runner, "boot-firmware", "--image", rom, "--com1-out", trace,
         "--budget", "700000000", *options],
        check=False, capture_output=True,
    )
    (work / f"{name}.json").write_bytes(run.stdout)
    report = json.loads(run.stdout)
    if run.returncode != 0 or report["exit"] != "halted" or report["status"] != "completed":
        raise RuntimeError(f"{name}: runner exited {run.returncode}: {report}")
    transcript = trace.read_bytes()
    offset = 0
    for line in EXPECTED:
        match = transcript.find(line + b"\r\n", offset)
        if match < 0:
            raise RuntimeError(f"{name}: missing or out-of-order COM1 line {line!r}")
        offset = match + len(line)
    if name == "tpm-esp" and b"OD-UEFI TCG2: boot variables and GPT verified\r\n" not in transcript:
        raise RuntimeError(f"{name}: no verified measured boot variables and GPT")
    print(f"{name}: {report['retired']} instructions, {report['quanta']} quanta")


def run(runner: Path, rom: Path, app: Path, work: Path) -> None:
    disk = work / "tpm-esp.raw"
    subprocess.run(
        [sys.executable, DISK_BUILDER, "--esp-fat32", "--bootx64", app, "--out", disk],
        check=True,
    )
    boot(runner, rom, work, "tpm-fv", ["--fv-app", "tpm"])
    boot(runner, rom, work, "tpm-esp", ["--tpm", "--nvme-image", str(disk)])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--rom", type=Path, required=True, help="firmware built with --with-test-apps")
    parser.add_argument("--app", type=Path, required=True, help="the tpm.efi embedded in the ROM")
    parser.add_argument("--work", type=Path, help="retain the disk, COM1 traces and JSON reports")
    args = parser.parse_args()
    if args.work is not None:
        args.work.mkdir(parents=True, exist_ok=True)
        run(args.runner.resolve(), args.rom.resolve(), args.app.resolve(), args.work.resolve())
    else:
        with tempfile.TemporaryDirectory(prefix="od-firmware-tpm-") as path:
            run(args.runner.resolve(), args.rom.resolve(), args.app.resolve(), Path(path))


if __name__ == "__main__":
    main()
