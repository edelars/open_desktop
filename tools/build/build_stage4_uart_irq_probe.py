#!/usr/bin/env python3
"""Build and validate the freestanding Stage4UartIrqProbeV1 kernel image (S4-24e).

The image is a flat identity-mapped Long64/CPL0 guest: GDT, a one-gate IDT,
an idle/handler code page and the IDTR pseudo-descriptor (also the initial
stack top). The raw image is loaded at GPA 0x1000 and is exactly 0x6000 bytes.
Variants only exist to prove that broken guests fail the acceptance check.
"""

from __future__ import annotations

import argparse
import json
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_stage4_flat_probe import (  # noqa: E402
    BuildError,
    elf_symbol,
    output_directory,
    parse_elf_load_segments,
    resolve_executable,
    run,
    rust_toolchain,
    sha256,
)

IMAGE_BASE = 0x1000
IMAGE_SIZE = 0x6000
GDT_GPA = 0x2000
IDT_GPA = 0x3000
CODE_GPA = 0x4000
IDTR_GPA = 0x6FF0
VECTOR = 0x51
GDT = struct.pack("<QQQ", 0, 0x00AF9B000000FFFF, 0x00CF93000000FFFF)


def _out8(port: int, value: int) -> bytes:
    return b"\xba" + struct.pack("<I", port) + bytes([0xB0, value, 0xEE])


def _unmask() -> bytes:
    return (
        bytes.fromhex("bf0000c0fe")
        + bytes.fromhex("b818000000")
        + bytes.fromhex("8907")
        + bytes.fromhex("b851000000")
        + bytes.fromhex("894710")
    )


def expected_entry(ier: int, late_unmask: bool) -> bytes:
    code = bytes.fromhex("0f015c2400")  # LIDT [RSP+0]
    code += bytes.fromhex("bf0000e0fe") + bytes.fromhex("b8ff010000") + bytes.fromhex("8987f0000000")
    if not late_unmask:
        code += _unmask()
    for port, value in ((0x3FB, 0x83), (0x3F8, 0x01), (0x3F9, 0x00), (0x3FB, 0x03), (0x3FC, 0x08), (0x3F9, ier)):
        code += _out8(port, value)
    if late_unmask:
        code += _unmask()
    return code + bytes.fromhex("fb") + bytes.fromhex("f4ebfd")  # STI; HLT; JMP back to HLT


def expected_handler(omit_eoi: bool, omit_rbr_read: bool) -> bytes:
    code = bytes.fromhex("bafa030000ec") + bytes.fromhex("baf8030000")
    if not omit_rbr_read:
        code += bytes.fromhex("ec")
    code += bytes.fromhex("ee")
    if not omit_eoi:
        code += bytes.fromhex("bf0000e0fe") + bytes.fromhex("b800000000") + bytes.fromhex("8987b0000000")
    return code + bytes.fromhex("48cf")


def validate_image(elf: bytes, raw: bytes, args: argparse.Namespace) -> dict:
    entry, segments = parse_elf_load_segments(elf)
    code_start, _ = elf_symbol(elf, "_start")
    handler, _ = elf_symbol(elf, "stage4_irq_handler")
    code_end, _ = elf_symbol(elf, "stage4_code_end")
    if entry != CODE_GPA or code_start != CODE_GPA:
        raise BuildError(f"entry is 0x{entry:x}, expected 0x{CODE_GPA:x}")
    entry_code = expected_entry(args.ier, args.late_unmask)
    handler_code = expected_handler(args.omit_eoi, args.omit_rbr_read)
    if handler != CODE_GPA + len(entry_code) or code_end != handler + len(handler_code):
        raise BuildError(
            f"unexpected code layout: handler=0x{handler:x} end=0x{code_end:x} "
            f"entry_len=0x{len(entry_code):x} handler_len=0x{len(handler_code):x}"
        )
    expected = [
        (GDT_GPA, GDT_GPA, len(GDT), 6),
        (IDT_GPA, IDT_GPA, 0x1000, 6),
        # The linker rounds the code output section up to a multiple of four bytes.
        (CODE_GPA, CODE_GPA, (len(entry_code) + len(handler_code) + 3) & ~3, 5),
        (IDTR_GPA, IDTR_GPA, 10, 6),
    ]
    if len(segments) != len(expected):
        raise BuildError(f"expected {len(expected)} PT_LOAD segments, found {len(segments)}")
    for segment, (vaddr, paddr, size, flags) in zip(segments, expected, strict=True):
        actual = (segment["vaddr"], segment["paddr"], segment["filesz"], segment["flags"])
        if actual != (vaddr, paddr, size, flags) or segment["memsz"] != size:
            raise BuildError(f"unexpected PT_LOAD {actual}, expected {(vaddr, paddr, size, flags)}")
    if len(raw) != IMAGE_SIZE:
        raise BuildError(f"raw image is 0x{len(raw):x} bytes, expected 0x{IMAGE_SIZE:x}")
    for segment in segments:
        start = segment["paddr"] - IMAGE_BASE
        if raw[start : start + segment["filesz"]] != elf[segment["offset"] : segment["offset"] + segment["filesz"]]:
            raise BuildError(f"raw bytes differ from ELF at GPA 0x{segment['paddr']:x}")

    def at(gpa: int, length: int) -> bytes:
        return raw[gpa - IMAGE_BASE : gpa - IMAGE_BASE + length]

    if at(GDT_GPA, len(GDT)) != GDT:
        raise BuildError("GDT descriptors do not match the fixed profile")
    idt = at(IDT_GPA, 0x1000)
    gate = idt[VECTOR * 16 : VECTOR * 16 + 16]
    offset_low, selector, ist, type_attr, offset_mid, offset_high, reserved = struct.unpack("<HHBBHII", gate)
    if (offset_low, selector, ist, type_attr, offset_mid, offset_high, reserved) != (handler, 8, 0, 0x8E, 0, 0, 0):
        raise BuildError(f"IDT gate for vector 0x{VECTOR:x} is wrong: {gate.hex()}")
    if idt[: VECTOR * 16] != bytes(VECTOR * 16) or idt[(VECTOR + 1) * 16 :] != bytes(0x1000 - (VECTOR + 1) * 16):
        raise BuildError("IDT must contain no gate other than the INTIN4 vector")
    if at(CODE_GPA, len(entry_code)) != entry_code:
        raise BuildError("entry code does not match the selected variant")
    if at(handler, len(handler_code)) != handler_code:
        raise BuildError("handler code does not match the selected variant")
    if at(IDTR_GPA, 10) != struct.pack("<HQ", 0xFFF, IDT_GPA):
        raise BuildError("IDTR pseudo-descriptor is wrong")
    covered = [
        (GDT_GPA, len(GDT)),
        (IDT_GPA, 0x1000),
        (CODE_GPA, len(entry_code) + len(handler_code)),
        (IDTR_GPA, 10),
    ]
    for gpa in range(IMAGE_BASE, IMAGE_BASE + IMAGE_SIZE):
        if raw[gpa - IMAGE_BASE] and not any(start <= gpa < start + size for start, size in covered):
            raise BuildError(f"non-zero byte outside the fixed layout at GPA 0x{gpa:x}")
    return {
        "entry_gpa": f"0x{entry:x}",
        "handler_gpa": f"0x{handler:x}",
        "code_end_gpa": f"0x{code_end:x}",
        "gdt_gpa": f"0x{GDT_GPA:x}",
        "idt_gpa": f"0x{IDT_GPA:x}",
        "idtr_gpa": f"0x{IDTR_GPA:x}",
        "vector": f"0x{VECTOR:x}",
        "load_gpa": f"0x{IMAGE_BASE:x}",
        "image_size": f"0x{IMAGE_SIZE:x}",
        "segments": [
            {"paddr": f"0x{p:x}", "filesz": f"0x{s:x}", "flags": f"0x{f:x}"} for _, p, s, f in expected
        ],
    }


