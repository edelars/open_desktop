#!/usr/bin/env python3
"""Build and validate the freestanding S4-33e huge-page probe image (2 MiB positive, 1 GiB negative)."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_stage4_flat_probe import (  # noqa: E402
    BuildError,
    output_directory,
    parse_elf_load_segments,
    resolve_executable,
    run,
    rust_toolchain,
    sha256,
)

IMAGE_BASE = 0x1000
IMAGE_SIZE = 0x400000
NX = 1 << 63
DEFAULT_START = 0x1122334455667788
DEFAULT_MIDDLE = 0x99AABBCCDDEEFF01
DEFAULT_END = 0x0F1E2D3C4B5A6978
SENTINEL = 0xFEEDFACE0BADF00D
DECOY = 0xA5A5A5A5A5A5A5A5
STORE_DECOY = 0x5A5A5A5A5A5A5A5A
# Handoff values the loader supplies; a data value equal to one of them would hide a defect.
INITIAL_RAX = 0xDEADBEEFCAFEBABE
STORE_VALUE = 0xC0DEC0DE123456AB
# Entries come from the SDM formats (Vol. 3A 4.5, formats of the PML4E, PDPTE, PDE that maps a 2-MByte page, and PTE): P=bit0, R/W=bit1,
# PS=bit7, address bits 51:12 (4 KiB) / 51:21 (2 MiB) / 51:30 (1 GiB), XD=bit63. A and D are clear.
ENTRIES = {
    0x1000: 0x2003,  # PML4E[0] -> PDPT 0
    0x1008: 0x5003,  # PML4E[1] -> PDPT 1 (1 GiB negative route)
    0x2000: 0x3003,  # PDPT0[0] -> PD
    0x3010: 0x4003,  # PD[2], GVA 0x400000 -> code page table
    0x3020: 0x200083 | NX,  # PD[4], GVA 0x800000: 2 MiB leaf (PS=1) at GPA 0x200000
    0x3028: 0x400083 | NX,  # PD[5], GVA 0xa00000: 2 MiB leaf at GPA 0x400000
    0x4000: 0x7001,  # GVA 0x400000 -> code frame 0x7000, supervisor read/execute
    0x5000: 0x40000083,  # PDPT1[0], GVA 0x8000000000: PS=1 (a 1 GiB leaf, reserved in the profile)
}
CODE = bytes.fromhex("488b03" "488b03" "488b03" "48890b" "488b03" "ee" "f4")
CODE_GPA = 0x7000
DECOY_GPA = 0x8000
START_GPA = 0x200000
MIDDLE_GPA = 0x300000
STORE_GPA = 0x3FFF00
END_GPA = 0x3FFFF8
SENTINEL_GPA = 0x400000
# (vaddr, paddr, filesz, flags) of the eight PT_LOAD segments.
SEGMENTS = [
    (0x1000, 0x1000, 0x6000, 6),
    (0x400000, CODE_GPA, len(CODE) + 3, 5),  # the linker pads 17 bytes to 20; the pad is zero
    (0x600000, DECOY_GPA, 8, 6),
    (0x800000, START_GPA, 8, 6),
    (0x900000, MIDDLE_GPA, 8, 6),
    (0x9FFF00, STORE_GPA, 8, 6),
    (0x9FFFF8, END_GPA, 8, 6),
    (0xA00000, SENTINEL_GPA, 8, 6),
]


def layout(start: int, middle: int, end: int) -> dict:
    return {
        "entry_gva": "0x400000",
        "code": {
            "gva": "0x400000",
            "gpa": "0x7000",
            "bytes": CODE.hex(),
            "raw_offset": "0x6000",
            "instructions": [
                {"gva": "0x400000", "bytes": "488b03", "mnemonic": "MOV RAX,[RBX]", "operand": "offset 0"},
                {"gva": "0x400003", "bytes": "488b03", "mnemonic": "MOV RAX,[RBX]", "operand": "offset 0x100000"},
                {"gva": "0x400006", "bytes": "488b03", "mnemonic": "MOV RAX,[RBX]", "operand": "offset 0x1ffff8"},
                {"gva": "0x400009", "bytes": "48890b", "mnemonic": "MOV [RBX],RCX", "operand": "offset 0x1fff00"},
                {"gva": "0x40000c", "bytes": "488b03", "mnemonic": "MOV RAX,[RBX]", "operand": "offset 0x1fff00"},
                {"gva": "0x40000f", "bytes": "ee", "mnemonic": "OUT DX,AL"},
                {"gva": "0x400010", "bytes": "f4", "mnemonic": "HLT"},
            ],
        },
        "mappings": [
            {"gva": "0x400000", "size": "4 KiB", "gpa": "0x7000", "access": "supervisor-rx",
             "entry_gpa": "0x4000", "entry": "0x7001"},
            {"gva": "0x800000", "size": "2 MiB", "gpa": "0x200000", "access": "supervisor-rw-nx",
             "entry_gpa": "0x3020", "entry": "0x8000000000200083", "ps": 1,
             "entry_after_reads": "0x80000000002000a3", "entry_after_store": "0x80000000002000e3"},
            {"gva": "0xa00000", "size": "2 MiB", "gpa": "0x400000", "access": "supervisor-rw-nx",
             "entry_gpa": "0x3028", "entry": "0x8000000000400083", "ps": 1},
            {"gva": "0x8000000000", "size": "1 GiB (not declared)", "entry_gpa": "0x5000",
             "entry": "0x40000083", "ps": 1, "expected": "#PF P|RSVD, PFEC 0x09 at CPL0 read"},
        ],
        "offsets": {
            "start": {"gva": "0x800000", "gpa": "0x200000", "offset": "0x0", "value": f"0x{start:016x}"},
            "middle": {"gva": "0x900000", "gpa": "0x300000", "offset": "0x100000", "value": f"0x{middle:016x}"},
            "end": {"gva": "0x9ffff8", "gpa": "0x3ffff8", "offset": "0x1ffff8", "value": f"0x{end:016x}"},
            "store": {"gva": "0x9fff00", "gpa": "0x3fff00", "offset": "0x1fff00",
                      "prefill": f"0x{STORE_DECOY:016x}", "stored": f"0x{STORE_VALUE:016x}"},
            "past_page": {"gva": "0xa00000", "gpa": "0x400000", "sentinel": f"0x{SENTINEL:016x}"},
            "decoy_4k_frame": {"gpa": "0x8000", "value": f"0x{DECOY:016x}"},
        },
        "negative_1gib": {"rbx": "0x8000000000", "pml4e": "0x5003", "pdpte": "0x40000083",
                           "pfec_cpl0_read": "0x09", "cr2": "0x8000000000"},
    }


def expected_image(start: int, middle: int, end: int) -> bytes:
    image = bytearray(IMAGE_SIZE)

    def put(gpa: int, data: bytes) -> None:
        image[gpa - IMAGE_BASE : gpa - IMAGE_BASE + len(data)] = data

    for address, entry in ENTRIES.items():
        put(address, entry.to_bytes(8, "little"))
    put(CODE_GPA, CODE)
    put(DECOY_GPA, DECOY.to_bytes(8, "little"))
    put(START_GPA, start.to_bytes(8, "little"))
    put(MIDDLE_GPA, middle.to_bytes(8, "little"))
    put(STORE_GPA, STORE_DECOY.to_bytes(8, "little"))
    put(END_GPA, end.to_bytes(8, "little"))
    put(SENTINEL_GPA, SENTINEL.to_bytes(8, "little"))
    return bytes(image)


def check_values(start: int, middle: int, end: int) -> None:
    values = [start, middle, end]
    if len(set(values)) != 3:
        raise BuildError("start, middle and end values must be pairwise different")
    reserved = {DECOY, STORE_DECOY, SENTINEL, INITIAL_RAX, STORE_VALUE}
    if reserved & set(values):
        raise BuildError("values must differ from the decoy, sentinel, handoff and store constants")
    if len({v & 0xFF for v in [end, STORE_VALUE]}) != 2:
        raise BuildError("the end value must differ from the stored value in its low byte (UART byte)")


def validate_image(elf: bytes, raw: bytes, start: int, middle: int, end: int) -> None:
    check_values(start, middle, end)
    entry, segments = parse_elf_load_segments(elf)
    if entry != 0x400000 or len(segments) != len(SEGMENTS):
        raise BuildError("invalid entry or segment count")
    for segment, (vaddr, paddr, filesz, flags) in zip(segments, SEGMENTS, strict=True):
        if (segment["vaddr"], segment["paddr"], segment["filesz"], segment["flags"]) != (
            vaddr,
            paddr,
            filesz,
            flags,
        ):
            raise BuildError(f"unexpected segment {segment!r}")
        offset = paddr - IMAGE_BASE
        if raw[offset : offset + filesz] != elf[segment["offset"] : segment["offset"] + filesz]:
            raise BuildError("raw segment does not match ELF")
    if len(raw) != IMAGE_SIZE:
        raise BuildError("image size is not the fixed profile size")
    if raw != expected_image(start, middle, end):
        raise BuildError("image does not match the fixed huge-page profile")


def parse_u64(text: str) -> int:
    try:
        number = int(text, 0)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected an integer such as 0x1122334455667788") from exc
    if not 0 <= number < 1 << 64:
        raise argparse.ArgumentTypeError("value must fit in 64 bits")
    return number


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--start-value", type=parse_u64, default=DEFAULT_START)
    parser.add_argument("--middle-value", type=parse_u64, default=DEFAULT_MIDDLE)
    parser.add_argument("--end-value", type=parse_u64, default=DEFAULT_END)
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    check_values(args.start_value, args.middle_value, args.end_value)
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/huge_pages"
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp",
         "-ffreestanding", "-fno-pic", "-fno-pie", "-mno-red-zone",
         f"-DSTART_VALUE=0x{args.start_value:016x}",
         f"-DMIDDLE_VALUE=0x{args.middle_value:016x}",
         f"-DEND_VALUE=0x{args.end_value:016x}", "-c",
         str(fixture / "entry.S"), "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T",
         str(fixture / "layout.ld"), "-o", str(out / "huge_pages_probe.elf"),
         str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00",
         str(out / "huge_pages_probe.elf"), str(out / "huge_pages_probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "huge_pages_probe.elf").read_bytes()
    unpadded = out / "huge_pages_probe.unpadded.bin"
    raw = unpadded.read_bytes()
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"objcopy image exceeds fixed size: {len(raw):#x}")
    raw = raw + bytes(IMAGE_SIZE - len(raw))
    validate_image(elf, raw, args.start_value, args.middle_value, args.end_value)
    (out / "huge_pages_probe.bin").write_bytes(raw)
    unpadded.unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    manifest = {
        "schema": "stage4-huge-pages-probe-build-v1",
        "profile": "Stage4HugePagesProbeV1",
        "source_revision": revision,
        "start_value_u64": f"0x{args.start_value:016x}",
        "middle_value_u64": f"0x{args.middle_value:016x}",
        "end_value_u64": f"0x{args.end_value:016x}",
        "layout": layout(args.start_value, args.middle_value, args.end_value),
        "source": {name: sha256((fixture / name).read_bytes()) for name in ("entry.S", "layout.ld")},
        "target": "x86_64-unknown-none-elf",
        "toolchain": {
            "clang": clang_version,
            "linker": linker_version,
            "objcopy": objcopy_version.splitlines()[0] if objcopy_version else "",
            "rustc": rustc_version.splitlines()[0],
            "host": host,
        },
        "commands": commands,
        "artifacts": {
            "huge_pages_probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "huge_pages_probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"raw sha256: {manifest['artifacts']['huge_pages_probe.bin']['sha256']}; output: {out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
