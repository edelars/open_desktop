#!/usr/bin/env python3
"""Exercise SetupMode, Auth2 enrollment, and a Secure Boot cold restart."""

import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
from uuid import UUID
import zlib

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "tools" / "build"
sys.path.insert(0, str(BUILD))
from make_test_disk import write_fat32
from sign_test_app import sign_image

HELLO = b"OD-UEFI hello 0.1.0\r\n"
BEFORE = b"OD-UEFI Secure Boot SetupMode=1 SecureBoot=0\r\n"
AFTER = b"OD-UEFI Secure Boot enrolled PK KEK db SetupMode=0 SecureBoot=1\r\n"
STOP = b"OD-FW no bootable option\r\nOD-FW STOP code=0x21 detail=0x0000000000000001\r\n"
GLOBAL = UUID("8be4df61-93ca-11d2-aa0d-00e098032b8c").bytes_le
DATABASE = UUID("d719b2cb-3d3a-4596-a3bc-dad00e67656f").bytes_le
X509 = UUID("a5c059a1-94e4-4aa7-87b5-ab155c2bf072").bytes_le
PKCS7 = UUID("4aafd29d-68df-49ee-8aa9-347d375665a7").bytes_le
OWNER = UUID("407ca603-5d60-42cf-9614-3c99d9abd16e").bytes_le
CERTIFICATE_CONFIG = """[req]
distinguished_name=identity
x509_extensions=usage
prompt=no
[identity]
CN=Open Desktop Secure Boot test
[usage]
keyUsage=digitalSignature
extendedKeyUsage=codeSigning
basicConstraints=critical,CA:FALSE
"""


def require(condition, detail):
    if not condition:
        raise RuntimeError(detail)


def call(args, **options):
    return subprocess.run(args, cwd=ROOT, check=True, **options)


def build_inputs(work):
    runner = ROOT / os.environ.get("CARGO_TARGET_DIR", "target") / "release" / "od-runner"
    call(["cargo", "build", "-p", "od-runner", "--release", "--offline"])
    rom = work / "firmware.bin"
    call([sys.executable, BUILD / "build_firmware.py", "--out", rom,
          "--target-dir", work / "firmware-target"])
    target = work / "app-target"
    call(["cargo", "build", "--manifest-path", ROOT / "firmware/uefi/tests/apps/Cargo.toml",
          "--release", "--offline", "--target", "x86_64-unknown-uefi", "--target-dir", target,
          "--bin", "hello", "--bin", "enroll"])
    return runner, rom, target / "x86_64-unknown-uefi/release"


def signature_list(cert):
    return X509 + struct.pack("<III", 44 + len(cert), 0, 16 + len(cert)) + OWNER + cert


def setup_auth2(value):
    time = struct.pack("<HBBBBBBIhBB", 2026, 11, 19, 22, 43, 1, 0, 0, 0, 0, 0)
    return time + struct.pack("<IHH", 24, 0x0200, 0x0ef1) + PKCS7 + value


def make_disks(work, apps):
    hello = (apps / "hello.efi").read_bytes()
    enrollment = (apps / "enroll.efi").read_bytes()
    esls = {}
    # Keep private keys in a separate private directory, even if --work is retained.
    with TemporaryDirectory(prefix="od-s725-keys-") as directory:
        secret = Path(directory)
        config = secret / "openssl.cnf"
        config.write_text(CERTIFICATE_CONFIG, encoding="ascii")
        for name in ("PK", "KEK", "db"):
            cert = secret / f"{name}.crt"
            key = secret / f"{name}.key"
            call(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-config", config,
                  "-days", "1", "-keyout", key, "-out", cert], capture_output=True)
            der = call(["openssl", "x509", "-in", cert, "-outform", "DER"],
                       capture_output=True).stdout
            esls[name] = signature_list(der)
        signed = sign_image(hello, secret / "db.crt", secret / "db.key")
    write_fat32(work / "unsigned.raw", "none", ((b"BOOTX64 EFI", hello),))
    write_fat32(work / "signed.raw", "none", ((b"BOOTX64 EFI", signed),))
    files = tuple(zip((b"PK      AUT", b"KEK     AUT", b"DB      AUT"),
                      (setup_auth2(esls[name]) for name in ("PK", "KEK", "db")), strict=True))
    write_fat32(work / "enroll.raw", "none", ((b"BOOTX64 EFI", enrollment), *files))
    return {(GLOBAL, "PK"): esls["PK"], (GLOBAL, "KEK"): esls["KEK"], (DATABASE, "db"): esls["db"]}


