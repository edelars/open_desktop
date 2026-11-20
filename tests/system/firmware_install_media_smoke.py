#!/usr/bin/env python3
"""Convert an owned ISO-9660 fixture and boot its EFI app through the stock firmware."""

from __future__ import annotations

import fcntl
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
import zlib

from firmware_esp_boot_smoke import APPS, BUILDER, DISK, HELLO, ROOT, STOP, TARGET, boot, digest, require

SECTOR = 512
ISO_SECTOR = 2048
README = b"OD-UEFI FAT32 ESP README\r\n"


def convert(tool, source, output, expected, reason):
    result = subprocess.run([tool, "import-install-media", source, output],
                            cwd=ROOT, capture_output=True, text=True)
    fields = dict(line.split("=", 1) for line in result.stdout.splitlines())
    require(result.returncode == expected and fields.get("reason") == reason,
            f"unexpected import result {result.returncode}: {result.stdout}{result.stderr}")
    return fields


def verify_disk(path, app):
    with path.open("rb") as disk:
        def sectors(lba, count=1):
            disk.seek(lba * SECTOR)
            data = disk.read(count * SECTOR)
            require(len(data) == count * SECTOR, "truncated output disk")
            return data

        last = path.stat().st_size // SECTOR - 1
        require(sectors(0)[450] == 0xEE, "missing protective MBR")
        table = None
        for here, alternate in ((1, last), (last, 1)):
            header = bytearray(sectors(here))
            require(header[:8] == b"EFI PART", "missing GPT header")
            size, crc = struct.unpack_from("<II", header, 12)
            struct.pack_into("<I", header, 16, 0)
            require(size == 92 and zlib.crc32(header[:size]) == crc, "GPT header CRC")
            require(struct.unpack_from("<QQ", header, 24) == (here, alternate), "GPT header LBAs")
            at, entries, stride, entry_crc = struct.unpack_from("<QIII", header, 72)
            require((entries, stride) == (128, 128), "GPT entry geometry")
            current = sectors(at, 32)
            require(zlib.crc32(current) == entry_crc and (table is None or table == current),
                    "GPT tables differ or their CRC is wrong")
            table = current
        first, end = struct.unpack_from("<QQ", table, 32)
        require((first, end) == (2048, last - 33), "partition bounds")
        bpb = sectors(first)
        bps, spc, reserved, fat_count = struct.unpack_from("<HBHB", bpb, 11)
        total, fat_size = struct.unpack_from("<II", bpb, 32)
        root = struct.unpack_from("<I", bpb, 44)[0]
        require(bps == SECTOR and fat_count == 2 and total == end - first + 1,
                "FAT32 geometry")
        require(sectors(first + 6) == bpb and sectors(first + 7) == sectors(first + 1),
                "FAT32 backup BPB or FSInfo")
        fat = sectors(first + reserved, fat_size)
        require(sectors(first + reserved + fat_size, fat_size) == fat, "FAT32 copies differ")
        data = first + reserved + fat_count * fat_size

        def chain(cluster):
            seen = set()
            blocks = []
            while 2 <= cluster < 0x0FFFFFF8:
                require(cluster not in seen and cluster * 4 + 4 <= len(fat), "FAT cycle or bound")
                seen.add(cluster)
                blocks.append(sectors(data + (cluster - 2) * spc, spc))
                cluster = struct.unpack_from("<I", fat, cluster * 4)[0] & 0x0FFFFFFF
            require(cluster >= 0x0FFFFFF8, "FAT32 chain did not terminate")
            return b"".join(blocks)

        def locate(directory, name, attributes):
            for start in range(0, len(directory), 32):
                row = directory[start:start + 32]
                if row[0] == 0:
                    break
                if row[:11] == name:
                    hi, lo = struct.unpack_from("<H4xH", row, 20)
                    require(row[11] == attributes, f"wrong FAT attributes for {name!r}")
                    return hi << 16 | lo, struct.unpack_from("<I", row, 28)[0]
            raise RuntimeError(f"missing FAT entry {name!r}")

        root_rows = chain(root)
        efi, _ = locate(root_rows, b"EFI        ", 0x10)
        boot_dir, _ = locate(chain(efi), b"BOOT       ", 0x10)
        efi_app, length = locate(chain(boot_dir), b"BOOTX64 EFI", 0x20)
        note, note_len = locate(root_rows, b"README  TXT", 0x20)
        require(length == len(app) and chain(efi_app)[:length] == app, "EFI file bytes")
        require(note_len == len(README) and chain(note)[:note_len] == README, "README file bytes")


