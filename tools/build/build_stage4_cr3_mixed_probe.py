#!/usr/bin/env python3
"""Build and validate the fixed freestanding mixed-page CR3 probe image."""

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
IMAGE_SIZE = 0x201000
ENTRIES = {
    0x1000: 0x2003,
    0x2000: 0x3003,
    0x3010: 0x4003,
    0x4000: 0xB001,
    0x5000: 0x6003,
    0x6000: 0x7003,
    0x7010: 0x200083,
}
CONTENTS = [
    (0xB000, bytes.fromhex("0f22d80f0b000000000f0b")),
    (0x200000, bytes.fromhex("000000488b030f22d80f0b")),
    (0x201000, (0x1000).to_bytes(8, "little")),
]
# (vaddr, paddr, filesz, flags) for the four PT_LOAD segments.
SEGMENTS = [
    (0x1000, 0x1000, 0x7000, 6),
    (0x400000, 0xB000, 11, 5),
    (0x401000, 0x200000, 11, 5),
    (0x402000, 0x201000, 8, 6),
]


def expected_image() -> bytes:
    image = bytearray(IMAGE_SIZE)
    for address, value in ENTRIES.items():
        image[address - IMAGE_BASE : address - IMAGE_BASE + 8] = value.to_bytes(8, "little")
    for address, data in CONTENTS:
        image[address - IMAGE_BASE : address - IMAGE_BASE + len(data)] = data
    return bytes(image)


def validate_image(elf: bytes, raw: bytes) -> None:
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
    if len(raw) > IMAGE_SIZE:
        raise BuildError("oversized image")
    if raw.ljust(IMAGE_SIZE, b"\0") != expected_image():
        raise BuildError("image does not match the fixed mixed CR3 profile")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    fixture = root / "tests/fixtures/guest/stage4/cr3_mixed_probe"
    out = output_directory(root, args.output_dir)
    clang = resolve_executable(os.environ.get("STAGE4_CLANG"), "clang")
    rustc, host, rustc_version, linker, objcopy, env = rust_toolchain()
    env.update({"LC_ALL": "C", "SOURCE_DATE_EPOCH": "0"})
    clang_version = run([str(clang), "--version"], env=env).splitlines()[0]
    linker_version = run([str(linker), "--version"], env=env).splitlines()[0]
    objcopy_version = run([str(objcopy), "--version"], env=env)
    commands = [
        [str(clang), "--target=x86_64-unknown-none-elf", "-x", "assembler-with-cpp",
         "-ffreestanding", "-fno-pic", "-fno-pie", "-mno-red-zone", "-c",
         str(fixture / "entry.S"), "-o", str(out / "entry.o")],
        [str(linker), "-m", "elf_x86_64", "--build-id=none", "-T",
         str(fixture / "layout.ld"), "-o", str(out / "probe.elf"), str(out / "entry.o")],
        [str(objcopy), "-O", "binary", "--gap-fill=0x00", str(out / "probe.elf"),
         str(out / "probe.bin")],
    ]
    for command in commands:
        run(command, env=env, cwd=root)
    elf = (out / "probe.elf").read_bytes()
    raw = (out / "probe.bin").read_bytes()
    validate_image(elf, raw)
    raw = raw.ljust(IMAGE_SIZE, b"\0")
    (out / "probe.bin").write_bytes(raw)
    revision = run(["git", "rev-parse", "HEAD"], env=env, cwd=root).strip()
    manifest = {
        "schema": "stage4-cr3-mixed-probe-build-v1",
        "profile": "Stage4Cr3MixedProbeV1",
        "root_a": "0x1000",
        "root_b": "0x5000",
        "entry_gva": "0x400000",
        "data_gva": "0x401000",
        "code_gpa": ["0xb000", "0x200000"],
        "data_gpa": ["0x201000"],
        "raw_sha256": sha256(raw),
        "elf_sha256": sha256(elf),
        "base_revision": revision,
        "inputs": {
            name: sha256((fixture / name).read_bytes()) for name in ("entry.S", "layout.ld")
        },
        "tools": {
            "clang": clang_version,
            "linker": linker_version,
            "objcopy": objcopy_version.splitlines()[0] if objcopy_version else "",
            "rustc": rustc_version.splitlines()[0],
            "host": host,
        },
        "commands": commands,
        "artifacts": {
            "probe.elf": {"size": len(elf), "sha256": sha256(elf)},
            "probe.bin": {"size": len(raw), "sha256": sha256(raw)},
        },
    }
    (out / "build_manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"raw sha256: {manifest['raw_sha256']}; output: {out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
