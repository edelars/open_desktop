#!/usr/bin/env python3
"""Generate GPT, FAT32 ESP and ISO-9660 fixtures for UEFI checks."""

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


def fat32_blocks(first_lba: int, count: int, boot_files: tuple[tuple[bytes, bytes], ...] = ()):
    """Yield logical sector writes. The caller provides an otherwise zeroed ESP."""
    fat_blocks = ((count + 2) * 4 + BLOCK - 1) // BLOCK
    data = RESERVED + FATS * fat_blocks
    data_clusters = count - data
    if data_clusters < 65525:
        raise ValueError("ESP is too small for the FAT32 cluster-count range")
    fat_entries = [0x0ffffff8, 0xffffffff, *([0x0fffffff] * 4)]
    boot_dir = (
        dir_entry(b".          ", 0x10, BOOT)
        + dir_entry(b"..         ", 0x10, EFI)
        + dir_entry(b"README  TXT", 0x20, FILE, len(README))
    )
    allocated = []
    for name, content in boot_files:
        start = len(fat_entries)
        clusters = (len(content) + BLOCK - 1) // BLOCK
        if clusters == 0 or start + clusters > data_clusters + 2:
            raise ValueError("boot file is empty or does not fit in the ESP")
        fat_entries.extend(range(start + 1, start + clusters))
        fat_entries.append(0x0fffffff)
        boot_dir += dir_entry(name, 0x20, start, len(content))
        allocated.append((start, content))
    free_clusters = data_clusters + 2 - len(fat_entries)
    boot = boot_sector(first_lba, count, fat_blocks)
    info = fsinfo(free_clusters, len(fat_entries) if free_clusters else 0xffffffff)
    yield first_lba, boot
    yield first_lba + 1, info
    yield first_lba + 6, boot
    yield first_lba + 7, info

    fat = struct.pack(f"<{len(fat_entries)}I", *fat_entries)
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
    yield first_lba + data + BOOT - 2, boot_dir
    yield first_lba + data + FILE - 2, README
    for cluster, content in allocated:
        yield first_lba + data + cluster - 2, content



def write_fat32(out: Path, damage: str, boot_files: tuple[tuple[bytes, bytes], ...] = ()) -> None:
    with out.open("wb") as disk:
        disk.truncate(FAT32_BLOCKS * BLOCK)
        for lba, data in gpt_regions(FAT32_BLOCKS, FAT32_PARTITIONS, damage):
            disk.seek(lba * BLOCK)
            disk.write(data)
        for lba, data in fat32_blocks(FAT32_START, FAT32_END - FAT32_START + 1, boot_files):
            disk.seek(lba * BLOCK)
            disk.write(data)


def write_iso9660(out: Path, app: bytes) -> None:
    """ECMA-119 (5th ed.) Level 1 directory tree with an EFI app and README."""
    sector = 2048
    root, efi, boot = 20, 21, 22
    app_lba = 23
    readme_lba = app_lba + (len(app) + sector - 1) // sector
    blocks = readme_lba + 1
    disk = bytearray(blocks * sector)

    def both16(number):
        return struct.pack("<H", number) + struct.pack(">H", number)

    def both32(number):
        return struct.pack("<I", number) + struct.pack(">I", number)

    def record(name, lba, size, directory):
        length = 33 + len(name) + (len(name) % 2 == 0)
        row = bytearray(length)
        row[0] = length
        row[2:10] = both32(lba)
        row[10:18] = both32(size)
        row[18:25] = bytes([126, 11, 20, 0, 0, 0, 0])
        row[25] = 2 if directory else 0
        row[28:32] = both16(1)
        row[32] = len(name)
        row[33:33 + len(name)] = name
        return bytes(row)

    def directory(lba, parent, children):
        return record(b"\0", lba, sector, True) + record(b"\1", parent, sector, True) + children

    def path_table(order):
        rows = []
        for name, lba, parent in ((b"\0", root, 1), (b"EFI", efi, 1), (b"BOOT", boot, 2)):
            rows.append(struct.pack(order + "BBIH", len(name), 0, lba, parent) + name + bytes(len(name) % 2))
        return b"".join(rows)

    path_l, path_m = path_table("<"), path_table(">")
    pvd = bytearray(sector)
    pvd[:7] = b"\x01CD001\x01"
    pvd[8:40] = b"OPEN DESKTOP".ljust(32)
    pvd[40:72] = b"OD_INSTALL_FIXTURE".ljust(32)
    pvd[80:88] = both32(blocks)
    pvd[120:132] = both16(1) + both16(1) + both16(sector)
    pvd[132:140] = both32(len(path_l))
    struct.pack_into("<I", pvd, 140, 18)
    struct.pack_into(">I", pvd, 148, 19)
    pvd[156:190] = record(b"\0", root, sector, True)
    pvd[190:813] = b" " * (813 - 190)
    pvd[813:847] = (b"2026112000000000\0") * 2
    pvd[847:881] = (b"0000000000000000\0") * 2
    pvd[881] = 1
    for lba, contents in (
        (16, pvd), (17, b"\xffCD001\x01"), (18, path_l), (19, path_m),
        (root, directory(root, root, record(b"EFI", efi, sector, True)
                         + record(b"README.TXT;1", readme_lba, len(README), False))),
        (efi, directory(efi, root, record(b"BOOT", boot, sector, True))),
        (boot, directory(boot, efi, record(b"BOOTX64.EFI;1", app_lba, len(app), False))),
        (app_lba, app), (readme_lba, README),
    ):
        start = lba * sector
        disk[start:start + len(contents)] = contents
    out.write_bytes(disk)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--damage", choices=("none", "primary", "both"), default="none")
    form = parser.add_mutually_exclusive_group()
    form.add_argument("--esp-fat32", action="store_true")
    form.add_argument("--iso9660", action="store_true")
    parser.add_argument("--bootx64", type=Path, help="place this EFI image at \\EFI\\BOOT\\BOOTX64.EFI")
    parser.add_argument("--boot-probe", type=Path, help="place this EFI image at \\EFI\\BOOT\\CHECK.EFI")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.iso9660:
        if args.bootx64 is None or args.boot_probe or args.damage != "none":
            parser.error("--iso9660 requires --bootx64 and a clean single application")
        write_iso9660(args.out, args.bootx64.read_bytes())
    elif args.esp_fat32:
        boot_files = tuple(
            (name, path.read_bytes())
            for name, path in ((b"BOOTX64 EFI", args.bootx64), (b"CHECK   EFI", args.boot_probe))
            if path is not None
        )
        write_fat32(args.out, args.damage, boot_files)
    elif args.bootx64 or args.boot_probe:
        parser.error("EFI files require --esp-fat32 or --iso9660")
    else:
        args.out.write_bytes(image(args.damage))


if __name__ == "__main__":
    main()
