#!/usr/bin/env python3
"""S7-31n: efivarfs and ACPI as the Ubuntu 7.0.0-38 kernel shows them on the unified loop.

Boots the stock kernel (and the two diagnostic variants of firmware_linux_uefi_stop_record.py) through
our UEFI firmware on `od-runner boot-firmware --cpu-loop unified`, with the S7-31n initramfs
(tests/fixtures/guest/s7_31n/init.S, built by tools/build/build_s7_31n_initramfs.py into work/).
The NVRAM seed holds Boot0001, BootOrder and ODBootCount (the S7-26 test variable, vendor GUID of
the reboot application). The /init mounts efivarfs, lists it and dumps the kernel ring buffer to
COM1 between markers; this script reads only COM1. Diagnostic, not an acceptance test: it exits 0
when every variant produced identical records on every run, whatever they show. The record keeps the
COM1 hash and excerpts (efivars names, ACPI lines), not the log.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import uuid
import zlib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests" / "system"))
import firmware_linux_uefi_stop_record as stop  # noqa: E402
esp_smoke = stop.esp_smoke

BUILD_INITRAMFS = ROOT / "tools" / "build" / "build_s7_31n_initramfs.py"
DEFAULT_INITRAMFS = ROOT / "work" / "stage7" / "s7-31n" / "initramfs.cpio"
OD_VENDOR = uuid.UUID("6f1d52a4-26b0-4f0c-9a37-4e0b61c82d15").bytes_le  # as the reboot test application
ODBOOTCOUNT = 3
NAME_RE = re.compile(rb"[A-Za-z0-9_]+-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
# The coordinator's filter plus the usual severity words of the ACPI core.
ACPI_ERROR_RE = re.compile(r"acpi.*error|ACPI BIOS Error|AE_", re.IGNORECASE)
ACPI_WARNING_RE = re.compile(r"acpi.*(warn|fail|invalid|unable|bad|ignor|unsupported)", re.IGNORECASE)
MARKERS = ("OD_S7_31N_BEGIN", "OD_S7_31N_EFIVARFS_MOUNT_OK", "OD_S7_31N_EFIVARFS_MOUNT_FAIL",
           "OD_S7_31N_EFIVARS_BEGIN", "OD_S7_31N_EFIVARS_END", "OD_S7_31N_DMESG_BEGIN",
           "OD_S7_31N_DMESG_END", "OD_S7_31N_END")


def snapshot(entries: list[tuple[str, bytes, bytes]]) -> bytes:
    """ODNVBANK v1 with one committed store: (name, vendor GUID bytes, value), NV|BS|RT."""
    records = []
    for name, guid, value in entries:
        encoded = name.encode("utf-16le")
        size = 40 + len(encoded) + len(value)
        fixed = struct.pack("<IIBBHII16sI", size, size ^ 0xffffffff, 1, 0, len(name), 7, len(value), guid, 0)
        record = bytearray(fixed + encoded + value)
        struct.pack_into("<I", record, 36, zlib.crc32(record))
        records.append(record)
    body = b"".join(records)
    header = bytearray(struct.pack("<8sHHIII", b"ODVARSTR", 1, 24, 24 + len(body), 0, 0))
    struct.pack_into("<I", header, 16, zlib.crc32(header))
    payload = header + body
    identity = struct.pack("<QI", 1, len(payload))
    bank = identity + struct.pack("<I", zlib.crc32(identity + payload)) + payload
    return struct.pack("<8sII", b"ODNVBANK", 1, 0) + bank + bytes(2 * esp_smoke.BANK_BYTES - len(bank))


def section(raw: bytes, begin: bytes, end: bytes) -> bytes | None:
    start = raw.find(begin)
    if start < 0:
        return None
    start += len(begin)
    stop_at = raw.find(end, start)
    return raw[start:stop_at] if stop_at >= 0 else None


def analyse(com1: bytes) -> dict:
    markers = {name: com1.find(name.encode()) >= 0 for name in MARKERS}
    efivars = section(com1, b"OD_S7_31N_EFIVARS_BEGIN", b"OD_S7_31N_EFIVARS_END")
    names = sorted({m.decode() for m in NAME_RE.findall(efivars)}) if efivars is not None else None
    dmesg = section(com1, b"OD_S7_31N_DMESG_BEGIN", b"OD_S7_31N_DMESG_END")
    text = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", dmesg or b"").replace(b"\r", b"").decode("latin1")
    lines = [line for line in text.split("\n") if line.strip()]
    acpi = [line for line in lines if "acpi" in line.lower()]
    console = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", com1).replace(b"\r", b"").decode("latin1")
    console_acpi = [line for line in console.split("\n") if "acpi" in line.lower()]
    return {
        "markers": markers,
        "efivarfs_mounted": markers["OD_S7_31N_EFIVARFS_MOUNT_OK"] and not markers["OD_S7_31N_EFIVARFS_MOUNT_FAIL"],
        "efivars_names": names,
        "dmesg_lines": len(lines),
        "dmesg_acpi_lines": acpi,
        "dmesg_acpi_error_lines": [line for line in acpi if ACPI_ERROR_RE.search(line)],
        "dmesg_acpi_warning_like_lines": [line for line in acpi if ACPI_WARNING_RE.search(line)],
        "console_acpi_lines_total": len(console_acpi),
        "console_acpi_error_lines": [line for line in console_acpi if ACPI_ERROR_RE.search(line)],
    }


def one_run(job: tuple) -> tuple[str, int, dict]:
    variant, run, runner, rom, esp, nvram, scratch, budget, rom_bytes, kernel_bytes, layout = job
    report, com1 = stop.boot(runner, rom, esp, nvram, scratch, f"{variant}-{run}", budget, [], "unified", None)
    record = {
        "exit": report["exit"], "exit_detail": report["exit_detail"], "rip": report["rip"],
        "retired": report["retired"], "quanta": report["quanta"], "activity": report["activity"],
        "runner_exit_code": report["runner_exit_code"], "com1_bytes": report["com1_bytes"],
        "com1_sha256": hashlib.sha256(com1).hexdigest(), "nvram_snapshot_sha256": report["nvram_snapshot_sha256"],
        "cpl3_retired": (report.get("unified") or {}).get("cpl3_retired"),
        "com1_last_line": stop.last_line(com1),
    }
    record.update(analyse(com1))
    print(f"{variant} run {run}: {record['exit']} retired={record['retired']} mounted={record['efivarfs_mounted']} "
          f"names={record['efivars_names']}", flush=True)
    return variant, run, record


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stage4-dir", type=Path, default=ROOT / "work" / "stage4")
    parser.add_argument("--initramfs", type=Path, default=DEFAULT_INITRAMFS)
    parser.add_argument("--work-dir", type=Path, default=None)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=2)
    parser.add_argument("--budget", type=int, default=6_000_000_000)
    parser.add_argument("--variants", default=",".join(stop.VARIANTS))
    parser.add_argument("--jobs", type=int, default=6, help="runner processes at once")
    args = parser.parse_args()
    variants = args.variants.split(",")
    if any(name not in stop.VARIANTS for name in variants) or args.runs < 1:
        parser.error(f"variants must come from {stop.VARIANTS}")
    kernel_path = args.stage4_dir / "linux-7.0.0-38.38" / "vmlinuz-7.0.0-38-generic"
    subprocess.run([sys.executable, BUILD_INITRAMFS, "--out-dir", args.initramfs.parent], check=True, cwd=ROOT,
                   stdout=subprocess.DEVNULL)
    target = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    runner = target / "release" / "od-runner"
    subprocess.run(["cargo", "build", "--release", "-p", "od-runner", "--offline"], check=True, cwd=ROOT)
    with tempfile.TemporaryDirectory(prefix="od-efivars-acpi-") as temporary:
        scratch = args.work_dir or Path(temporary)
        scratch.mkdir(parents=True, exist_ok=True)
        rom = scratch / "stock.bin"
        subprocess.run([sys.executable, stop.BUILDER, "--out", rom, "--target-dir", scratch / "rom"],
                       check=True, cwd=ROOT, stdout=subprocess.DEVNULL)
        nvram = scratch / "seed.nv"
        option = esp_smoke.load_option(esp_smoke.file_node("BOOTX64.EFI"), (stop.CMDLINE + "\0").encode("utf-16le"))
        nvram.write_bytes(snapshot([
            ("Boot0001", esp_smoke.GLOBAL, option),
            ("BootOrder", esp_smoke.GLOBAL, struct.pack("<H", 1)),
            ("ODBootCount", OD_VENDOR, struct.pack("<I", ODBOOTCOUNT)),
        ]))
        record = {
            "schema": "od-s7-31n-efivars-acpi-v1",
            "inputs": {
                "vmlinuz_sha256": stop.sha256(kernel_path), "initramfs_sha256": stop.sha256(args.initramfs),
                "firmware_rom_sha256": stop.sha256(rom), "nvram_seed_sha256": stop.sha256(nvram),
                "cmdline": stop.CMDLINE,
                "nvram_seed_variables": ["Boot0001-8be4df61-93ca-11d2-aa0d-00e098032b8c",
                                         "BootOrder-8be4df61-93ca-11d2-aa0d-00e098032b8c",
                                         f"ODBootCount-{uuid.UUID(bytes_le=OD_VENDOR)} (u32 = {ODBOOTCOUNT})"],
            },
            "variants": {},
        }
        jobs, esps = [], {}
        for variant in variants:
            patched = stop.make_variant(kernel_path, variant, scratch / f"vmlinuz-{variant}.efi")
            esp = scratch / f"esp-{variant}.raw"
            subprocess.run([sys.executable, stop.DISK, "--esp-fat32", "--bootx64", patched, "--boot-file",
                            f"INITRD.IMG={args.initramfs}", "--out", esp], check=True, cwd=ROOT)
            esps[variant] = (stop.sha256(patched), stop.sha256(esp))
            for run in range(args.runs):
                jobs.append((variant, run, runner, rom, esp, nvram, scratch, args.budget, None, None, None))
        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            results = list(pool.map(one_run, jobs))
        all_equal = True
        for variant in variants:
            runs = [rec for name, _, rec in sorted(results, key=lambda item: (item[0], item[1])) if name == variant]
            identical = all(item == runs[0] for item in runs[1:])
            all_equal &= identical
            first = runs[0]
            # Full excerpts for the first run; the others are equal to it when `identical`.
            record["variants"][variant] = {
                "variant_file_sha256": esps[variant][0], "esp_sha256": esps[variant][1],
                "runs": runs[:1] + [{key: item[key] for key in ("exit", "retired", "rip", "com1_sha256")}
                                    for item in runs[1:]],
                "records_identical": identical,
                "efivars_names": first["efivars_names"],
                "dmesg_acpi_error_lines": first["dmesg_acpi_error_lines"],
            }
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"records identical across runs: {all_equal}; wrote {args.out}")
        if not all_equal:
            raise SystemExit(2)


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, StopIteration, ValueError, subprocess.CalledProcessError) as error:
        print(f"efivars/acpi record: {error!r}", file=sys.stderr)
        raise SystemExit(1) from error
