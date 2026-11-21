#!/usr/bin/env python3
"""Boot a stock ROM from a generated NVMe ESP and exercise persisted boot policy."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import uuid
import zlib

from firmware_boot_smoke import check_transcript

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests" / "fixtures" / "firmware"
BUILDER = ROOT / "tools" / "build" / "build_firmware.py"
DISK = ROOT / "tools" / "build" / "make_test_disk.py"
APPS = ROOT / "firmware" / "uefi" / "tests" / "apps"
TARGET = "x86_64-unknown-uefi"
GLOBAL = uuid.UUID("8be4df61-93ca-11d2-aa0d-00e098032b8c").bytes_le
END = b"\x7f\xff\x04\0"
HELLO = b"OD-UEFI hello 0.1.0\r\n"
NEXT = b"OD-UEFI ESP BootNext, BootCurrent and LoadOptions verified\r\n"
ORDER = b"OD-UEFI ESP BootOrder, BootCurrent and LoadOptions verified\r\n"
STOP = b"OD-FW no bootable option\r\nOD-FW STOP code=0x21 detail=0x0000000000000001\r\n"
BUDGET = 12_000_000
MAX_QUANTA = 5000
STORE_BYTES = 256 * 1024
BANK_BYTES = 16 + STORE_BYTES


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def digest(path: Path) -> str:
    sha = hashlib.sha256()
    with path.open("rb") as data:
        for chunk in iter(lambda: data.read(64 * 1024), b""):
            sha.update(chunk)
    return sha.hexdigest()


def file_node(name: str) -> bytes:
    value = ("\\EFI\\BOOT\\" + name + "\0").encode("utf-16le")
    return struct.pack("<BBH", 4, 4, 4 + len(value)) + value


def hard_drive_node(disk: Path) -> bytes:
    with disk.open("rb") as source:
        source.seek(2 * 512)
        entry = source.read(128)
    first, last = struct.unpack_from("<QQ", entry, 32)
    return struct.pack("<BBHIQQ16sBB", 4, 1, 42, 1, first, last - first + 1, entry[16:32], 2, 2)


def load_option(nodes: bytes, options: bytes = b"", *, active: bool = True) -> bytes:
    path = nodes + END
    return struct.pack("<IH", int(active), len(path)) + "Fixture\0".encode("utf-16le") + path + options


def encode_snapshot(entries: list[tuple[str, bytes]]) -> bytes:
    """Build an ODNVBANK v1 snapshot with one committed test variable store."""
    records = []
    for name, value in entries:
        encoded_name = name.encode("utf-16le")
        size = 40 + len(encoded_name) + len(value)
        fixed = struct.pack("<IIBBHII16sI", size, size ^ 0xffffffff, 1, 0, len(name),
                            7, len(value), GLOBAL, 0)
        record = bytearray(fixed + encoded_name + value)
        struct.pack_into("<I", record, 36, zlib.crc32(record))
        records.append(record)
    body = b"".join(records)
    header = bytearray(struct.pack("<8sHHIII", b"ODVARSTR", 1, 24, 24 + len(body), 0, 0))
    struct.pack_into("<I", header, 16, zlib.crc32(header))
    payload = header + body
    identity = struct.pack("<QI", 1, len(payload))
    bank = identity + struct.pack("<I", zlib.crc32(identity + payload)) + payload
    return struct.pack("<8sII", b"ODNVBANK", 1, 0) + bank + bytes(2 * BANK_BYTES - len(bank))


def decode_snapshot(snapshot: Path) -> dict[str, bytes]:
    image = snapshot.read_bytes()
    require(len(image) == 16 + 2 * BANK_BYTES
            and struct.unpack_from("<8sII", image) == (b"ODNVBANK", 1, 0), "NVRAM bank header")
    banks = []
    for offset in (16, 16 + BANK_BYTES):
        generation, length, crc = struct.unpack_from("<QII", image, offset)
        if generation == 0:
            continue
        data = image[offset + 16:offset + 16 + length]
        require(0 < length <= STORE_BYTES and zlib.crc32(image[offset:offset + 12] + data) == crc,
                "NVRAM bank CRC")
        banks.append((generation, data))
    require(bool(banks) and len({generation for generation, _ in banks}) == len(banks),
            "NVRAM needs one highest generation")
    _, data = max(banks)
    magic, version, offset, total, crc, reserved = struct.unpack_from("<8sHHIII", data)
    require((magic, version, offset, total, reserved) == (b"ODVARSTR", 1, 24, len(data), 0),
            "unexpected NVRAM snapshot header")
    require(zlib.crc32(data[:16] + b"\0" * 4 + data[20:24]) == crc, "NVRAM header CRC")
    variables = {}
    while offset < total:
        size, complement, live, padding, units, attrs, length, guid, crc = struct.unpack_from(
            "<IIBBHII16sI", data, offset
        )
        require(size == 40 + 2 * units + length and size <= total - offset
                and complement == size ^ 0xffffffff and live == 1 and padding == 0 and attrs & 1,
                "unexpected NVRAM record")
        record = data[offset:offset + size]
        require(zlib.crc32(record[:36] + b"\0" * 4 + record[40:]) == crc, "NVRAM record CRC")
        if guid == GLOBAL:
            name = record[40:40 + 2 * units].decode("utf-16le")
            require(name not in variables, "duplicate variable in NVRAM snapshot")
            variables[name] = record[40 + 2 * units:]
        offset += size
    return variables


def build_disks(scratch: Path, app_dir: Path) -> tuple[Path, Path]:
    esp = scratch / "esp.raw"
    raw = scratch / "no-esp.raw"
    subprocess.run([
        sys.executable, DISK, "--esp-fat32", "--bootx64", app_dir / "hello.efi",
        "--boot-probe", app_dir / "esp-boot-probe.efi", "--out", esp,
    ], check=True, cwd=ROOT)
    subprocess.run([sys.executable, DISK, "--damage", "both", "--out", raw], check=True, cwd=ROOT)
    return esp, raw


def inputs(scratch: Path, esp: Path) -> tuple[Path, Path]:
    missing = scratch / "missing.nv"
    missing.write_bytes(encode_snapshot([
        ("Boot0003", load_option(file_node("MISSING.EFI"))),
        ("BootOrder", struct.pack("<H", 3)),
    ]))
    ordered = scratch / "ordered.nv"
    ordered.write_bytes(encode_snapshot([
        ("Boot000A", load_option(hard_drive_node(esp) + file_node("CHECK.EFI"), b"next")),
        ("Boot000B", load_option(file_node("CHECK.EFI"), b"order")),
        ("Boot000C", load_option(file_node("BOOTX64.EFI"), active=False)),
        ("Boot000D", b"\x01\0\0\0\x20\0invalid"),
        ("BootNext", struct.pack("<H", 10)),
        ("BootOrder", struct.pack("<HHH", 12, 13, 11)),
    ]))
    return missing, ordered


def boot(runner: Path, rom: Path, disk: Path, scratch: Path, name: str, seed: Path | None):
    trace = scratch / f"{name}.com1"
    snapshot = scratch / f"{name}-after.nv"
    args = [runner, "boot-firmware", "--image", rom, "--nvme-image", disk,
            "--budget", str(BUDGET), "--com1-out", trace, "--nvram-out", snapshot]
    if seed is not None:
        args.extend(["--nvram-in", seed])
    result = subprocess.run(args, capture_output=True, text=True, check=False, cwd=ROOT)
    report = json.loads(result.stdout)
    require(result.returncode == 6 and report["status"] == "firmware_stop"
            and report["exit"] == "halted"
            and report["fw_stop"] == {"code": "0x21", "detail": "0x0000000000000001"}
            and report["memory_error"] is None and report["page_fault"] is None
            and 0 < report["retired"] <= BUDGET and 0 < report["quanta"] <= MAX_QUANTA,
            f"{name} did not reach boot exhaustion: {report}")
    require(report["image_sha256"] == digest(rom), f"{name} used the wrong ROM")
    require(report["nvram_snapshot_sha256"] == digest(snapshot), f"{name} NVRAM output digest")
    return trace, snapshot, report


def verify_variables(snapshot: Path, numbers: list[int], preserved: dict[str, bytes]) -> None:
    data = decode_snapshot(snapshot)
    require("BootNext" not in data and "BootCurrent" not in data,
            f"{snapshot.name} persisted a consumed or volatile variable")
    require(all(data.get(name) == value for name, value in preserved.items()),
            f"{snapshot.name} rewrote an existing boot option")
    if not numbers:
        require("BootOrder" not in data and "Boot0000" not in data, "boot option created without ESP")
        return
    require(data.get("BootOrder") == struct.pack(f"<{len(numbers)}H", *numbers),
            f"{snapshot.name} BootOrder differs")
    automatic = data.get("Boot0000", b"")
    require(automatic.startswith(b"\x01\0\0\0") and automatic.endswith(file_node("BOOTX64.EFI") + END),
            f"{snapshot.name} did not persist the full default path")


def run_cases(runner: Path, rom: Path, scratch: Path, app_dir: Path):
    esp, raw = build_disks(scratch, app_dir)
    missing, ordered = inputs(scratch, esp)
    reboot_seed = scratch / "esp_order-after.nv"
    for name, disk, seed, messages, numbers in [
        ("esp_default", esp, None, HELLO, [0]),
        ("esp_missing", raw, None, b"", []),
        ("esp_fallback", esp, missing, HELLO, [3, 0]),
        ("esp_order", esp, ordered, NEXT + ORDER + HELLO, [12, 13, 11, 14, 0]),
        ("esp_reboot", esp, reboot_seed, ORDER + HELLO, [12, 13, 11, 14, 0]),
    ]:
        trace, snapshot, report = boot(runner, rom, disk, scratch, name, seed)
        data = trace.read_bytes()
        require(data.endswith(messages + STOP), f"{name} boot output order differs: {data[-400:]!r}")
        for marker in (HELLO, NEXT, ORDER):
            require(data.count(marker) == messages.count(marker), f"{name} repeated or skipped {marker!r}")
        preserved = {} if seed is None else {
            key: value for key, value in decode_snapshot(seed).items()
            if key not in ("BootNext", "BootOrder") and key.startswith("Boot")
        }
        verify_variables(snapshot, numbers, preserved)
        print(f"{name} {report['retired']} instructions, {report['quanta']} quanta", flush=True)
        yield name, trace, snapshot, disk


def main() -> None:
    manifest = json.loads((FIXTURES / "esp_boot_manifest.json").read_text(encoding="utf-8"))
    target = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    runner = target / "release" / "od-runner"
    subprocess.run(["cargo", "build", "--release", "-p", "od-runner", "--offline"], check=True, cwd=ROOT)
    with tempfile.TemporaryDirectory(prefix="od-esp-boot-") as directory:
        scratch = Path(directory)
        rom = scratch / "stock.bin"
        subprocess.run([sys.executable, BUILDER, "--out", rom, "--target-dir", scratch / "rom"],
                       check=True, cwd=ROOT)
        app_target = scratch / "apps"
        environment = dict(os.environ, CARGO_TARGET_DIR=str(app_target))
        environment.pop("RUSTFLAGS", None)
        environment.pop("CARGO_ENCODED_RUSTFLAGS", None)
        subprocess.run(["cargo", "build", "--release", "--target", TARGET, "--locked", "--offline",
                        "--bin", "hello", "--bin", "esp-boot-probe"],
                       check=True, cwd=APPS, env=environment)
        for name, trace, snapshot, disk in run_cases(runner, rom, scratch, app_target / TARGET / "release"):
            expected = manifest[name]
            require(digest(disk) == expected["disk_sha256"], f"{name} disk fixture SHA-256")
            require(digest(snapshot) == expected["nvram_sha256"], f"{name} NVRAM fixture SHA-256")
            check_transcript(trace.read_bytes(), (FIXTURES / expected["transcript"]).read_bytes(), name)


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f"ESP boot smoke: {error}", file=sys.stderr)
        raise SystemExit(1) from error
