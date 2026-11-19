#!/usr/bin/env python3
"""Build a small, deterministic project-owned GPT fixture for the UEFI blk app."""

from __future__ import annotations

import argparse
import struct
import uuid
import zlib
from pathlib import Path

BLOCK = 512
BLOCKS = 256
LAST = BLOCKS - 1
ENTRY_COUNT = 128
ENTRY_SIZE = 128
ARRAY_BLOCKS = ENTRY_COUNT * ENTRY_SIZE // BLOCK
BACKUP_ARRAY = LAST - ARRAY_BLOCKS
FIRST_USABLE, LAST_USABLE = 2 + ARRAY_BLOCKS, BACKUP_ARRAY - 1
DISK_ID = uuid.UUID("32345678-d0a7-4ee2-a14b-0277282a1f51")
ESP = uuid.UUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
BASIC_DATA = uuid.UUID("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7")
PARTITIONS = (
    (1, ESP, uuid.UUID("ffff0001-a120-4280-b011-160000000001"), 64, 95, 0, "OD ESP"),
    (2, BASIC_DATA, uuid.UUID("ffff0002-a120-4280-b011-160000000002"), 96, 111, 2, "OD Hidden"),
    (3, BASIC_DATA, uuid.UUID("ffff0003-a120-4280-b011-160000000003"), 128, 175, 0, "OD Data"),
)


def header(lba: int, alternate_lba: int, table_lba: int, table_crc: int) -> bytes:
    block = bytearray(BLOCK)
    struct.pack_into(
        "<8sIIIIQQQQ16sQIII", block, 0,
        b"EFI PART", 0x10000, 92, 0, 0, lba, alternate_lba,
        FIRST_USABLE, LAST_USABLE, DISK_ID.bytes_le, table_lba, ENTRY_COUNT, ENTRY_SIZE, table_crc,
    )
    struct.pack_into("<I", block, 16, zlib.crc32(block[:92]))
    return block


def image(damage: str) -> bytearray:
    disk = bytearray(BLOCK * BLOCKS)
    struct.pack_into("<B3sB3sII", disk, 446, 0, b"\x00\x02\x00", 0xEE, b"\xff" * 3, 1, LAST)
    disk[510:512] = b"\x55\xaa"
    entries = bytearray(ENTRY_COUNT * ENTRY_SIZE)
    for slot, kind, identifier, first, last, attributes, name in PARTITIONS:
        struct.pack_into(
            "<16s16sQQQ72s", entries, (slot - 1) * ENTRY_SIZE,
            kind.bytes_le, identifier.bytes_le, first, last, attributes, name.encode("utf-16le"),
        )
        marker = name.encode("ascii") + b"\0"
        disk[first * BLOCK:first * BLOCK + len(marker)] = marker
    table_crc = zlib.crc32(entries)
    disk[BLOCK:2 * BLOCK] = header(1, LAST, 2, table_crc)
    disk[2 * BLOCK:(2 + ARRAY_BLOCKS) * BLOCK] = entries
    disk[BACKUP_ARRAY * BLOCK:LAST * BLOCK] = entries
    disk[LAST * BLOCK:] = header(LAST, 1, BACKUP_ARRAY, table_crc)
    if damage in ("primary", "both"):
        disk[BLOCK + 16] ^= 1
    if damage == "both":
        disk[LAST * BLOCK + 16] ^= 1
    return disk


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--damage", choices=("none", "primary", "both"), default="none")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    args.out.write_bytes(image(args.damage))


if __name__ == "__main__":
    main()
