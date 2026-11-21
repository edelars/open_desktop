#!/usr/bin/env python3
"""Generate GPT, FAT32 ESP, ISO-9660 and UDF 1.02 fixtures for UEFI checks."""

from __future__ import annotations

import argparse
from binascii import crc_hqx
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


UDF_LONG_NAME = "Quick fixture release.txt"
UDF_LONG_DATA = bytes(range(256)) * 9


def udf_tag(data: bytearray, identifier: int, location: int) -> bytes:
    struct.pack_into("<HHBBHHHI", data, 0, identifier, 2, 0, 0, 1,
                     crc_hqx(data[16:], 0), len(data) - 16, location)
    data[4] = sum(data[:16]) & 0xff
    return bytes(data)


def write_udf102(out: Path, app: bytes) -> None:
    """ECMA-167 Part 3/4 and ECMA TR/112-7, read-only NSR02 with a Type-1 map."""
    sector = 2048
    blocks = 1024
    partition = 300
    partition_block_count = blocks - 257 - partition
    app_block = 16
    note_block = app_block + (len(app) + sector - 1) // sector
    if note_block + 7 >= partition_block_count:
        raise ValueError("EFI fixture exceeds the UDF partition")
    disk = bytearray(blocks * sector)
    charset = b"\0OSTA Compressed Unicode".ljust(64, b"\0")
    domain = b"\0*OSTA UDF Compliant".ljust(24, b"\0") + b"\x02\x01" + bytes(6)

    def put(lba, contents):
        start = lba * sector
        disk[start:start + len(contents)] = contents

    def dstring(text, length):
        value = b"\x08" + text
        return value.ljust(length - 1, b"\0") + bytes([len(value)])

    def long_address(lbn, length=sector):
        return struct.pack("<IIH6x", length, lbn, 0)

    def descriptor(lba, identifier, data):
        put(lba, udf_tag(data, identifier, lba))

    for lba, identifier in ((16, b"BEA01"), (17, b"NSR02"), (18, b"TEA01")):
        put(lba, b"\0" + identifier + b"\x01")

    main_sequence, reserve_sequence = 257, 273
    for anchor_lba in (256, blocks - 257, blocks - 1):
        anchor = bytearray(512)
        struct.pack_into("<IIII", anchor, 16, 16 * sector, main_sequence,
                         16 * sector, reserve_sequence)
        descriptor(anchor_lba, 2, anchor)

    for sequence in (main_sequence, reserve_sequence):
        primary = bytearray(512)
        struct.pack_into("<I", primary, 16, 1)
        primary[24:56] = dstring(b"OD UDF", 32)
        struct.pack_into("<HHHHII", primary, 56, 1, 1, 2, 3, 1, 1)
        primary[72:200] = dstring(b"00000000OD UDF", 128)
        primary[200:264] = primary[264:328] = charset
        descriptor(sequence, 1, primary)

        part = bytearray(512)
        struct.pack_into("<IHH", part, 16, 2, 1, 7)
        part[24:31] = b"\0+NSR02"
        struct.pack_into("<III", part, 184, 1, partition, partition_block_count)
        descriptor(sequence + 1, 5, part)

        logical = bytearray(446)
        struct.pack_into("<I", logical, 16, 3)
        logical[20:84] = charset
        logical[84:212] = dstring(b"OD UDF", 128)
        struct.pack_into("<I", logical, 212, sector)
        logical[216:248] = domain
        logical[248:264] = long_address(0, 2 * sector)
        struct.pack_into("<II", logical, 264, 6, 1)
        struct.pack_into("<BBHH", logical, 440, 1, 6, 1, 7)
        descriptor(sequence + 2, 6, logical)
        descriptor(sequence + 3, 8, bytearray(512))

    file_set = bytearray(512)
    struct.pack_into("<HHII", file_set, 28, 3, 3, 1, 1)
    file_set[48:112] = file_set[240:304] = charset
    file_set[112:240] = dstring(b"OD UDF", 128)
    file_set[304:336] = dstring(b"OD UDF", 32)
    file_set[400:416] = long_address(2)
    file_set[416:448] = domain
    put(partition, udf_tag(file_set, 256, 0))
    put(partition + 1, udf_tag(bytearray(512), 8, 1))

    def file_entry(lbn, parent, directory, ranges):
        data = bytearray(176 + 8 * len(ranges))
        struct.pack_into("<H2xH", data, 20, 4, 1)
        data[27] = 4 if directory else 5
        struct.pack_into("<I", data, 28, parent)
        struct.pack_into("<H", data, 48, 1)
        struct.pack_into("<QQ", data, 56, sum(len(blob) for _, blob in ranges),
                         sum((len(blob) + sector - 1) // sector for _, blob in ranges))
        struct.pack_into("<II", data, 168, 0, 8 * len(ranges))
        for index, (at, blob) in enumerate(ranges):
            struct.pack_into("<II", data, 176 + 8 * index, len(blob), at)
            put(partition + at, blob)
        put(partition + lbn, udf_tag(data, 261, lbn))

    def directory(lbn, parent, block, members):
        contents = bytearray()
        for name, child, flags in (("", parent, 0x0a), *members):
            value = b"\x10" + name.encode("utf-16-be") if name else b""
            data = bytearray((38 + len(value) + 3) & ~3)
            struct.pack_into("<HBB", data, 16, 1, flags, len(value))
            data[20:36] = long_address(child)
            data[38:38 + len(value)] = value
            contents.extend(udf_tag(data, 257, block + len(contents) // sector))
        file_entry(lbn, parent, True, [(block, contents)])

    directory(2, 2, 10, (("EFI", 3, 2), ("README.TXT", 6, 0),
                          (UDF_LONG_NAME, 7, 0), ("QUICKF~1.TXT", 8, 0), ("Empty note.txt", 9, 0)))
    directory(3, 2, 11, (("BOOT", 4, 2),))
    directory(4, 3, 12, (("BOOTX64.EFI", 5, 0),))
    file_entry(5, 4, False, [(app_block, app)])
    file_entry(6, 2, False, [(note_block, README)])
    file_entry(7, 2, False, [(note_block + 2, UDF_LONG_DATA[:sector]),
                             (note_block + 4, UDF_LONG_DATA[sector:])])
    file_entry(8, 2, False, [(note_block + 6, b"Alias collision check")])
    file_entry(9, 2, False, [])
    out.write_bytes(disk)


def extra_boot_file(spec: str, parser: argparse.ArgumentParser) -> tuple[bytes, bytes]:
    """Parse NAME.EXT=PATH into an 11-byte short directory name and the file contents."""
    name, separator, path = spec.partition("=")
    stem, dot, extension = name.partition(".")
    valid = (separator and path and dot and 1 <= len(stem) <= 8 and 1 <= len(extension) <= 3
             and name.isascii() and name == name.upper() and name.count(".") == 1
             and all(c.isalnum() or c in "_-" for c in stem + extension))
    if not valid:
        parser.error(f"--boot-file needs an upper-case 8.3 NAME.EXT=PATH, got {spec!r}")
    return (stem.ljust(8) + extension.ljust(3)).encode("ascii"), Path(path).read_bytes()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--damage", choices=("none", "primary", "both"), default="none")
    form = parser.add_mutually_exclusive_group()
    form.add_argument("--esp-fat32", action="store_true")
    form.add_argument("--iso9660", action="store_true")
    form.add_argument("--udf102", action="store_true")
    parser.add_argument("--bootx64", type=Path, help="place this EFI image at \\EFI\\BOOT\\BOOTX64.EFI")
    parser.add_argument("--boot-probe", type=Path, help="place this EFI image at \\EFI\\BOOT\\CHECK.EFI")
    parser.add_argument("--boot-file", action="append", default=[], metavar="NAME.EXT=PATH",
                        help="place PATH at \\EFI\\BOOT\\NAME.EXT (8.3 name, upper case, repeatable)")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.iso9660 or args.udf102:
        if args.bootx64 is None or args.boot_probe or args.damage != "none":
            parser.error("optical media require --bootx64 and a clean single application")
        write_optical = write_iso9660 if args.iso9660 else write_udf102
        write_optical(args.out, args.bootx64.read_bytes())
    elif args.esp_fat32:
        boot_files = tuple(
            (name, path.read_bytes())
            for name, path in ((b"BOOTX64 EFI", args.bootx64), (b"CHECK   EFI", args.boot_probe))
            if path is not None
        ) + tuple(extra_boot_file(spec, parser) for spec in args.boot_file)
        write_fat32(args.out, args.damage, boot_files)
    elif args.bootx64 or args.boot_probe or args.boot_file:
        parser.error("EFI files require --esp-fat32, --iso9660 or --udf102")
    else:
        args.out.write_bytes(image(args.damage))


if __name__ == "__main__":
    main()
