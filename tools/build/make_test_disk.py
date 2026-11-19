#!/usr/bin/env python3
"""Generate the UEFI blk GPT fixture or the sparse FAT32 ESP fixture."""

from __future__ import annotations

import argparse
import struct
import uuid
import zlib
from pathlib import Path

BLOCK = 512
BLOCKS = 256
ENTRY_COUNT = 128
ENTRY_SIZE = 128
ARRAY_BLOCKS = ENTRY_COUNT * ENTRY_SIZE // BLOCK
DISK_ID = uuid.UUID("32345678-d0a7-4ee2-a14b-0277282a1f51")
ESP = uuid.UUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
BASIC_DATA = uuid.UUID("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7")
PARTITIONS = (
    (1, ESP, uuid.UUID("ffff0001-a120-4280-b011-160000000001"), 64, 95, 0, "OD ESP"),
    (2, BASIC_DATA, uuid.UUID("ffff0002-a120-4280-b011-160000000002"), 96, 111, 2, "OD Hidden"),
    (3, BASIC_DATA, uuid.UUID("ffff0003-a120-4280-b011-160000000003"), 128, 175, 0, "OD Data"),
)


FAT32_BLOCKS = 40 * 1024 * 1024 // BLOCK
FAT32_START = 2048
FAT32_END = FAT32_BLOCKS - ARRAY_BLOCKS - 2
FAT32_PARTITIONS = ((1, ESP, PARTITIONS[0][2], FAT32_START, FAT32_END, 0, "OD ESP"),)


def header(lba: int, alternate_lba: int, table_lba: int, table_crc: int,
           first_usable: int, last_usable: int) -> bytearray:
    block = bytearray(BLOCK)
    struct.pack_into(
        "<8sIIIIQQQQ16sQIII", block, 0,
        b"EFI PART", 0x10000, 92, 0, 0, lba, alternate_lba,
        first_usable, last_usable, DISK_ID.bytes_le, table_lba, ENTRY_COUNT, ENTRY_SIZE, table_crc,
    )
    struct.pack_into("<I", block, 16, zlib.crc32(block[:92]))
    return block


def gpt_regions(blocks: int, partitions: tuple, damage: str):
    last = blocks - 1
    backup_array = last - ARRAY_BLOCKS
    first_usable, last_usable = 2 + ARRAY_BLOCKS, backup_array - 1
    mbr = bytearray(BLOCK)
    struct.pack_into("<B3sB3sII", mbr, 446, 0, b"\x00\x02\x00", 0xEE, b"\xff" * 3, 1, last)
    mbr[510:] = b"\x55\xaa"
    yield 0, mbr

    entries = bytearray(ENTRY_COUNT * ENTRY_SIZE)
    for slot, kind, identifier, first, end, attributes, name in partitions:
        struct.pack_into(
            "<16s16sQQQ72s", entries, (slot - 1) * ENTRY_SIZE,
            kind.bytes_le, identifier.bytes_le, first, end, attributes, name.encode("utf-16le"),
        )
    table_crc = zlib.crc32(entries)
    primary = header(1, last, 2, table_crc, first_usable, last_usable)
    backup = header(last, 1, backup_array, table_crc, first_usable, last_usable)
    if damage in ("primary", "both"):
        primary[16] ^= 1
    if damage == "both":
        backup[16] ^= 1
    yield 1, primary
    yield 2, entries
    yield backup_array, entries
    yield last, backup


def image(damage: str) -> bytearray:
    disk = bytearray(BLOCK * BLOCKS)
    for lba, data in gpt_regions(BLOCKS, PARTITIONS, damage):
        at = lba * BLOCK
        disk[at:at + len(data)] = data
    for _, _, _, first, _, _, name in PARTITIONS:
        marker = name.encode("ascii") + b"\0"
        disk[first * BLOCK:first * BLOCK + len(marker)] = marker
    return disk


