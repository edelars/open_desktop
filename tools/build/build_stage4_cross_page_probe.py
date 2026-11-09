#!/usr/bin/env python3
"""Build and validate the freestanding S4-33d cross-page read and fetch probe image."""

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
IMAGE_SIZE = 0xD000
NX = 1 << 63
# Supervisor entries with A and D clear, so that every accessed/dirty effect is observable.
ENTRIES = {
    0x1000: 0x2003,
    0x2000: 0x3003,
    0x3010: 0x4003,  # GVA 0x400000 code table
    0x3020: 0x5003,  # GVA 0x800000 data table
    0x4000: 0x7001,  # GVA 0x400000 first code frame, supervisor read/execute
    0x4008: 0x9001,  # GVA 0x401000 second code frame
    0x5000: 0xB003 | NX,  # GVA 0x800000 first data frame
    0x5008: 0xD003 | NX,  # GVA 0x801000 second data frame
}
CODE_HEAD = bytes.fromhex("668b")  # first two bytes of MOV AX,[RBX]
CODE_TAIL = bytes.fromhex("03" "488b03" "8b03" "668b03" "ee" "f4")
CODE_HEAD_GPA = 0x7FFE
CODE_TAIL_GPA = 0x9000
DATA0_GPA = 0xB000
DATA1_GPA = 0xD000
OPERAND_OFFSET = 0xFFD
CONTROL_OFFSET = 0x800
DECOY0 = bytes([0xA5]) * 8
DECOY1 = bytes([0x5A]) * 8
# (vaddr, paddr, filesz, flags) of the eight PT_LOAD segments.
SEGMENTS = [
    (0x1000, 0x1000, 0x5000, 6),
    (0x400FFE, CODE_HEAD_GPA, len(CODE_HEAD), 5),
    (0x401000, CODE_TAIL_GPA, len(CODE_TAIL) + 1, 5),  # the linker pads 11 bytes to 12; the pad is zero
    (0x800000, DATA0_GPA, 8, 6),
    (0x800800, DATA0_GPA + CONTROL_OFFSET, 8, 6),
    (0x800FFD, DATA0_GPA + OPERAND_OFFSET, 3, 6),
    (0x801000, DATA1_GPA, 5, 6),
    (0x801FF8, DATA1_GPA + 0xFF8, 8, 6),
]
LAYOUT = {
    "entry_gva": "0x400ffe",
    "code": {
        "head": {"gva": "0x400ffe", "gpa": "0x7ffe", "bytes": CODE_HEAD.hex(), "raw_offset": "0x6ffe"},
        "tail": {"gva": "0x401000", "gpa": "0x9000", "bytes": CODE_TAIL.hex(), "raw_offset": "0x8000"},
        "instructions": [
            {"gva": "0x400ffe", "bytes": "668b03", "mnemonic": "MOV AX,[RBX]", "fetch": "crosses 0x401000"},
            {"gva": "0x401001", "bytes": "488b03", "mnemonic": "MOV RAX,[RBX]"},
            {"gva": "0x401004", "bytes": "8b03", "mnemonic": "MOV EAX,[RBX]"},
            {"gva": "0x401006", "bytes": "668b03", "mnemonic": "MOV AX,[RBX]"},
            {"gva": "0x401009", "bytes": "ee", "mnemonic": "OUT DX,AL"},
            {"gva": "0x40100a", "bytes": "f4", "mnemonic": "HLT"},
        ],
    },
    "mappings": [
        {"gva": "0x400000", "gpa": "0x7000", "access": "supervisor-rx", "pte_gpa": "0x4000", "pte": "0x7001"},
        {"gva": "0x401000", "gpa": "0x9000", "access": "supervisor-rx", "pte_gpa": "0x4008", "pte": "0x9001"},
        {"gva": "0x800000", "gpa": "0xb000", "access": "supervisor-rw-nx", "pte_gpa": "0x5000", "pte": "0x800000000000b003"},
        {"gva": "0x801000", "gpa": "0xd000", "access": "supervisor-rw-nx", "pte_gpa": "0x5008", "pte": "0x800000000000d003"},
    ],
    "operand": {
        "rbx": "0x800ffd",
        "first_page": {"gva": "0x800ffd", "gpa": "0xbffd", "length": 3, "raw_offset": "0xaffd"},
        "second_page": {"gva": "0x801000", "gpa": "0xd000", "length": 5, "raw_offset": "0xc000"},
        "control_copy": {"rbx": "0x800800", "gpa": "0xb800", "length": 8, "raw_offset": "0xa800"},
        "decoys": {
            "first_frame": {"gpa": "0xb000", "byte": "0xa5", "length": 8},
            "second_frame": {"gpa": "0xdff8", "byte": "0x5a", "length": 8},
        },
    },
}


