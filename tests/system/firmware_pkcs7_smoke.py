#!/usr/bin/env python3
"""Exercise the guest PKCS7 Verify Protocol under the protected Secure Boot profile."""

import argparse
from datetime import UTC, datetime, timedelta
from hashlib import sha256
import os
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory
from uuid import UUID

from firmware_secure_boot_smoke import (
    AFTER, APPS, BEFORE, BUILD, ROOT, SHA256, X509, auth2, auth2_time, boot, call,
    file_node, load_option, read_snapshot, require, sign_image, signature_list,
    write_fat32,
)
from sign_test_app import der

PROBE_PASSED = b"OD-UEFI PKCS7 VerifyBuffer, VerifySignature and timed revocation verified\r\n"
TBS_SHA256 = UUID("3bd2a492-96c0-4079-b420-fcf98ef103ed").bytes_le
TIMESTAMP_ATTRIBUTE = bytes.fromhex("060b2a864886f70d010910020e")
TARGET = "x86_64-unknown-uefi"


def fields(encoded):
    """Split test-generated DER into exact TLVs, retaining raw bytes."""
    results = []
    cursor = 0
    while cursor < len(encoded):
        start = cursor
        require(len(encoded) - cursor >= 2, "short DER header in fixture")
        tag, size = encoded[cursor:cursor + 2]
        cursor += 2
        if size & 0x80:
            width = size & 0x7f
            require(0 < width <= 4 and cursor + width <= len(encoded), "invalid DER length")
            size = int.from_bytes(encoded[cursor:cursor + width], "big")
            cursor += width
        end = cursor + size
        require(end <= len(encoded), "truncated DER field in fixture")
        results.append((tag, encoded[cursor:end], encoded[start:end]))
        cursor = end
    return results


def content_info(encoded):
    [(tag, body, _)] = fields(encoded)
    require(tag == 0x30, "expected ContentInfo")
    oid, explicit = fields(body)
    [(tag, body, _)] = fields(explicit[1])
    require(oid[0] == 6 and explicit[0] == 0xa0 and tag == 0x30, "expected SignedData")
    return oid[2], fields(body)


def signer_info(encoded):
    oid, signed = content_info(encoded)
    require(signed[-1][0] == 0x31, "expected one SignerInfo")
    [(tag, body, _)] = fields(signed[-1][1])
    require(tag == 0x30, "expected SignerInfo sequence")
    return oid, signed, fields(body)


def attach_timestamp(encoded, token):
    oid, signed, signer = signer_info(encoded)
    require(signer[-1][0] == 4, "expected final RSA signature field")
    attribute = der(0x30, TIMESTAMP_ATTRIBUTE + der(0x31, token))
    signer = der(0x30, b"".join(item[2] for item in signer) + der(0xa1, attribute))
    signed = b"".join(item[2] for item in signed[:-1]) + der(0x31, signer)
    return der(0x30, oid + der(0xa0, der(0x30, signed)))


def cutoff(token, offset):
    _, signed = content_info(token)
    _, embedded = fields(signed[2][1])
    [(tag, value, _)] = fields(embedded[1])
    require(tag == 4, "expected encapsulated TSTInfo")
    [(tag, value, _)] = fields(value)
    require(tag == 0x30, "expected TSTInfo sequence")
    generation_time = fields(value)[4][1].decode("ascii")
    fmt = "%Y%m%d%H%M%S.%fZ" if "." in generation_time else "%Y%m%d%H%M%SZ"
    utc = datetime.strptime(generation_time, fmt).replace(tzinfo=UTC) + offset
    return struct.pack("<H6BIh2B", utc.year, utc.month, utc.day, utc.hour, utc.minute,
                       utc.second, 0, utc.microsecond * 1000, 0, 0, 0)


def openssl(*args, cwd):
    return call(["openssl", *args], cwd=cwd, capture_output=True).stdout


