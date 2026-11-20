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

from firmware_esp_boot_smoke import file_node, load_option

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "tools" / "build"
APPS = ROOT / "firmware" / "uefi" / "tests" / "apps"
sys.path.insert(0, str(BUILD))
from make_test_disk import write_fat32
from sign_test_app import pe_digest, sign_image

HELLO = b"OD-UEFI hello 0.1.0\r\n"
BEFORE = b"OD-UEFI Secure Boot SetupMode=1 SecureBoot=0\r\n"
AFTER = b"OD-UEFI Secure Boot enrolled PK KEK db SetupMode=0 SecureBoot=1\r\n"
FALLBACK = b"OD-UEFI ESP BootOrder, BootCurrent and LoadOptions verified\r\n"
REVOKED = (b"OD-UEFI Secure Boot rejected unsigned at LoadImage and revoked at StartImage\r\n"
           b"OD-UEFI image execution table verified\r\n")
STOP = b"OD-FW no bootable option\r\nOD-FW STOP code=0x21 detail=0x0000000000000001\r\n"
GLOBAL = UUID("8be4df61-93ca-11d2-aa0d-00e098032b8c").bytes_le
DATABASE = UUID("d719b2cb-3d3a-4596-a3bc-dad00e67656f").bytes_le
X509 = UUID("a5c059a1-94e4-4aa7-87b5-ab155c2bf072").bytes_le
SHA256 = UUID("c1c41626-504c-4092-aca9-41f936934328").bytes_le
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


def call(args, *, cwd=ROOT, **options):
    return subprocess.run(args, cwd=cwd, check=True, **options)


def build_inputs(work):
    runner = ROOT / os.environ.get("CARGO_TARGET_DIR", "target") / "release" / "od-runner"
    call(["cargo", "build", "-p", "od-runner", "--release", "--offline"])
    rom = work / "firmware.bin"
    call([sys.executable, BUILD / "build_firmware.py", "--out", rom,
          "--target-dir", work / "firmware-target"])
    target = work / "app-target"
    call(["cargo", "build", "--release", "--locked", "--offline", "--target", "x86_64-unknown-uefi",
          "--target-dir", target, "--bin", "hello", "--bin", "enroll", "--bin", "esp-boot-probe"],
         cwd=APPS)
    return runner, rom, target / "x86_64-unknown-uefi/release"


def signature_list(kind, value):
    return kind + struct.pack("<III", 44 + len(value), 0, 16 + len(value)) + OWNER + value


def auth2_time(second):
    return struct.pack("<HBBBBBBIhBB", 2026, 11, 19, 22, 43, second, 0, 0, 0, 0, 0)


def auth2(time, value, signature=b""):
    return (time + struct.pack("<IHH", 24 + len(signature), 0x0200, 0x0ef1)
            + PKCS7 + signature + value)


def dbx_update(secret, value):
    time = auth2_time(2)
    message = "dbx".encode("utf-16le") + DATABASE + struct.pack("<I", 0x27) + time + value
    signature = call(["openssl", "smime", "-sign", "-binary", "-noattr", "-md", "sha256",
                      "-signer", secret / "KEK.crt", "-inkey", secret / "KEK.key",
                      "-outform", "DER"], input=message, capture_output=True).stdout
    return auth2(time, value, signature)