def expected_image(value: int) -> bytes:
    image = bytearray(IMAGE_SIZE)

    def put(gpa: int, data: bytes) -> None:
        image[gpa - IMAGE_BASE : gpa - IMAGE_BASE + len(data)] = data

    for address, entry in ENTRIES.items():
        put(address, entry.to_bytes(8, "little"))
    put(CODE_HEAD_GPA, CODE_HEAD)
    put(CODE_TAIL_GPA, CODE_TAIL)
    raw = value.to_bytes(8, "little")
    put(DATA0_GPA, DECOY0)
    put(DATA0_GPA + CONTROL_OFFSET, raw)
    put(DATA0_GPA + OPERAND_OFFSET, raw[:3])
    put(DATA1_GPA, raw[3:])
    put(DATA1_GPA + 0xFF8, DECOY1)
    return bytes(image)


def validate_image(elf: bytes, raw: bytes, value: int) -> None:
    if value >> 32 == 0:
        raise BuildError("value must have a nonzero upper dword so that qword and dword reads differ")
    entry, segments = parse_elf_load_segments(elf)
    if entry != 0x400FFE or len(segments) != len(SEGMENTS):
        raise BuildError("invalid entry or segment count")
    for segment, (vaddr, paddr, filesz, flags) in zip(segments, SEGMENTS, strict=True):
        if (segment["vaddr"], segment["paddr"], segment["filesz"], segment["flags"]) != (
            vaddr,
            paddr,
            filesz,
            flags,
        ):
            raise BuildError(f"unexpected segment {segment!r}")
        start = paddr - IMAGE_BASE
        if raw[start : start + filesz] != elf[segment["offset"] : segment["offset"] + filesz]:
            raise BuildError("raw segment does not match ELF")
    if len(raw) != IMAGE_SIZE:
        raise BuildError("image size is not the fixed profile size")
    if raw != expected_image(value):
        raise BuildError("image does not match the fixed cross-page profile")


def parse_u64(text: str) -> int:
    try:
        number = int(text, 0)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected an integer such as 0x8877665544332211") from exc
    if not 0 <= number < 1 << 64:
        raise argparse.ArgumentTypeError("value must fit in 64 bits")
    return number


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--value", type=parse_u64, default=0x8877665544332211)
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    if args.value >> 32 == 0:
        raise BuildError("value must have a nonzero upper dword so that qword and dword reads differ")
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/cross_page"
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
         f"-DPROBE_VALUE=0x{args.value:016x}", "-c",
         str(fixture / "entry.S"), "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T",
         str(fixture / "layout.ld"), "-o", str(out / "cross_page_probe.elf"),
         str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00",
         str(out / "cross_page_probe.elf"), str(out / "cross_page_probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "cross_page_probe.elf").read_bytes()
    unpadded = out / "cross_page_probe.unpadded.bin"
    raw = unpadded.read_bytes()
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"objcopy image exceeds fixed size: {len(raw):#x}")
    raw = raw + bytes(IMAGE_SIZE - len(raw))
    validate_image(elf, raw, args.value)
    (out / "cross_page_probe.bin").write_bytes(raw)
    unpadded.unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    value_bytes = args.value.to_bytes(8, "little")
    manifest = {
        "schema": "stage4-cross-page-probe-build-v1",
        "profile": "Stage4CrossPageProbeV1",
        "source_revision": revision,
        "value_u64": f"0x{args.value:016x}",
        "first_page_bytes": value_bytes[:3].hex(),
        "second_page_bytes": value_bytes[3:].hex(),
        "layout": LAYOUT,
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
            "cross_page_probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "cross_page_probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"raw sha256: {manifest['artifacts']['cross_page_probe.bin']['sha256']}; output: {out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