def policy_snapshot(path):
    raw = path.read_bytes()
    bank_bytes = 16 + 256 * 1024
    require(len(raw) == 16 + 2 * bank_bytes
            and struct.unpack_from("<8sII", raw) == (b"ODNVBANK", 2, 0), "protected snapshot header")
    banks = []
    for offset in (16, 16 + bank_bytes):
        epoch, size, checksum = struct.unpack_from("<QII", raw, offset)
        if epoch:
            data = raw[offset + 16:offset + 16 + size]
            require(24 <= size <= 256 * 1024
                    and zlib.crc32(raw[offset:offset + 12] + data) == checksum, "bank CRC")
            banks.append((epoch, data))
    require(bool(banks) and len({generation for generation, _ in banks}) == len(banks),
            "snapshot needs a unique active generation")
    _, active = max(banks)
    magic, version, offset, size, checksum, _ = struct.unpack_from("<8sHHIII", active)
    require((magic, version, offset, size) == (b"ODVARSTR", 2, 24, len(active))
            and zlib.crc32(active[:16] + bytes(4) + active[20:24]) == checksum, "store header")
    values = {}
    while offset < size:
        length, complement, live, flags, units, attributes, value_length = struct.unpack_from(
            "<IIBBHII", active, offset)
        name_end = 40 + 2 * units
        require(length == name_end + (16 if flags else 0) + value_length
                and length ^ 0xffff_ffff == complement and offset + length <= size, "record size")
        record = active[offset:offset + length]
        if flags and live == 1:
            namespace = record[20:36]
            name = record[40:name_end].decode("utf-16le")
            require(attributes == 0x27 and (namespace, name) not in values, "protected EFI variable")
            values[namespace, name] = record[name_end + 16:]
        offset += length
    return values


def boot(runner, rom, work, name, disk, snapshot, expected, snapshot_out=None):
    trace = work / f"{name}.com1"
    command = [runner, "boot-firmware", "--image", rom, "--com1-out", trace,
               "--budget", "20000000", "--nvram-in", snapshot, "--nvme-image", work / disk]
    if snapshot_out is not None:
        command.extend(["--nvram-out", snapshot_out])
    result = subprocess.run(command, cwd=ROOT, capture_output=True, check=False)
    (work / f"{name}.json").write_bytes(result.stdout)
    report = json.loads(result.stdout)
    require(result.returncode == 6 and report["exit"] == "halted"
            and report["status"] == "firmware_stop"
            and report["fw_stop"] == {"code": "0x21", "detail": "0x0000000000000001"},
            f"{name}: {report}")
    serial = trace.read_bytes()
    require(serial.endswith(b"".join(expected) + STOP)
            and all(serial.count(marker) == expected.count(marker) for marker in (HELLO, BEFORE, AFTER)),
            f"{name}: unexpected COM1 {serial[-400:]!r}")
    print(f"{name}: {report['retired']} instructions, {report['quanta']} quanta", flush=True)


def run(runner, rom, apps, work, repetitions):
    expected = make_disks(work, apps)
    for index in range(1, repetitions + 1):
        setup = work / f"setup-{index}.nv"
        enrolled = work / f"enrolled-{index}.nv"
        defaults = work / f"defaults-{index}.nv"
        call([runner, "prepare-secure-boot-nvram", "setup", "--out", setup], capture_output=True)
        require(policy_snapshot(setup) == {}, "SetupMode begins without enrolled keys")
        boot(runner, rom, work, f"setup-{index}", "unsigned.raw", setup, [HELLO])
        boot(runner, rom, work, f"enroll-{index}", "enroll.raw", setup, [BEFORE, AFTER], enrolled)
        require(policy_snapshot(enrolled) == expected, "enrolled PK, KEK and db match public fixtures")
        boot(runner, rom, work, f"signed-{index}", "signed.raw", enrolled, [HELLO])
        boot(runner, rom, work, f"unsigned-{index}", "unsigned.raw", enrolled, [])
        call([runner, "prepare-secure-boot-nvram", "defaults", "--out", defaults], capture_output=True)
        require(set(policy_snapshot(defaults)) == {(GLOBAL, "PK"), (GLOBAL, "KEK"),
                                                   (DATABASE, "db"), (DATABASE, "dbx")},
                "default PK, KEK, db and dbx")
        boot(runner, rom, work, f"foreign-{index}", "signed.raw", defaults, [])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, help="empty directory for retained builds, public fixtures and traces")
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--runner", type=Path)
    parser.add_argument("--rom", type=Path)
    parser.add_argument("--apps", type=Path)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("repetitions must be positive")
    supplied = [args.runner, args.rom, args.apps]
    if any(supplied) and not all(supplied):
        parser.error("--runner, --rom and --apps are supplied together")
    with TemporaryDirectory(prefix="od-firmware-secure-boot-") as scratch:
        work = args.work.resolve() if args.work else Path(scratch)
        work.mkdir(parents=True, exist_ok=True)
        if any(work.iterdir()):
            parser.error("--work must be empty")
        runner, rom, apps = (tuple(path.resolve() for path in supplied)
                             if all(supplied) else build_inputs(work))
        run(runner, rom, apps, work, args.repetitions)


if __name__ == "__main__":
    try:
        main()
    except (KeyError, OSError, RuntimeError, ValueError, struct.error, subprocess.CalledProcessError) as error:
        raise SystemExit(f"Secure Boot enrollment smoke: {error}") from error
