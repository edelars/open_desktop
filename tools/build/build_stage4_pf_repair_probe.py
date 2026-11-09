#!/usr/bin/env python3
"""Build and validate the freestanding S4-33c CR0.WP #PF repair/retry probe image."""

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
ENTRIES = {
    0x1000: 0x2027,
    0x2000: 0x3027,
    0x3010: 0x4027,  # GVA 0x400000 code table
    0x3018: 0x5027,  # GVA 0x600000 IDT/GDT/stack table
    0x3020: 0x6007,  # GVA 0x800000 data table, A clear
    0x4008: 0x7027,  # GVA 0x401000 main code
    0x4010: 0x8027,  # GVA 0x402000 #PF handler
    0x4018: 0x6027,  # GVA 0x403000 RW alias of the data page table
    0x5000: 0x9027,  # GVA 0x600000 IDT
    0x5008: 0xA027,  # GVA 0x601000 GDT
    0x5808: 0xB027,  # GVA 0x701000 stack, lower page
    0x5810: 0xC027,  # GVA 0x702000 stack
    0x6000: 0xD001,  # GVA 0x800000 supervisor read-only, A clear
}
MAIN_CODE = bytes.fromhex("48890b488b03")  # MOV [RBX],RCX; MOV RAX,[RBX]
HANDLER_CODE = bytes.fromhex("488937" "0f013b" "488d642408" "48cf")
HANDLER_GVA = 0x402000
MAIN_CODE_GPA = 0x7000
HANDLER_CODE_GPA = 0x8000
IDT_GPA = 0x9000
GDT_GPA = 0xA000
DATA_GPA = 0xD000
GATE_OFFSET = 14 * 16
# Interrupt gate 14: target 0x402000, CS 0x08, IST 0, type 0x8e.
GATE = (
    (HANDLER_GVA & 0xFFFF).to_bytes(2, "little")
    + (0x08).to_bytes(2, "little")
    + bytes([0, 0x8E])
    + ((HANDLER_GVA >> 16) & 0xFFFF).to_bytes(2, "little")
    + (HANDLER_GVA >> 32).to_bytes(4, "little")
    + bytes(4)
)
GDT = bytes(8) + bytes.fromhex("ffff0000009baf00") + bytes.fromhex("ffff00000093cf00")
# (vaddr, paddr, filesz, flags) of the six PT_LOAD segments.
SEGMENTS = [
    (0x1000, 0x1000, 0x6000, 6),
    (0x401000, MAIN_CODE_GPA, len(MAIN_CODE), 5),
    (0x402000, HANDLER_CODE_GPA, 16, 5),  # the linker rounds 13 bytes up to 16; the tail is zero
    (0x600000, IDT_GPA, GATE_OFFSET + len(GATE), 6),
    (0x601000, GDT_GPA, len(GDT), 6),
    (0x800000, DATA_GPA, 24, 6),
]
LAYOUT = {
    "entry_gva": "0x401000",
    "main_code": {"gva": "0x401000", "gpa": "0x7000", "bytes": MAIN_CODE.hex(), "raw_offset": "0x6000"},
    "handler_code": {"gva": "0x402000", "gpa": "0x8000", "bytes": HANDLER_CODE.hex(), "raw_offset": "0x7000"},
    "idt": {"gva": "0x600000", "gpa": "0x9000", "gate_vector": 14, "gate": GATE.hex(), "raw_offset": "0x8000"},
    "gdt": {"gva": "0x601000", "gpa": "0xa000", "bytes": GDT.hex(), "raw_offset": "0x9000"},
    "stack": {"top_gva": "0x702027", "pages": [{"gva": "0x701000", "gpa": "0xb000"}, {"gva": "0x702000", "gpa": "0xc000"}]},
    "mappings": [
        {"gva": "0x800000", "gpa": "0xd000", "access": "supervisor-ro", "pte_gpa": "0x6000"},
        {"gva": "0x403000", "gpa": "0x6000", "access": "supervisor-rw-pte-alias"},
    ],
    "repair": {"pte_value": "0xd023", "alias_gva": "0x403000", "ro_pte": "0xd001"},
    "initial_qword_raw_offset": "0xc000",
    "canary_qword_raw_offset": "0xc008",
    "store_qword_raw_offset": "0xc010",
}


def expected_image(initial: int, store: int, canary: int) -> bytes:
    image = bytearray(IMAGE_SIZE)
    for address, value in ENTRIES.items():
        image[address - IMAGE_BASE : address - IMAGE_BASE + 8] = value.to_bytes(8, "little")
    for gpa, code in (
        (MAIN_CODE_GPA, MAIN_CODE),
        (HANDLER_CODE_GPA, HANDLER_CODE),
        (IDT_GPA + GATE_OFFSET, GATE),
        (GDT_GPA, GDT),
    ):
        image[gpa - IMAGE_BASE : gpa - IMAGE_BASE + len(code)] = code
    base = DATA_GPA - IMAGE_BASE
    for index, value in enumerate((initial, canary, store)):
        image[base + 8 * index : base + 8 * index + 8] = value.to_bytes(8, "little")
    return bytes(image)


def validate_image(elf: bytes, raw: bytes, initial: int, store: int, canary: int) -> None:
    if len({initial, store, canary}) != 3:
        raise BuildError("initial, store and canary qwords must differ")
    entry, segments = parse_elf_load_segments(elf)
    if entry != 0x401000 or len(segments) != len(SEGMENTS):
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
    if raw != expected_image(initial, store, canary):
        raise BuildError("image does not match the fixed #PF repair profile")


def parse_u64(value: str) -> int:
    try:
        number = int(value, 0)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected an integer such as 0x1122334455667788") from exc
    if not 0 <= number < 1 << 64:
        raise argparse.ArgumentTypeError("value must fit in 64 bits")
    return number


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--initial-value", type=parse_u64, default=0x1122334455667788)
    parser.add_argument("--store-value", type=parse_u64, default=0x8877665544332211)
    parser.add_argument("--canary-value", type=parse_u64, default=0x6A3591C7D248B0EF)
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/pf_repair"
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
         f"-DPROBE_INITIAL=0x{args.initial_value:016x}",
         f"-DPROBE_CANARY=0x{args.canary_value:016x}",
         f"-DPROBE_STORE_VALUE=0x{args.store_value:016x}", "-c",
         str(fixture / "entry.S"), "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T",
         str(fixture / "layout.ld"), "-o", str(out / "pf_repair_probe.elf"),
         str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00",
         str(out / "pf_repair_probe.elf"), str(out / "pf_repair_probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "pf_repair_probe.elf").read_bytes()
    unpadded = out / "pf_repair_probe.unpadded.bin"
    raw = unpadded.read_bytes()
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"objcopy image exceeds fixed size: {len(raw):#x}")
    raw = raw + bytes(IMAGE_SIZE - len(raw))
    validate_image(elf, raw, args.initial_value, args.store_value, args.canary_value)
    (out / "pf_repair_probe.bin").write_bytes(raw)
    unpadded.unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    manifest = {
        "schema": "stage4-pf-repair-probe-build-v1",
        "profile": "Stage4PfRepairProbeV1",
        "source_revision": revision,
        "initial_u64": f"0x{args.initial_value:016x}",
        "canary_u64": f"0x{args.canary_value:016x}",
        "store_value_u64": f"0x{args.store_value:016x}",
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
            "pf_repair_probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "pf_repair_probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"raw sha256: {manifest['artifacts']['pf_repair_probe.bin']['sha256']}; output: {out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
