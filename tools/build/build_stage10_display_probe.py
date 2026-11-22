#!/usr/bin/env python3
"""Build the Stage 10 guest display probe (S10-08): image and JSON manifest.

The probe is own freestanding x86-64 code, hand-assembled here (no external
assembler, no foreign boot code). It is wrapped in the ODFWIMG1 firmware image
container (docs/specs/FIRMWARE_BOOT.md 3.2) and is run by the firmware board
(od-machine load_firmware_image / run_boot), the same path that already
executes dword MMIO stores for the display registers (DISPLAY_DEVICE.md 13.1).

Guest program, Long64 CPL0, identity-mapped boot page tables:

1. write WIDTH, HEIGHT, FORMAT and MODE_COMMIT (dword MMIO stores);
2. read STATUS into EDX (expected 0x1: ACTIVE);
3. store the `bars` pattern of tests/fixtures/guest/stage10/README.md into the
   framebuffer memory, one dword per pixel, computed by the guest;
4. write FRAME_GENERATION (doorbell), read STATUS into R9D (expected 0x3);
5. HLT.

Pattern in the guest, without division: `acc` counts 8 per pixel and the bar
index `idx` (seed mod 8 at the start of each row) is incremented, modulo 8, each time
`acc` reaches WIDTH (then WIDTH is subtracted). For WIDTH >= 8 this equals
(x * 8 // WIDTH + seed) mod 8 for every x, the formula of the reference.

The stored image is the container with trailing zero bytes trimmed; the loader
pads it with zeros to 4 MiB. The CRC32 in the header is computed over the padded
image, exactly as the firmware board checks it.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

IMAGE_BYTES = 0x0040_0000
HEADER_BYTES = 0x40
LINK_BASE = 0x0020_0000
CRC_OFFSET = 0x30
CODE_OFFSET = HEADER_BYTES
TABLE_OFFSET = 0x100
TABLE_GVA = LINK_BASE + TABLE_OFFSET

DISPLAY_REGISTERS_BASE = 0xFC00_0000
DISPLAY_FRAMEBUFFER_BASE = 0xF800_0000
FORMAT_BGRX8888 = 1
MIN_WIDTH = 8
MAX_WIDTH = 4096
MAX_HEIGHT = 2304
BAR_COUNT = 8
STATUS_AFTER_COMMIT = 0x1  # ACTIVE
STATUS_AFTER_FRAME = 0x3  # ACTIVE | FRAME_SINCE_COMMIT
BUDGET_FACTOR = 2


class BuildError(Exception):
    pass


def _load_display_pattern():
    path = ROOT / "tools/build/display_pattern.py"
    spec = importlib.util.spec_from_file_location("display_pattern_for_stage10_probe", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


DISPLAY_PATTERN = _load_display_pattern()


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def u32(value: int) -> bytes:
    return struct.pack("<I", value)


class Assembler:
    """Straight-line code with short backward and forward jumps."""

    def __init__(self) -> None:
        self.code = bytearray()
        self.labels: dict[str, int] = {}
        self.fixups: list[tuple[int, str]] = []

    def emit(self, *parts: bytes | int) -> None:
        for part in parts:
            if isinstance(part, int):
                self.code.append(part)
            else:
                self.code += part

    def label(self, name: str) -> None:
        self.labels[name] = len(self.code)

    def jump8(self, opcode: int, target: str) -> None:
        self.emit(opcode, 0)
        self.fixups.append((len(self.code) - 1, target))

    def finish(self) -> bytes:
        for position, target in self.fixups:
            displacement = self.labels[target] - (position + 1)
            if not -128 <= displacement <= 127:
                raise BuildError(f"jump to {target} does not fit in rel8: {displacement}")
            self.code[position] = displacement & 0xFF
        return bytes(self.code)


def bars_table_dwords() -> list[int]:
    """Eight 0x00RRGGBB values, in the reference order (display_pattern.BARS_COLORS)."""
    return [(r << 16) | (g << 8) | b for r, g, b in DISPLAY_PATTERN.BARS_COLORS]


def validate_parameters(pattern: str, width: int, height: int, seed: int) -> None:
    if pattern != "bars":
        raise BuildError("only the bars pattern is implemented by the guest probe")
    if not MIN_WIDTH <= width <= MAX_WIDTH:
        raise BuildError(f"width must be in {MIN_WIDTH}..{MAX_WIDTH}")
    if not 1 <= height <= MAX_HEIGHT:
        raise BuildError(f"height must be in 1..{MAX_HEIGHT}")
    if not 0 <= seed <= 0xFFFFFFFF:
        raise BuildError("seed must fit in 32 bits")


def assemble_probe(width: int, height: int, seed: int) -> tuple[bytes, int]:
    """Return (machine code, retired instruction count up to and including HLT)."""
    a = Assembler()
    # Display registers: ESI = 0xFC000000.
    a.emit(0xBE, u32(DISPLAY_REGISTERS_BASE))  # mov esi, imm32
    a.emit(0xB9, u32(width), 0x89, 0x4E, 0x40)  # mov ecx, W ; mov [rsi+0x40], ecx
    a.emit(0xB9, u32(height), 0x89, 0x4E, 0x44)  # mov ecx, H ; mov [rsi+0x44], ecx
    a.emit(0xB9, u32(FORMAT_BGRX8888), 0x89, 0x4E, 0x48)  # mov ecx, 1 ; mov [rsi+FORMAT], ecx
    a.emit(0x89, 0x4E, 0x4C)  # mov [rsi+MODE_COMMIT], ecx (ECX = 1)
    a.emit(0x8B, 0x56, 0x70)  # mov edx, [rsi+STATUS]
    a.emit(0xBF, u32(DISPLAY_FRAMEBUFFER_BASE))  # mov edi, framebuffer
    a.emit(0xBD, u32(height))  # mov ebp, H
    setup = 11
    a.label("row")
    a.emit(0xB9, u32(width))  # mov ecx, W
    a.emit(0x45, 0x31, 0xC0)  # xor r8d, r8d  (acc)
    a.emit(0xBB, u32(seed & 7))  # mov ebx, seed mod 8  (idx)
    a.label("pixel")
    a.emit(0x8B, 0x04, 0x9D, u32(TABLE_GVA))  # mov eax, [rbx*4 + table]
    a.emit(0x89, 0x07)  # mov [rdi], eax
    a.emit(0x83, 0xC7, 0x04)  # add edi, 4
    a.emit(0x41, 0x83, 0xC0, 0x08)  # add r8d, 8
    a.emit(0x41, 0x81, 0xF8, u32(width))  # cmp r8d, W
    a.jump8(0x72, "skip")  # jb skip
    a.emit(0x41, 0x81, 0xE8, u32(width))  # sub r8d, W
    a.emit(0xFF, 0xC3)  # inc ebx
    a.emit(0x83, 0xE3, 0x07)  # and ebx, 7
    a.label("skip")
    a.emit(0xFF, 0xC9)  # dec ecx
    a.jump8(0x75, "pixel")  # jnz pixel
    a.emit(0xFF, 0xCD)  # dec ebp
    a.jump8(0x75, "row")  # jnz row
    a.emit(0xB9, u32(1), 0x89, 0x8E, u32(0x80))  # mov ecx, 1 ; mov [rsi+FRAME_GENERATION], ecx
    a.emit(0x44, 0x8B, 0x4E, 0x70)  # mov r9d, [rsi+STATUS]
    a.emit(0xF4)  # hlt
    tail = 4
    # Per row: 3 before the pixel loop, 2 after; per pixel 8, and 3 more for each of
    # the 8 carries (WIDTH >= 8: acc < WIDTH after each pixel, so one subtraction suffices).
    per_row = 3 + 2 + 8 * width + 3 * BAR_COUNT
    return a.finish(), setup + height * per_row + tail


def build_image(code: bytes) -> bytes:
    """ODFWIMG1 container, trailing zero bytes trimmed; CRC32 over the padded image."""
    if CODE_OFFSET + len(code) > TABLE_OFFSET:
        raise BuildError("code overlaps the color table")
    table = b"".join(u32(value) for value in bars_table_dwords())
    full = bytearray(IMAGE_BYTES)
    full[0:8] = b"ODFWIMG1"
    full[0x08:0x0C] = u32(1)  # format_version
    full[0x0C:0x10] = u32(HEADER_BYTES)
    full[0x10:0x18] = struct.pack("<Q", IMAGE_BYTES)
    full[0x18:0x20] = struct.pack("<Q", LINK_BASE)
    full[0x20:0x28] = struct.pack("<Q", CODE_OFFSET)  # entry_offset
    full[0x28:0x2A] = struct.pack("<H", 1)  # handoff_major
    full[0x2A:0x2C] = struct.pack("<H", 0)  # handoff_minor_max (0: every minor passes)
    full[CODE_OFFSET : CODE_OFFSET + len(code)] = code
    full[TABLE_OFFSET : TABLE_OFFSET + len(table)] = table
    full[CRC_OFFSET : CRC_OFFSET + 4] = u32(zlib.crc32(bytes(full)))
    return bytes(full[: TABLE_OFFSET + len(table)])


def pad_image(stored: bytes) -> bytes:
    if len(stored) > IMAGE_BYTES:
        raise BuildError("stored image is longer than the firmware image")
    return stored + bytes(IMAGE_BYTES - len(stored))


def image_crc_is_valid(stored: bytes) -> bool:
    full = bytearray(pad_image(stored))
    expected = struct.unpack_from("<I", full, CRC_OFFSET)[0]
    full[CRC_OFFSET : CRC_OFFSET + 4] = bytes(4)
    return zlib.crc32(bytes(full)) == expected


def simulate_guest_bars(width: int, height: int, seed: int) -> bytes:
    """The guest's loop in Python (acc/idx, no division): the BGRX frame it stores."""
    table = bars_table_dwords()
    out = bytearray()
    for _ in range(height):
        acc = 0
        idx = seed & 7
        for _ in range(width):
            out += u32(table[idx])
            acc += 8
            if acc >= width:
                acc -= width
                idx = (idx + 1) & 7
    return bytes(out)


