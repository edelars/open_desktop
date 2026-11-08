#!/usr/bin/env python3
"""Compare two raw 32-bit frames (B, G, R, X per pixel) with a per-channel tolerance.

Exit status: 0 when every pixel matches within the tolerance, 1 when at least
one pixel differs, 2 on usage errors (bad arguments, tolerance, or file size).
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


BYTES_PER_PIXEL = 4
CHANNELS = ("B", "G", "R", "X")
REPORT_LIMIT = 10


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compare two raw frames pixel by pixel; tolerance is the maximum allowed "
        "difference per channel (default 0, exact match)."
    )
    parser.add_argument("expected", type=Path, help="reference raw frame")
    parser.add_argument("actual", type=Path, help="raw frame under test")
    parser.add_argument("--width", type=int, required=True)
    parser.add_argument("--height", type=int, required=True)
    parser.add_argument("--tolerance", type=int, default=0, help="0..255, default 0")
    return parser.parse_args(argv)


def compare(expected: bytes, actual: bytes, width: int, height: int, tolerance: int):
    """Return (differing_pixels, max_channel_diff, first_differences)."""
    expected_size = width * height * BYTES_PER_PIXEL
    if len(expected) != expected_size or len(actual) != expected_size:
        raise ValueError(
            f"frame size mismatch: reference file {len(expected)} bytes, actual file {len(actual)} bytes, "
            f"required {expected_size} for {width}x{height}x32"
        )
    differing = 0
    max_diff = 0
    first: list[tuple[int, int, tuple[int, ...], tuple[int, ...]]] = []
    for pixel in range(width * height):
        base = pixel * BYTES_PER_PIXEL
        e = expected[base : base + BYTES_PER_PIXEL]
        a = actual[base : base + BYTES_PER_PIXEL]
        diffs = [abs(ec - ac) for ec, ac in zip(e, a)]
        pixel_max = max(diffs)
        max_diff = max(max_diff, pixel_max)
        if pixel_max > tolerance:
            differing += 1
            if len(first) < REPORT_LIMIT:
                first.append((pixel % width, pixel // width, tuple(e), tuple(a)))
    return differing, max_diff, first


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.width < 1 or args.height < 1:
        print("error: width and height must be positive", file=sys.stderr)
        return 2
    if not 0 <= args.tolerance <= 255:
        print("error: tolerance must be in 0..255", file=sys.stderr)
        return 2
    try:
        expected = args.expected.read_bytes()
        actual = args.actual.read_bytes()
        differing, max_diff, first = compare(
            expected, actual, args.width, args.height, args.tolerance
        )
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    total = args.width * args.height
    print(
        f"size={args.width}x{args.height} pixels={total} tolerance={args.tolerance} "
        f"max_channel_diff={max_diff} differing_pixels={differing}"
    )
    for x, y, e, a in first:
        expected_text = " ".join(f"{name}={value:02x}" for name, value in zip(CHANNELS, e))
        actual_text = " ".join(f"{name}={value:02x}" for name, value in zip(CHANNELS, a))
        print(f"diff at x={x} y={y}: expected {expected_text} | actual {actual_text}")
    if differing:
        if differing > REPORT_LIMIT:
            print(f"... {differing - REPORT_LIMIT} more differing pixels not listed")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