README = b"OD-UEFI FAT32 ESP README\r\n"
VOLUME_LABEL = b"OD ESP     "
RESERVED = 32
FATS = 2
ROOT = 2
EFI = 3
BOOT = 4
FILE = 5


def dir_entry(short: bytes, attributes: int, cluster: int = 0, size: int = 0) -> bytes:
    record = bytearray(32)
    record[:11] = short
    record[11] = attributes
    struct.pack_into("<H", record, 20, cluster >> 16)
    struct.pack_into("<HHI", record, 24, 0x5821, cluster & 0xffff, size)
    return record


def boot_sector(first_lba: int, blocks: int, fat_blocks: int) -> bytearray:
    boot = bytearray(BLOCK)
    boot[:11] = b"\xeb\x58\x90ODFAT32 "
    struct.pack_into("<HBHBHHBHHHII", boot, 11, BLOCK, 1, RESERVED, FATS,
                     0, 0, 0xf8, 0, 63, 255, first_lba, blocks)
    struct.pack_into("<IHHIHH", boot, 36, fat_blocks, 0, 0, ROOT, 1, 6)
    struct.pack_into("<BBBI", boot, 64, 0x80, 0, 0x29, 0x7377_1832)
    boot[71:82] = VOLUME_LABEL
    boot[82:90] = b"FAT32   "
    boot[510:] = b"\x55\xaa"
    return boot


def fsinfo(free_clusters: int, available_start: int) -> bytearray:
    info = bytearray(BLOCK)
    struct.pack_into("<I", info, 0, 0x41615252)
    struct.pack_into("<III", info, 484, 0x61417272, free_clusters, available_start)
    struct.pack_into("<I", info, 508, 0xaa550000)
    return info


def fat32_blocks(first_lba: int, count: int):
    """Yield logical sector writes. The caller provides an otherwise zeroed ESP."""
    fat_blocks = ((count + 2) * 4 + BLOCK - 1) // BLOCK
    data = RESERVED + FATS * fat_blocks
    data_clusters = count - data
    if data_clusters < 65525:
        raise ValueError("ESP is too small for the FAT32 cluster-count range")
    boot = boot_sector(first_lba, count, fat_blocks)
    info = fsinfo(data_clusters - 4, FILE + 1)
    yield first_lba, boot
    yield first_lba + 1, info
    yield first_lba + 6, boot
    yield first_lba + 7, info

    fat = struct.pack("<6I", 0x0ffffff8, 0xffffffff, *([0x0fffffff] * 4))
    yield first_lba + RESERVED, fat
    yield first_lba + RESERVED + fat_blocks, fat

    yield first_lba + data + ROOT - 2, (
        dir_entry(VOLUME_LABEL, 8) + dir_entry(b"EFI        ", 0x10, EFI)
    )
    yield first_lba + data + EFI - 2, (
        dir_entry(b".          ", 0x10, EFI)
        + dir_entry(b"..         ", 0x10)
        + dir_entry(b"BOOT       ", 0x10, BOOT)
    )
    yield first_lba + data + BOOT - 2, (
        dir_entry(b".          ", 0x10, BOOT)
        + dir_entry(b"..         ", 0x10, EFI)
        + dir_entry(b"README  TXT", 0x20, FILE, len(README))
    )
    yield first_lba + data + FILE - 2, README



def write_fat32(out: Path, damage: str) -> None:
    with out.open("wb") as disk:
        disk.truncate(FAT32_BLOCKS * BLOCK)
        for lba, data in gpt_regions(FAT32_BLOCKS, FAT32_PARTITIONS, damage):
            disk.seek(lba * BLOCK)
            disk.write(data)
        for lba, data in fat32_blocks(FAT32_START, FAT32_END - FAT32_START + 1):
            disk.seek(lba * BLOCK)
            disk.write(data)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--damage", choices=("none", "primary", "both"), default="none")
    parser.add_argument("--esp-fat32", action="store_true")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.esp_fat32:
        write_fat32(args.out, args.damage)
    else:
        args.out.write_bytes(image(args.damage))


if __name__ == "__main__":
    main()