def build(args: argparse.Namespace) -> int:
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/uart_irq_probe"
    if not 0 <= args.ier <= 0xFF:
        raise BuildError("--ier must fit in one byte")
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    defines = [
        f"-DPROBE_IER={args.ier}",
        f"-DPROBE_LATE_UNMASK={int(args.late_unmask)}",
        f"-DPROBE_OMIT_EOI={int(args.omit_eoi)}",
        f"-DPROBE_OMIT_RBR_READ={int(args.omit_rbr_read)}",
    ]
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp", "-ffreestanding",
         "-fno-pic", "-fno-pie", "-mno-red-zone", *defines, "-c", str(fixture / "entry.S"),
         "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T", str(fixture / "layout.ld"),
         "-o", str(out / "probe.elf"), str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00", str(out / "probe.elf"),
         str(out / "probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "probe.elf").read_bytes()
    # objcopy starts at the lowest load address (the GDT at 0x2000).
    body = (out / "probe.unpadded.bin").read_bytes()
    raw = bytes(GDT_GPA - IMAGE_BASE) + body
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"image exceeds its fixed size: 0x{len(raw):x}")
    raw = raw.ljust(IMAGE_SIZE, b"\0")
    layout = validate_image(elf, raw, args)
    (out / "probe.bin").write_bytes(raw)
    (out / "probe.unpadded.bin").unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    variant = {
        "ier": args.ier,
        "late_unmask": args.late_unmask,
        "omit_eoi": args.omit_eoi,
        "omit_rbr_read": args.omit_rbr_read,
    }
    manifest = {
        "schema": "stage4-uart-irq-probe-build-v1",
        "profile": "Stage4UartIrqProbeV1",
        "variant": variant,
        "accepted_profile": variant == {"ier": 1, "late_unmask": False, "omit_eoi": False, "omit_rbr_read": False},
        "base_revision": revision,
        "source": {name: sha256((fixture / name).read_bytes()) for name in ("entry.S", "layout.ld")},
        "target": "x86_64-unknown-none-elf",
        "toolchain": {
            "clang": clang_version,
            "ld.lld": linker_version,
            "rust-objcopy": objcopy_version.splitlines()[0] if objcopy_version else "",
            "rustc": rustc_version.splitlines()[0],
            "host": host,
        },
        "commands": commands,
        "layout": layout,
        "artifacts": {
            "probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"output: {out}")
    print(f"raw sha256: {manifest['artifacts']['probe.bin']['sha256']}; variant: {variant}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--ier", type=lambda v: int(v, 0), default=1)
    parser.add_argument("--late-unmask", action="store_true")
    parser.add_argument("--omit-eoi", action="store_true")
    parser.add_argument("--omit-rbr-read", action="store_true")
    args = parser.parse_args()
    try:
        return build(args)
    except (BuildError, OSError) as error:
        print(f"build_stage4_uart_irq_probe: error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
