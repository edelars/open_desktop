#!/usr/bin/env python3
"""Verify that the real vars EFI application reads a committed value after reboot."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from firmware_boot_smoke import BUILDER, FIXTURES, ROOT, build, check_transcript


def main() -> None:
    manifest = json.loads((FIXTURES / "manifest.json").read_text(encoding="utf-8"))["nvram_reboot"]
    runner = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "release" / "od-runner"
    build(["cargo", "build", "--release", "-p", "od-runner", "--offline"])
    with tempfile.TemporaryDirectory(prefix="od-firmware-nvram-") as directory:
        scratch = Path(directory)
        image = scratch / "firmware-with-apps.bin"
        build([sys.executable, BUILDER, "--with-test-apps", "--out", image,
               "--target-dir", scratch / "target"])
        transcripts = [scratch / "before.com1", scratch / "after.com1"]
        budget = manifest["instruction_budget"]
        result = subprocess.run(
            [runner, "boot-firmware", "--image", image, "--fv-app", "vars",
             "--budget", str(budget), "--com1-out", transcripts[0],
             "--nvram-reboot-out", transcripts[1]],
            cwd=ROOT, capture_output=True, text=True, check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(f"NVRAM reboot exited {result.returncode}\n{result.stdout}{result.stderr}")
        report = json.loads(result.stdout)
        boots = report["boots"]
        if (
            report["status"] != "completed"
            or report["exit"] != "halted"
            or len(boots) != 2
            or boots[0]["handoff_sha256"] != boots[1]["handoff_sha256"]
            or report["nvram_snapshot_sha256"] != manifest["snapshot_sha256"]
        ):
            raise RuntimeError(f"Unexpected NVRAM reboot report: {report}")
        image_hash = hashlib.sha256(image.read_bytes()).hexdigest()
        markers = [b"OD-UEFI vars NV probe set\r\n", b"OD-UEFI vars NV probe restored od-s7-10\r\n"]
        for index, (boot, case, transcript, marker) in enumerate(
            zip(boots, manifest["boots"], transcripts, markers, strict=True), start=1,
        ):
            if (
                boot["exit"] != "halted"
                or boot["status"] != "firmware_stop"
                or boot["fw_stop"] != manifest["fw_stop"]
                or boot["image_sha256"] != image_hash
                or not 0 < boot["retired"] <= budget
                or not 0 < boot["quanta"] <= manifest["max_quanta"]
            ):
                raise RuntimeError(f"Unexpected boot #{index}: {boot}")
            expected = (FIXTURES / case["fixture"]).read_bytes()
            if hashlib.sha256(expected).hexdigest() != case["fixture_sha256"]:
                raise RuntimeError(f"Boot #{index} fixture checksum differs from manifest")
            actual = transcript.read_bytes()
            check_transcript(actual, expected, f"NVRAM boot #{index}")
            if marker not in actual:
                raise RuntimeError(f"Boot #{index} did not exercise its expected NV probe path")
            print(f"NVRAM boot #{index} {boot['retired']} instructions, {boot['quanta']} quanta")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