def check_rejection(tool, source, scratch, output):
    initial = digest(output)
    convert(tool, source, output, 5, "target_exists")
    require(initial == digest(output), "existing output changed")
    partial = scratch / "blocked.raw.partial"
    partial.write_bytes(b"existing staging data")
    convert(tool, source, scratch / "blocked.raw", 5, "partial_exists")
    require(partial.read_bytes() == b"existing staging data", "existing staging path changed")

    locked = scratch / "locked.raw"
    with source.open("rb") as lease:
        fcntl.flock(lease.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        convert(tool, source, locked, 4, "busy")
    require(not locked.exists() and not locked.with_suffix(".raw.partial").exists(),
            "busy source produced output")

    original = source.read_bytes()
    for name, at, replacement, code, reason in (
        ("endian", 16 * ISO_SECTOR + 84, b"\x01", 2, "iso_byte_order_fields"),
        ("file-version", 16 * ISO_SECTOR + 881, b"\x02", 3, "iso_file_structure_version"),
        ("record", 20 * ISO_SECTOR, b"\x01", 2, "iso_record_length"),
        ("udf", 18 * ISO_SECTOR, b"\0BEA01\x01", 3, "udf"),
        ("cycle", 20 * ISO_SECTOR + 68 + 2, struct.pack("<I", 20) + struct.pack(">I", 20),
         2, "iso_directory_extent"),
    ):
        bad = scratch / f"{name}.iso"
        target = scratch / f"{name}.raw"
        bad.write_bytes(original[:at] + replacement + original[at + len(replacement):])
        convert(tool, bad, target, code, reason)
        require(not target.exists() and not target.with_suffix(".raw.partial").exists(),
                f"{name} produced an output")
    print("Rejected malformed ISO fields, cyclic directory, UDF, busy source and existing paths.")


def main():
    target = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "release"
    subprocess.run(["cargo", "build", "--release", "--locked", "--offline",
                    "-p", "od-image-tool", "-p", "od-runner"], cwd=ROOT, check=True)
    with TemporaryDirectory(prefix="od-install-media-") as name:
        scratch = Path(name)
        rom = scratch / "firmware.bin"
        subprocess.run([sys.executable, BUILDER, "--out", rom, "--target-dir", scratch / "fw-target"],
                       cwd=ROOT, check=True)
        apps = scratch / "app-target"
        env = dict(os.environ, CARGO_TARGET_DIR=str(apps))
        env.pop("RUSTFLAGS", None)
        env.pop("CARGO_ENCODED_RUSTFLAGS", None)
        subprocess.run(["cargo", "build", "--release", "--locked", "--offline",
                        "--target", TARGET, "--bin", "hello"], cwd=APPS, env=env, check=True)
        app = apps / TARGET / "release/hello.efi"
        source = scratch / "fixture.iso"
        output = scratch / "converted.raw"
        subprocess.run([sys.executable, DISK, "--iso9660", "--bootx64", app, "--out", source],
                       cwd=ROOT, check=True)
        before = digest(source)
        tool = target / "od-image-tool"
        status = convert(tool, source, output, 0, "ok")
        require(status["published"] == "true" and status["verified"] == "true" and status["files"] == "2",
                "import did not verify two files and publish")
        require(int(status["logical_size"]) == output.stat().st_size, "reported size")
        require(not output.with_suffix(".raw.partial").exists(), "successful import left staging path")
        verify_disk(output, app.read_bytes())
        check_rejection(tool, source, scratch, output)
        require(digest(source) == before, "source ISO changed")
        trace, _, report = boot(target / "od-runner", rom, output, scratch, "converted-esp", None)
        transcript = trace.read_bytes()
        require(transcript.count(HELLO) == 1 and transcript.endswith(HELLO + STOP),
                "firmware did not start the converted EFI application")
        print(f"Converted ESP booted through NVMe: {report['retired']} instructions, {report['quanta']} quanta.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, KeyError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"install media smoke: {error}", file=sys.stderr)
        raise SystemExit(1) from error
