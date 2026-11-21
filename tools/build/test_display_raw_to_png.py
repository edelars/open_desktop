#!/usr/bin/env python3
"""Tests for the screen dump to PNG converter (S8-11)."""

from __future__ import annotations

import struct
import subprocess
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/display_raw_to_png.py"


def dump(width: int, height: int, pixels: bytes, stride: int | None = None, pixel_format: int = 1) -> bytes:
    stride = width * 4 if stride is None else stride
    header = struct.pack("<8s6I4Q", b"ODFBRAW1", 64, width, height, stride, pixel_format, 0, 3, 1, 5_000, 99)
    return header + pixels


def decode_png(data: bytes) -> tuple[int, int, list[bytes]]:
    """Minimal decoder for what the converter writes: 8-bit RGB, filter 0, one IDAT."""
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    position, chunks = 8, {}
    while position < len(data):
        length = struct.unpack(">I", data[position : position + 4])[0]
        kind = data[position + 4 : position + 8]
        payload = data[position + 8 : position + 8 + length]
        crc = struct.unpack(">I", data[position + 8 + length : position + 12 + length])[0]
        assert crc == zlib.crc32(kind + payload) & 0xFFFFFFFF, kind
        chunks[kind] = payload
        position += 12 + length
    width, height, depth, color, _, _, _ = struct.unpack(">IIBBBBB", chunks[b"IHDR"])
    assert (depth, color) == (8, 2)
    raw = zlib.decompress(chunks[b"IDAT"])
    rows = []
    for y in range(height):
        line = raw[y * (width * 3 + 1) : (y + 1) * (width * 3 + 1)]
        assert line[0] == 0
        rows.append(line[1:])
    return width, height, rows


def run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, "-I", str(SCRIPT), *args], capture_output=True, text=True, check=False)


class DisplayRawToPngTest(unittest.TestCase):
    def convert(self, data: bytes, *extra: str) -> tuple[subprocess.CompletedProcess[str], bytes | None]:
        with tempfile.TemporaryDirectory() as tmp:
            source, target = Path(tmp) / "in.raw", Path(tmp) / "out.png"
            source.write_bytes(data)
            result = run(str(source), str(target), *extra)
            return result, target.read_bytes() if target.exists() else None

    def test_dump_with_header_becomes_rgb(self) -> None:
        # Two pixels: B,G,R,X = 10,20,30,255 and 1,2,3,0; second row has stride padding.
        pixels = bytes([10, 20, 30, 255, 1, 2, 3, 0, 9, 9, 9, 9, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7])
        result, png = self.convert(dump(2, 2, pixels, stride=12))
        self.assertEqual(result.returncode, 0, result.stderr)
        width, height, rows = decode_png(png)
        self.assertEqual((width, height), (2, 2))
        self.assertEqual(rows[0], bytes([30, 20, 10, 3, 2, 1]))
        self.assertEqual(rows[1], bytes([6, 5, 4, 7, 7, 7]))

    def test_headerless_frame_with_raw_size(self) -> None:
        result, png = self.convert(bytes([0, 0, 255, 0, 0, 255, 0, 0]), "--raw-size", "2x1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(decode_png(png)[2], [bytes([255, 0, 0, 0, 255, 0])])

    def test_bad_inputs_are_refused_without_output(self) -> None:
        good = bytes(16)
        for data, extra, text in [
            (b"short", (), "header is missing"),
            (dump(2, 2, good, pixel_format=2), (), "pixel format"),
            (dump(2, 2, good[:8]), (), "frame has"),
            (dump(2, 2, good, stride=4), (), "smaller than"),
            (dump(0, 2, b""), (), "unsupported size"),
            (good, ("--raw-size", "3x3"), "frame has"),
        ]:
            result, png = self.convert(data, *extra)
            self.assertEqual(result.returncode, 2, text)
            self.assertIn(text, result.stderr)
            self.assertIsNone(png)

    def test_existing_output_is_not_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source, target = Path(tmp) / "in.raw", Path(tmp) / "out.png"
            source.write_bytes(dump(1, 1, bytes(4)))
            target.write_bytes(b"keep")
            self.assertEqual(run(str(source), str(target)).returncode, 2)
            self.assertEqual(target.read_bytes(), b"keep")


if __name__ == "__main__":
    unittest.main()
