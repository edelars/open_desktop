#!/usr/bin/env python3
"""Attach a test Authenticode signature to an unsigned x64 EFI application."""

import argparse
from hashlib import sha256
from pathlib import Path
import struct
import subprocess


def der(tag: int, body: bytes) -> bytes:
    size = len(body)
    if size < 128:
        return bytes([tag, size]) + body
    length = size.to_bytes((size.bit_length() + 7) // 8, "big")
    return bytes([tag, 0x80 | len(length)]) + length + body


def oid(value: str) -> bytes:
    first, second, *tail = map(int, value.split("."))
    body = bytearray()
    for arc in (40 * first + second, *tail):
        digits = [arc & 0x7f]
        arc >>= 7
        while arc:
            digits.append(0x80 | (arc & 0x7f))
            arc >>= 7
        body.extend(reversed(digits))
    return der(6, bytes(body))


def elements(data: bytes):
    at = 0
    while at < len(data):
        if len(data) - at < 2:
            raise ValueError("truncated DER element")
        start = at
        tag, size = data[at:at + 2]
        at += 2
        if size & 0x80:
            width = size & 0x7f
            length = data[at:at + width]
            if not 1 <= width <= 4 or len(length) != width or not length[0]:
                raise ValueError("invalid DER length")
            size = int.from_bytes(length, "big")
            if size < 128:
                raise ValueError("nonminimal DER length")
            at += width
        end = at + size
        if end > len(data):
            raise ValueError("truncated DER contents")
        yield tag, data[at:end], data[start:end]
        at = end


def issuer_and_serial(certificate: bytes) -> bytes:
    outer, = list(elements(certificate))
    tbs, *_ = list(elements(outer[1]))
    if outer[0] != 0x30 or tbs[0] != 0x30:
        raise ValueError("expected an X.509 certificate")
    fields = list(elements(tbs[1]))
    if fields and fields[0][0] == 0xa0:
        fields = fields[1:]
    if len(fields) < 3 or fields[0][0] != 2 or fields[2][0] != 0x30:
        raise ValueError("X.509 certificate lacks its issuer or serial number")
    return der(0x30, fields[2][2] + fields[0][2])


def pe_digest(image: bytes) -> tuple[bytes, int]:
    if not image.startswith(b"MZ"):
        raise ValueError("expected a PE file")
    pe, = struct.unpack_from("<I", image, 0x3c)
    if image[pe:pe + 6] != b"PE\0\0\x64\x86":
        raise ValueError("expected an AMD64 PE file")
    section_count, = struct.unpack_from("<H", image, pe + 6)
    optional_size, = struct.unpack_from("<H", image, pe + 20)
    optional = pe + 24
    headers, = struct.unpack_from("<I", image, optional + 60)
    checksum = optional + 64
    security = optional + 112 + 4 * 8
    section_table = optional + optional_size
    if (image[optional:optional + 2] != b"\x0b\x02"
            or struct.unpack_from("<I", image, optional + 108)[0] < 5
            or not security + 8 <= section_table <= headers <= len(image)
            or section_table + 40 * section_count > headers):
        raise ValueError("invalid PE32+ headers or missing security directory")
    if any(image[security:security + 8]):
        raise ValueError("the input PE already has a certificate table")
    raw = []
    for offset in range(section_table, section_table + 40 * section_count, 40):
        size, start = struct.unpack_from("<II", image, offset + 16)
        if size:
            raw.append((start, size))
    digest = sha256(image[:checksum])
    digest.update(image[checksum + 4:security])
    digest.update(image[security + 8:headers])
    end = headers
    for start, size in sorted(raw):
        if start != end or start + size > len(image):
            raise ValueError("test PE must have contiguous raw sections within the file")
        end += size
        digest.update(image[start:end])
    digest.update(image[end:])
    return digest.digest(), security


def openssl(program: str, *args, data: bytes | None = None) -> bytes:
    result = subprocess.run([program, *args], input=data, capture_output=True, check=False)
    if result.returncode:
        raise ValueError(result.stderr.decode(errors="replace").strip())
    return result.stdout


def sign_image(image: bytes, certificate: Path, key: Path, program: str = "openssl") -> bytes:
    image = bytearray(image + bytes(-len(image) % 8))
    digest, security = pe_digest(image)
    cert = openssl(program, "x509", "-in", certificate, "-outform", "DER")
    pubkey = openssl(program, "x509", "-in", certificate, "-pubkey", "-noout")
    if pubkey != openssl(program, "pkey", "-in", key, "-pubout"):
        raise ValueError("the signing key does not match the certificate")

    # A PKCS#7 ContentInfo for Authenticode embeds the SPC SEQUENCE directly.
    # openssl cms wraps encapsulated content in a CMS OCTET STRING instead.
    algorithm = der(0x30, oid("2.16.840.1.101.3.4.2.1") + der(5, b""))
    spc = oid("1.3.6.1.4.1.311.2.1.4")
    pe_data = der(0x30, oid("1.3.6.1.4.1.311.2.1.15") + der(0xa0, der(0x30, der(3, b"\0"))))
    indirect = pe_data + der(0x30, algorithm + der(4, digest))
    content = der(0x30, spc + der(0xa0, der(0x30, indirect)))
    attributes = b"".join(sorted([
        der(0x30, oid("1.2.840.113549.1.9.3") + der(0x31, spc)),
        der(0x30, oid("1.2.840.113549.1.9.4") + der(0x31, der(4, sha256(indirect).digest()))),
    ]))
    signature = openssl(program, "dgst", "-sha256", "-sign", key,
                        "-sigopt", "rsa_padding_mode:pkcs1", data=der(0x31, attributes))
    if len(signature) != 256:
        raise ValueError("expected an RSA-2048 PKCS#1 v1.5 signature")
    version = der(2, b"\x01")
    rsa = der(0x30, oid("1.2.840.113549.1.1.1") + der(5, b""))
    signer = der(0x30, version + issuer_and_serial(cert) + algorithm
                 + der(0xa0, attributes) + rsa + der(4, signature))
    signed = der(0x30, version + der(0x31, algorithm) + content + der(0xa0, cert) + der(0x31, signer))
    pkcs7 = der(0x30, oid("1.2.840.113549.1.7.2") + der(0xa0, signed))

    payload = pkcs7 + bytes(-len(pkcs7) % 8)
    entry = struct.pack("<IHH", 8 + len(payload), 0x0200, 2) + payload
    struct.pack_into("<II", image, security, len(image), len(entry))
    return bytes(image) + entry


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path, help="unsigned PE32+ test application")
    parser.add_argument("--certificate", required=True, type=Path, help="PEM test certificate")
    parser.add_argument("--key", required=True, type=Path, help="RSA-2048 private test key")
    parser.add_argument("--out", required=True, type=Path, help="new output PE file")
    parser.add_argument("--openssl", default="openssl", help="OpenSSL or LibreSSL executable")
    args = parser.parse_args()
    try:
        signed = sign_image(args.image.read_bytes(), args.certificate, args.key, args.openssl)
        with args.out.open("xb") as output:
            output.write(signed)
    except (OSError, ValueError, struct.error) as error:
        parser.exit(1, f"sign test app: {error}\n")


if __name__ == "__main__":
    main()
