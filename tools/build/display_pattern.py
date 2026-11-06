#!/usr/bin/env python3
"""Generate deterministic raw test patterns for the Stage 10 display checks.

Output format: raw, 4 bytes per pixel, rows top to bottom, no padding
(stride = width * 4). Bytes per pixel in memory order: B, G, R, X, where
X is always 0x00. All arithmetic is integer. The formulas are documented in
tests/fixtures/guest/stage10/README.md so the guest probe (S10-08) can
reproduce them without this script.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path


MAX_WIDTH = 4096
MAX_HEIGHT = 2304
PATTERNS = ("gradient", "bars", "checker")
# (R, G, B) per bar, left to right before the seed rotation.
BARS_COLORS = (
    (0xFF, 0xFF, 0xFF),  # white
    (0xFF, 0xFF, 0x00),  # yellow
    (0x00, 0xFF, 0xFF),  # cyan
    (0x00, 0xFF, 0x00),  # green
    (0xFF, 0x00, 0xFF),  # magenta
    (0xFF, 0x00, 0x00),  # red
    (0x00, 0x00, 0xFF),  # blue
    (0x00, 0x00, 0x00),  # black
)
CHECKER_CELL = 8


class PatternError(Exception):
    pass


def pixel_rgb(pattern: str, x: int, y: int, width: int, height: int, seed: int) -> tuple[int, int, int]:
    """Return (R, G, B) for one pixel. Every value is in 0..255."""
    if pattern == "gradient":
        r = (x * 256 // width) ^ (seed & 0xFF)
        g = (y * 256 // height) ^ ((seed >> 8) & 0xFF)
        b = ((x + y) & 0xFF) ^ ((seed >> 16) & 0xFF)
        return r, g, b
    if pattern == "bars":
        index = (x * len(BARS_COLORS) // width + seed) % len(BARS_COLORS)
        return BARS_COLORS[index]
    if pattern == "checker":
        if ((x // CHECKER_CELL) + (y // CHECKER_CELL)) & 1 == 0:
            return 0x00, 0x00, 0x00
        return (0xFF ^ (seed & 0xFF), 0xFF ^ ((seed >> 8) & 0xFF), 0xFF ^ ((seed >> 16) & 0xFF))
    raise PatternError(f"unknown pattern: {pattern}")


def render(pattern: str, width: int, height: int, seed: int) -> bytes:
    if not 1 <= width <= MAX_WIDTH:
        raise PatternError(f"width must be in 1..{MAX_WIDTH}")
    if not 1 <= height <= MAX_HEIGHT:
        raise PatternError(f"height must be in 1..{MAX_HEIGHT}")
    if not 0 <= seed <= 0xFFFFFFFF:
        raise PatternError("seed must fit in 32 bits")
    out = bytearray(width * height * 4)
    offset = 0
    for y in range(height):
        for x in range(width):
            r, g, b = pixel_rgb(pattern, x, y, width, height, seed)
            out[offset] = b
            out[offset + 1] = g
            out[offset + 2] = r
            out[offset + 3] = 0x00
            offset += 4
    return bytes(out)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Write a deterministic raw 32-bit test pattern and print its SHA-256."
    )
    parser.add_argument("--width", type=int, required=True, help=f"1..{MAX_WIDTH}")
    parser.add_argument("--height", type=int, required=True, help=f"1..{MAX_HEIGHT}")
    parser.add_argument("--pattern", choices=PATTERNS, required=True)
    parser.add_argument(
        "--seed", type=lambda text: int(text, 0), default=0, help="0..2^32-1 (decimal or 0x hex)"
    )
    parser.add_argument("--output", type=Path, required=True, help="raw file to write")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        data = render(args.pattern, args.width, args.height, args.seed)
    except PatternError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(data)
    digest = hashlib.sha256(data).hexdigest()
    print(f"sha256={digest} bytes={len(data)} path={args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
