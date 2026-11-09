#!/usr/bin/env python3
"""Build and validate the freestanding S4-33b CPL3 SYSCALL/SYSRETQ probe image."""

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
IMAGE_SIZE = 0x9000
NX = 1 << 63
ENTRIES = {
    0x1000: 0x2007,
    0x2000: 0x3007,
    0x3010: 0x4007,  # GVA 0x400000 user code table
    0x3018: 0x6003,  # GVA 0x600000 kernel code table (supervisor)
    0x3020: 0x5007,  # GVA 0x800000 data table
    0x4000: 0x7005,  # user read/execute
    0x5000: 0x9003 | NX,  # supervisor RW alias
    0x5008: 0x9005 | NX,  # user RO alias of the same GPA
    0x6000: 0x8001,  # supervisor read/execute
}
USER_CODE = bytes.fromhex("0f05488b03")  # SYSCALL; MOV RAX,[RBX]
KERNEL_CODE = bytes.fromhex("4c8917480f07")  # MOV [RDI],R10; SYSRETQ
USER_CODE_GPA = 0x7000
KERNEL_CODE_GPA = 0x8000
DATA_GPA = 0x9000
# (vaddr, paddr, filesz, flags) of the four PT_LOAD segments.
SEGMENTS = [
    (0x1000, 0x1000, 0x6000, 6),
    (0x400000, USER_CODE_GPA, len(USER_CODE), 5),
    (0x600000, KERNEL_CODE_GPA, 8, 5),  # lld rounds this segment up to 8 bytes; tail is zero
    (0x800000, DATA_GPA, 16, 6),
]
LAYOUT = {
    "entry_gva": "0x400000",
    "lstar_gva": "0x600000",
    "user_code": {"gva": "0x400000", "gpa": "0x7000", "bytes": USER_CODE.hex(), "raw_offset": "0x6000"},
    "kernel_code": {"gva": "0x600000", "gpa": "0x8000", "bytes": KERNEL_CODE.hex(), "raw_offset": "0x7000"},
    "mappings": [
        {"gva": "0x800000", "gpa": "0x9000", "access": "supervisor-rw", "raw_offset": "0x8000"},
        {"gva": "0x801000", "gpa": "0x9000", "access": "user-ro", "raw_offset": "0x8000"},
    ],
    "initial_qword_raw_offset": "0x8000",
    "store_qword_raw_offset": "0x8008",
}


def expected_image(initial: int, store: int) -> bytes:
    image = bytearray(IMAGE_SIZE)
    for address, value in ENTRIES.items():
        image[address - IMAGE_BASE : address - IMAGE_BASE + 8] = value.to_bytes(8, "little")
    for gpa, code in ((USER_CODE_GPA, USER_CODE), (KERNEL_CODE_GPA, KERNEL_CODE)):
        image[gpa - IMAGE_BASE : gpa - IMAGE_BASE + len(code)] = code
    image[DATA_GPA - IMAGE_BASE : DATA_GPA - IMAGE_BASE + 8] = initial.to_bytes(8, "little")
    image[DATA_GPA - IMAGE_BASE + 8 : DATA_GPA - IMAGE_BASE + 16] = store.to_bytes(8, "little")
    return bytes(image)


def validate_image(elf: bytes, raw: bytes, initial: int, store: int) -> None:
    if initial == store:
        raise BuildError("initial and store qwords must differ")
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
        start = paddr - IMAGE_BASE
        if raw[start : start + filesz] != elf[segment["offset"] : segment["offset"] + filesz]:
            raise BuildError("raw segment does not match ELF")
    if len(raw) != IMAGE_SIZE:
        raise BuildError("image size is not the fixed profile size")
    if raw != expected_image(initial, store):
        raise BuildError("image does not match the fixed CPL3 syscall profile")


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
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/cpl3_syscall"
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
         f"-DPROBE_STORE_VALUE=0x{args.store_value:016x}", "-c",
         str(fixture / "entry.S"), "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T",
         str(fixture / "layout.ld"), "-o", str(out / "cpl3_syscall_probe.elf"),
         str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00",
         str(out / "cpl3_syscall_probe.elf"), str(out / "cpl3_syscall_probe.unpadded.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "cpl3_syscall_probe.elf").read_bytes()
    unpadded = out / "cpl3_syscall_probe.unpadded.bin"
    raw = unpadded.read_bytes()
    if len(raw) > IMAGE_SIZE:
        raise BuildError(f"objcopy image exceeds fixed size: {len(raw):#x}")
    raw = raw + bytes(IMAGE_SIZE - len(raw))
    validate_image(elf, raw, args.initial_value, args.store_value)
    (out / "cpl3_syscall_probe.bin").write_bytes(raw)
    unpadded.unlink()
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    manifest = {
        "schema": "stage4-cpl3-syscall-probe-build-v1",
        "profile": "Stage4Cpl3SyscallProbeV1",
        "source_revision": revision,
        "initial_u64": f"0x{args.initial_value:016x}",
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
            "cpl3_syscall_probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "cpl3_syscall_probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"raw sha256: {manifest['artifacts']['cpl3_syscall_probe.bin']['sha256']}; output: {out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