def make_disks(work, apps):
    hello = (apps / "hello.efi").read_bytes()
    enrollment = (apps / "enroll.efi").read_bytes()
    fallback = (apps / "esp-boot-probe.efi").read_bytes()
    revoked_hash, _ = pe_digest(hello + bytes(-len(hello) % 8))
    dbx = signature_list(SHA256, revoked_hash)
    esls = {}
    # Keep private keys in a separate private directory, even if --work is retained.
    with TemporaryDirectory(prefix="od-s725-keys-") as directory:
        secret = Path(directory)
        config = secret / "openssl.cnf"
        config.write_text(CERTIFICATE_CONFIG, encoding="ascii")
        for serial, name in enumerate(("PK", "KEK", "db"), start=0x80):
            cert = secret / f"{name}.crt"
            key = secret / f"{name}.key"
            call(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-config", config,
                  "-days", "1", "-set_serial", str(serial), "-keyout", key, "-out", cert],
                 capture_output=True)
            der = call(["openssl", "x509", "-in", cert, "-outform", "DER"],
                       capture_output=True).stdout
            esls[name] = signature_list(X509, der)
        signed = sign_image(hello, secret / "db.crt", secret / "db.key")
        signed_fallback = sign_image(fallback, secret / "db.crt", secret / "db.key")
        signed_updater = sign_image(enrollment, secret / "db.crt", secret / "db.key")
        revoke = dbx_update(secret, dbx)
    write_fat32(work / "unsigned.raw", "none", ((b"BOOTX64 EFI", hello),))
    write_fat32(work / "signed.raw", "none", ((b"BOOTX64 EFI", signed),))
    write_fat32(work / "fallback.raw", "none", ((b"BOOTX64 EFI", hello), (b"CHECK   EFI", signed_fallback)))
    write_fat32(work / "revoke.raw", "none", ((b"BOOTX64 EFI", signed_updater),
               (b"TARGET  EFI", signed), (b"UNSIGN  EFI", hello), (b"DBX     AUT", revoke)))
    files = tuple(zip((b"PK      AUT", b"KEK     AUT", b"DB      AUT"),
                      (auth2(auth2_time(1), esls[name]) for name in ("PK", "KEK", "db")), strict=True))
    boot_options = (
        (b"BOOT000AOPT", load_option(file_node("BOOTX64.EFI"))),
        (b"BOOT000BOPT", load_option(file_node("CHECK.EFI"), b"order")),
    )
    write_fat32(work / "enroll.raw", "none", ((b"BOOTX64 EFI", enrollment), *files, *boot_options))
    for name, esl in {**esls, "dbx": dbx}.items():
        (work / f"{name}.esl").write_bytes(esl)
    enrolled = {(GLOBAL, "PK"): esls["PK"], (GLOBAL, "KEK"): esls["KEK"], (DATABASE, "db"): esls["db"]}
    return enrolled, {**enrolled, (DATABASE, "dbx"): dbx}


def read_snapshot(path):
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
    boot_order = None
    while offset < size:
        length, complement, live, flags, units, attributes, value_length = struct.unpack_from(
            "<IIBBHII", active, offset)
        name_end = 40 + 2 * units
        require(length == name_end + (16 if flags else 0) + value_length
                and length ^ 0xffff_ffff == complement and offset + length <= size, "record size")
        record = active[offset:offset + length]
        if live == 1:
            namespace = record[20:36]
            name = record[40:name_end].decode("utf-16le")
            if flags:
                require(attributes == 0x27 and (namespace, name) not in values, "protected EFI variable")
                values[namespace, name] = record[name_end + 16:]
            elif (namespace, name) == (GLOBAL, "BootOrder"):
                require(attributes == 7 and boot_order is None and value_length % 2 == 0,
                        "BootOrder EFI variable")
                boot_order = record[name_end:]
        offset += length
    return values, boot_order


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
            and all(serial.count(marker) == expected.count(marker)
                    for marker in (HELLO, BEFORE, AFTER, FALLBACK, REVOKED)),
            f"{name}: unexpected COM1 {serial[-400:]!r}")
    print(f"{name}: {report['retired']} instructions, {report['quanta']} quanta", flush=True)


def run(runner, rom, apps, work, repetitions):
    expected, expected_revoked = make_disks(work, apps)
    for index in range(1, repetitions + 1):
        setup = work / f"setup-{index}.nv"
        enrolled = work / f"enrolled-{index}.nv"
        revoked = work / f"revoked-{index}.nv"
        defaults = work / f"defaults-{index}.nv"
        call([runner, "prepare-secure-boot-nvram", "setup", "--out", setup], capture_output=True)
        require(read_snapshot(setup) == ({}, None), "SetupMode begins with empty policy and BootOrder")
        boot(runner, rom, work, f"setup-{index}", "unsigned.raw", setup, [HELLO])
        boot(runner, rom, work, f"enroll-{index}", "enroll.raw", setup, [BEFORE, AFTER], enrolled)
        order = struct.pack("<HH", 10, 11)
        require(read_snapshot(enrolled) == (expected, order), "guest enrolled keys and BootOrder")
        boot(runner, rom, work, f"signed-{index}", "signed.raw", enrolled, [HELLO])
        boot(runner, rom, work, f"unsigned-{index}", "unsigned.raw", enrolled, [])
        boot(runner, rom, work, f"fallback-{index}", "fallback.raw", enrolled, [FALLBACK])
        boot(runner, rom, work, f"revoke-{index}", "revoke.raw", enrolled, [REVOKED], revoked)
        require(read_snapshot(revoked) == (expected_revoked, order), "authenticated dbx is retained")
        boot(runner, rom, work, f"revoked-{index}", "signed.raw", revoked, [])
        call([runner, "prepare-secure-boot-nvram", "defaults", "--out", defaults], capture_output=True)
        default_policy, default_order = read_snapshot(defaults)
        require(set(default_policy) == {(GLOBAL, "PK"), (GLOBAL, "KEK"),
                                        (DATABASE, "db"), (DATABASE, "dbx")} and default_order is None,
                "default policy with empty BootOrder")
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