def build(pattern: str, width: int, height: int, seed: int) -> tuple[bytes, dict]:
    validate_parameters(pattern, width, height, seed)
    code, retired = assemble_probe(width, height, seed)
    image = build_image(code)
    reference = DISPLAY_PATTERN.render(pattern, width, height, seed)
    if simulate_guest_bars(width, height, seed) != reference:
        raise BuildError("guest algorithm disagrees with the reference pattern")
    stem = f"display_probe_{pattern}_{width}x{height}_seed{seed:#x}" if seed else (
        f"display_probe_{pattern}_{width}x{height}_seed0"
    )
    manifest = {
        "schema_version": 1,
        "name": "Stage10DisplayProbeV1",
        "generator": "tools/build/build_stage10_display_probe.py",
        "command": (
            f"python3 tools/build/build_stage10_display_probe.py --pattern {pattern} "
            f"--width {width} --height {height} --seed {seed:#x} --output-dir tests/fixtures/guest/stage10"
        ),
        "image": f"{stem}.img",
        "image_container": "ODFWIMG1, trailing zero bytes trimmed; zero-padded to image_padded_bytes before load",
        "image_bytes": len(image),
        "image_padded_bytes": IMAGE_BYTES,
        "image_sha256": sha256(image),
        "image_padded_sha256": sha256(pad_image(image)),
        "pattern": pattern,
        "width": width,
        "height": height,
        "seed": seed,
        "format": FORMAT_BGRX8888,
        "stride": width * 4,
        "frame_bytes": len(reference),
        "expected_frame_sha256": sha256(reference),
        "reference_frame": "bars_64x48_seed0.raw" if (pattern, width, height, seed) == ("bars", 64, 48, 0) else None,
        "expected_status_after_commit": STATUS_AFTER_COMMIT,
        "expected_status_after_frame": STATUS_AFTER_FRAME,
        "expected_generation": 1,
        "expected_retired": retired,
        "instruction_budget": retired * BUDGET_FACTOR,
    }
    return image, manifest


def stem_of(manifest: dict) -> str:
    return manifest["image"].removesuffix(".img")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--pattern", choices=("bars",), default="bars")
    parser.add_argument("--width", type=int, required=True, help=f"{MIN_WIDTH}..{MAX_WIDTH}")
    parser.add_argument("--height", type=int, required=True, help=f"1..{MAX_HEIGHT}")
    parser.add_argument("--seed", type=lambda text: int(text, 0), default=0)
    parser.add_argument("--output-dir", type=Path, required=True)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        image, manifest = build(args.pattern, args.width, args.height, args.seed)
    except (BuildError, DISPLAY_PATTERN.PatternError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / manifest["image"]).write_bytes(image)
    manifest_path = args.output_dir / f"{stem_of(manifest)}.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(
        f"image_sha256={manifest['image_sha256']} frame_sha256={manifest['expected_frame_sha256']} "
        f"retired={manifest['expected_retired']} manifest={manifest_path}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
