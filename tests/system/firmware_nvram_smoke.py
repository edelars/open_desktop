#!/usr/bin/env python3
"""Verify EFI variable persistence and access through the protected runtime path."""

import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from zlib import crc32

from firmware_boot_smoke import BUILDER, FIXTURES, ROOT, build, check_transcript


def protected_setup_snapshot() -> bytes:
    # ODNVBANK v2 with one bank containing a fresh ODVARSTR v2 in debug SetupMode.
    store = bytearray(struct.pack("<8sHHIII", b"ODVARSTR", 2, 24, 24, 0, 1))
    struct.pack_into("<I", store, 16, crc32(store))
    bank = struct.pack("<QI", 1, len(store))
    return (
        struct.pack("<8sII", b"ODNVBANK", 2, 0)
        + bank + struct.pack("<I", crc32(bank + store))
        + store + bytes(0x40000 - len(store)) + bytes(0x40010)
    )


def check_protected_snapshot(snapshot: bytes) -> None:
    banks = []
    for offset in (16, 16 + 0x40010):
        generation, length, checksum = struct.unpack_from("<QII", snapshot, offset)
        store = snapshot[offset + 16 : offset + 16 + length]
        if not 24 <= length <= 0x40000 or crc32(snapshot[offset : offset + 12] + store) != checksum:
            raise RuntimeError("Invalid protected NVRAM bank")
        banks.append((generation, store))
    _, store = max(banks)
    magic, version, header_size, length, checksum, flags = struct.unpack_from("<8sHHIII", store)
    if (
        (magic, version, header_size, length, flags) != (b"ODVARSTR", 2, 24, len(store), 3)
        or crc32(store[:16] + bytes(4) + store[20:24]) != checksum
    ):
        raise RuntimeError("Invalid active protected variable store")

    variables = {}
    offset = header_size
    while offset < len(store):
        size, _, state, tag, units, attrs, value_size, guid, checksum = struct.unpack_from(
            "<IIBBHII16sI", store, offset,
        )
        record = store[offset : offset + size]
        name_end = 40 + 2 * units
        value_start = name_end + 16 * tag
        if (
            size != value_start + value_size
            or len(record) != size
            or crc32(record[:36] + bytes(4) + record[40:]) != checksum
        ):
            raise RuntimeError("Invalid protected store record")
        name = record[40:name_end].decode("utf-16-le")
        variables[(guid.hex(), name)] = (state, attrs, record[name_end:value_start], record[value_start:])
        offset += size

    pk = variables.get(("61dfe48bca93d211aa0d00e098032b8c", "PK"))
    dbx = variables.get(("cbb219d73a3d9645a3bcdad00e67656f", "dbx"))
    counter = variables.get(("defe84cf01a5c2468a8d12ba3224192f", "ODMonotonicHigh"))
    public_key = (FIXTURES / "certs" / "windows-uefi-ca-2023.der").read_bytes()
    if (
        not pk or pk[:2] != (1, 0x27) or pk[2][6:7] != b"\x0b" or pk[3][44:] != public_key
        or not dbx or dbx[:2] != (1, 0x27) or dbx[2][6:7] != b"\x0a" or len(dbx[3]) != 19304
        or counter != (1, 0x07, b"", struct.pack("<I", 43))
    ):
        raise RuntimeError("Protected store lost policy, timestamps or the refreshed counter")


def check_protected_runtime(runner: Path, image: Path, scratch: Path) -> None:
    source = scratch / "protected-setup.nv"
    result_store = scratch / "protected-runtime.nv"
    transcript = scratch / "protected-runtime.com1"
    source.write_bytes(protected_setup_snapshot())
    budget = 15_000_000
    result = subprocess.run(
        [runner, "boot-firmware", "--image", image, "--fv-app", "rt",
         "--budget", str(budget), "--com1-out", transcript,
         "--nvram-in", source, "--nvram-out", result_store],
        cwd=ROOT, capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"Protected NVRAM exited {result.returncode}\n{result.stdout}{result.stderr}")
    report = json.loads(result.stdout)
    snapshot = result_store.read_bytes()
    if (
        report["status"] != "completed"
        or report["exit"] != "stopped"
        or report["stop_reason"] != "Shutdown"
        or report["fw_stop"] is not None
        or report["page_fault"] is not None
        or report["memory_error"] is not None
        or not 0 < report["retired"] <= budget
        or report["nvram_snapshot_sha256"] != hashlib.sha256(snapshot).hexdigest()
        or len(snapshot) != 0x80030
        or snapshot[:16] != source.read_bytes()[:16]
    ):
        raise RuntimeError(f"Unexpected protected NVRAM report: {report}")
    lines = transcript.read_bytes().splitlines()
    expected = [
        b"OD-UEFI protected Auth2 before EBS",
        b"OD-UEFI protected Auth2 after EBS",
        b"OD-UEFI protected NVRAM direct refresh",
        b"OD-UEFI protected Auth2 after VA",
    ]
    prefix = b"OD-UEFI protected "
    if [line[line.index(prefix):] for line in lines if prefix in line] != expected:
        raise RuntimeError("The runtime probe did not complete all protected NVRAM checks")
    check_protected_snapshot(snapshot)
    print(f"Protected runtime NVRAM {report['retired']} instructions, {report['quanta']} quanta")


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
            if marker not in actual or b"OD-UEFI vars NV monotonic high advanced\r\n" not in actual:
                raise RuntimeError(f"Boot #{index} did not exercise its expected NV probe path")
            print(f"NVRAM boot #{index} {boot['retired']} instructions, {boot['quanta']} quanta")
        check_protected_runtime(runner, image, scratch)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