def fixtures(work, keys):
    openssl("req", "-new", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256",
            "-days", "2", "-set_serial", "1", "-subj", "/CN=Open Desktop PKCS7 probe",
            "-keyout", "message.key", "-out", "message.pem", cwd=keys)
    openssl("req", "-new", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256",
            "-days", "2", "-set_serial", "2", "-subj", "/CN=Open Desktop test TSA root",
            "-addext", "basicConstraints=critical,CA:TRUE",
            "-addext", "keyUsage=critical,keyCertSign",
            "-keyout", "root.key", "-out", "root.pem", cwd=keys)
    openssl("req", "-new", "-newkey", "rsa:2048", "-nodes",
            "-subj", "/CN=Open Desktop test TSA", "-keyout", "tsa.key",
            "-out", "tsa.csr", cwd=keys)
    (keys / "tsa.conf").write_text(
        "[ext]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n"
        "extendedKeyUsage=critical,timeStamping\n"
        "[tsa]\nserial=ts.serial\nsigner_cert=tsa.pem\nsigner_key=tsa.key\n"
        "signer_digest=sha256\ndefault_policy=1.2.3.4.1\ndigests=sha256\n"
        "accuracy=secs:1,millisecs:5\nclock_precision_digits=6\nordering=yes\n"
        "tsa_name=yes\ness_cert_id_chain=no\ness_cert_id_alg=sha256\n",
        encoding="ascii",
    )
    (keys / "ts.serial").write_text("01\n", encoding="ascii")
    openssl("x509", "-req", "-in", "tsa.csr", "-CA", "root.pem", "-CAkey", "root.key",
            "-set_serial", "3", "-sha256", "-days", "2", "-extfile", "tsa.conf",
            "-extensions", "ext", "-out", "tsa.pem", cwd=keys)
    message = b"Open Desktop PKCS7 probe\0with binary bytes\xff\n"
    src = work / "message"
    src.write_bytes(message)
    digest = sha256(message).digest()
    (work / "digest").write_bytes(digest)
    signing = ("smime", "-sign", "-binary", "-in", src, "-signer", "message.pem",
               "-inkey", "message.key", "-md", "sha256", "-outform", "DER")
    embedded = openssl(*signing, "-nodetach", cwd=keys)
    detached = openssl(*signing, "-noattr", cwd=keys)
    alternate = openssl(*signing, "-noattr", "-nodetach", cwd=keys)
    (work / "embedded.der").write_bytes(embedded)
    (work / "detached.der").write_bytes(detached)
    _, _, signer = signer_info(embedded)
    require(signer[-1][0] == 4, "missing original signature value")
    original = keys / "original.sig"
    original.write_bytes(signer[-1][1])
    openssl("ts", "-query", "-data", original, "-sha256", "-cert", "-out", "request.tsq", cwd=keys)
    openssl("ts", "-reply", "-config", "tsa.conf", "-section", "tsa",
            "-queryfile", "request.tsq", "-out", "reply.tsr", cwd=keys)
    token = openssl("ts", "-reply", "-in", "reply.tsr", "-token_out", cwd=keys)
    (keys / "token.tst").write_bytes(token)
    openssl("ts", "-verify", "-token_in", "-in", "token.tst", "-data", original,
            "-CAfile", "root.pem", cwd=keys)
    (work / "timestamped.der").write_bytes(attach_timestamp(embedded, token))
    (work / "foreign-timestamp.der").write_bytes(attach_timestamp(alternate, token))
    certificate = openssl("x509", "-in", "message.pem", "-outform", "DER", cwd=keys)
    allowed = signature_list(X509, certificate)
    tbs = fields(fields(certificate)[0][1])[0][2]
    tbs_hash = sha256(tbs).digest()
    root = openssl("x509", "-in", "root.pem", "-outform", "DER", cwd=keys)
    (work / "allowed.esl").write_bytes(allowed)
    (work / "tsa.esl").write_bytes(signature_list(X509, root))
    (work / "message-revoked.esl").write_bytes(signature_list(SHA256, digest))
    for name, limit in (
        ("before", cutoff(token, timedelta(seconds=-1))),
        ("after", cutoff(token, timedelta(seconds=2))),
        ("always", bytes(16)),
    ):
        (work / f"revoked-{name}.esl").write_bytes(signature_list(TBS_SHA256, tbs_hash + limit))
    return allowed


def build_apps(work):
    target = work / "app-target"
    call(["cargo", "build", "--release", "--locked", "--offline", "--target", TARGET,
          "--target-dir", target, "--features", "pkcs7-probe", "--bin", "pkcs7-probe",
          "--bin", "enroll"], cwd=APPS, env={**os.environ, "OD_PKCS7_FIXTURE_DIR": str(work)})
    return target / TARGET / "release"


def exercise(runner, rom, work):
    with TemporaryDirectory(prefix="od-s724-pkcs7-keys-") as secrets:
        keys = Path(secrets)
        allowed = fixtures(work, keys)
        apps = build_apps(work)
        probe = sign_image((apps / "pkcs7-probe.efi").read_bytes(), keys / "message.pem",
                           keys / "message.key")
        enrollment = (apps / "enroll.efi").read_bytes()
    update = auth2(auth2_time(1), allowed)
    option = load_option(file_node("BOOTX64.EFI"))
    absent = load_option(file_node("ABSENT.EFI"))
    write_fat32(work / "enroll.raw", "none", (
        (b"BOOTX64 EFI", enrollment), (b"PK      AUT", update),
        (b"KEK     AUT", update), (b"DB      AUT", update),
        (b"BOOT000AOPT", option), (b"BOOT000BOPT", absent),
    ))
    write_fat32(work / "probe.raw", "none", ((b"BOOTX64 EFI", probe),))
    setup = work / "setup.nv"
    enrolled = work / "enrolled.nv"
    after = work / "after.nv"
    call([runner, "prepare-secure-boot-nvram", "setup", "--out", setup], capture_output=True)
    boot(runner, rom, work, "enroll", "enroll.raw", setup, [BEFORE, AFTER], enrolled)
    boot(runner, rom, work, "pkcs7", "probe.raw", enrolled, [PROBE_PASSED], after)
    require((work / "pkcs7.com1").read_bytes().count(PROBE_PASSED) == 1,
            "expected exactly one successful probe invocation")
    require(read_snapshot(enrolled) == read_snapshot(after), "PKCS7 changed the protected policy")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, help="empty directory for public fixtures and traces")
    parser.add_argument("--runner", type=Path)
    parser.add_argument("--rom", type=Path)
    args = parser.parse_args()
    if (args.runner is None) != (args.rom is None):
        parser.error("--runner and --rom must be supplied together")
    with TemporaryDirectory(prefix="od-firmware-pkcs7-") as scratch:
        work = args.work.resolve() if args.work else Path(scratch)
        work.mkdir(parents=True, exist_ok=True)
        require(not any(work.iterdir()), "work directory must be empty")
        if args.runner is None:
            call(["cargo", "build", "-p", "od-runner", "--release", "--offline"])
            runner = ROOT / os.environ.get("CARGO_TARGET_DIR", "target") / "release/od-runner"
            rom = work / "firmware.bin"
            call([sys.executable, BUILD / "build_firmware.py", "--out", rom,
                  "--target-dir", work / "firmware-target"])
        else:
            runner, rom = args.runner.resolve(), args.rom.resolve()
        exercise(runner, rom, work)


if __name__ == "__main__":
    main()
