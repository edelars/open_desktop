"""Independent SHA-256 reference for Microsoft Authenticode v1.0 (2008)."""

import argparse
import hashlib
from pathlib import Path
import struct


def authenticode_sha256(image: bytes) -> str:
    if image[:2] != b"MZ":
        raise ValueError("missing DOS signature")

    pe = struct.unpack_from("<I", image, 0x3C)[0]
    if image[pe : pe + 4] != b"PE\0\0":
        raise ValueError("missing PE signature")

    sections = struct.unpack_from("<H", image, pe + 6)[0]
    optional_size = struct.unpack_from("<H", image, pe + 20)[0]
    optional = pe + 24
    if optional_size < 112 or struct.unpack_from("<H", image, optional)[0] != 0x20B:
        raise ValueError("expected a PE32+ optional header")

    checksum = optional + 64
    headers_size = struct.unpack_from("<I", image, optional + 60)[0]
    directory_count = struct.unpack_from("<I", image, optional + 108)[0]
    if optional_size < 112 + 8 * directory_count:
        raise ValueError("data directories exceed the optional header")

    security = optional + 112 + 8 * 4
    end = len(image)
    if directory_count > 4:
        certificate, certificate_size = struct.unpack_from("<II", image, security)
        if certificate_size:
            if certificate + certificate_size != end:
                raise ValueError("certificate table must end at end of file")
            end -= certificate_size
        header_bytes = image[:checksum] + image[checksum + 4 : security]
        header_bytes += image[security + 8 : headers_size]
    else:
        header_bytes = image[:checksum] + image[checksum + 4 : headers_size]

    records = [
        struct.unpack_from("<II", image, optional + optional_size + 40 * index + 16)
        for index in range(sections)
    ]
    file_ranges = sorted((offset, size) for size, offset in records if size)
    total = headers_size + sum(size for _, size in file_ranges)
    if total > end:
        raise ValueError("the bytes-hashed counter exceeds the signed file span")

    stream = header_bytes + b"".join(
        image[offset : offset + size] for offset, size in file_ranges
    )
    return hashlib.sha256(stream + image[total:end]).hexdigest()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    print(authenticode_sha256(args.image.read_bytes()))
