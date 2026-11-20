#!/usr/bin/env python3
"""Exercise bounded UEFI startup and runtime failures on the native firmware board."""

from __future__ import annotations

import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile

from firmware_esp_boot_smoke import APPS, BUILDER, DISK, ROOT, TARGET


def build(scratch: Path) -> Path:
    command = [sys.executable, BUILDER, "--target-dir", scratch / "firmware-target"]
    subprocess.run(command + ["--out", scratch / "firmware.bin"], check=True, cwd=ROOT)
    subprocess.run(
        command + ["--with-test-apps", "--out", scratch / "firmware-apps.bin"],
        check=True,
        cwd=ROOT,
    )
    app_target = scratch / "apps-target"
    subprocess.run(
        [
            "cargo", "build", "--release", "--locked", "--offline", "--target", TARGET,
            "--target-dir", app_target, "--bin", "hello", "--bin", "failure-probe",
        ],
        check=True,
        cwd=APPS,
    )
    return app_target / TARGET / "release"


def prepare_disks(scratch: Path, apps: Path) -> None:
    spec = importlib.util.spec_from_file_location("make_test_disk", DISK)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load the deterministic disk generator")
    disks = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(disks)
    hello = (apps / "hello.efi").read_bytes()
    probe = (apps / "failure-probe.efi").read_bytes()
    broken_pe = bytearray(hello)
    pe = struct.unpack_from("<I", broken_pe, 0x3C)[0]
    if broken_pe[pe:pe + 4] != b"PE\0\0":
        raise ValueError("hello.efi must be a PE image")
    optional_size = struct.unpack_from("<H", broken_pe, pe + 20)[0]
    first_section = pe + 24 + optional_size
    if broken_pe[first_section:first_section + 8] != b".text\0\0\0":
        raise ValueError("hello.efi first section must be .text")
    # Keep the PE parseable up to the section table, with an aligned raw extent outside the file.
    struct.pack_into("<I", broken_pe, first_section + 20, (len(broken_pe) + 1023) & ~511)
    boot_name = b"BOOTX64 EFI"
    fault_name = b"CHECK   EFI"
    disks.write_fat32(scratch / "healthy-esp.raw", "none", ((boot_name, hello),))
    disks.write_fat32(scratch / "bad-gpt.raw", "both", ((boot_name, hello),))
    disks.write_fat32(scratch / "bad-pe.raw", "none",
                      ((boot_name, probe), (fault_name, bytes(broken_pe))))

    cycle = scratch / "fat-cycle.raw"
    disks.write_fat32(cycle, "none", ((boot_name, probe), (fault_name, hello)))
    with cycle.open("r+b") as disk:
        disk.seek(disks.FAT32_START * disks.BLOCK)
        bpb = disk.read(disks.BLOCK)
        reserved = struct.unpack_from("<H", bpb, 14)[0]
        copies = bpb[16]
        fat_sectors = struct.unpack_from("<I", bpb, 36)[0]
        first_fat = disks.FAT32_START + reserved
        boot_dir = first_fat + copies * fat_sectors + disks.BOOT - 2
        disk.seek(boot_dir * disks.BLOCK)
        directory = disk.read(disks.BLOCK)
        entry = next(
            directory[offset:offset + 32]
            for offset in range(0, disks.BLOCK, 32)
            if directory[offset:offset + 11] == fault_name
        )
        cluster = (struct.unpack_from("<H", entry, 20)[0] << 16) | struct.unpack_from("<H", entry, 26)[0]
        if struct.unpack_from("<I", entry, 28)[0] <= disks.BLOCK:
            raise ValueError("the FAT cycle fixture needs a file spanning clusters")
        for copy in range(copies):
            disk.seek((first_fat + copy * fat_sectors) * disks.BLOCK + 4 * cluster)
            disk.write(struct.pack("<I", cluster))

    disks.write_fat32(scratch / "failure-probe.raw", "none", ((boot_name, probe),))


def exercise(args: argparse.Namespace, scratch: Path) -> None:
    if args.rom is None:
        app_dir = build(scratch)
    else:
        shutil.copyfile(args.rom, scratch / "firmware.bin")
        shutil.copyfile(args.apps_rom, scratch / "firmware-apps.bin")
        app_dir = args.app_dir
    prepare_disks(scratch, app_dir)
    environment = os.environ | {"OD_FW_FAILURE_FIXTURES": str(scratch)}
    for repetition in range(1, args.repetitions + 1):
        print(f"Firmware failure pass {repetition}/{args.repetitions}", flush=True)
        subprocess.run(
            [
                "cargo", "test", "--release", "--offline", "-p", "od-machine", "--lib",
                "firmware_failure_smoke_without_tpm_or_secure_boot",
                "--", "--ignored", "--nocapture",
            ],
            check=True,
            env=environment,
            cwd=ROOT,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rom", type=Path)
    parser.add_argument("--apps-rom", type=Path)
    parser.add_argument("--app-dir", type=Path)
    parser.add_argument("--work", type=Path, help="retain fixtures in an empty directory")
    parser.add_argument("--repetitions", type=int, default=3)
    args = parser.parse_args()
    supplied = sum(value is not None for value in (args.rom, args.apps_rom, args.app_dir))
    if supplied not in (0, 3):
        parser.error("--rom, --apps-rom and --app-dir must be supplied together")
    if args.repetitions < 1:
        parser.error("--repetitions must be positive")
    if args.work is not None:
        scratch = args.work.resolve()
        scratch.mkdir(parents=True, exist_ok=True)
        if any(scratch.iterdir()):
            parser.error("--work must be empty")
        exercise(args, scratch)
    else:
        with tempfile.TemporaryDirectory(prefix="od-firmware-failure-") as directory:
            exercise(args, Path(directory))


if __name__ == "__main__":
    main()
