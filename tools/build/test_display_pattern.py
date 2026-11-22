#!/usr/bin/env python3
"""Tests for the Stage 10 display pattern generator and frame comparer."""

from __future__ import annotations

import hashlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PATTERN_SCRIPT = ROOT / "tools/build/display_pattern.py"
COMPARE_SCRIPT = ROOT / "tests/system/display_compare.py"
FIXTURES = ROOT / "tests/fixtures/guest/stage10"
FIXTURE_HASHES = {
    "bars_64x48_seed0.raw": "a738f2f35ccf34d9bca698c40fcca5cc8ab4bda80b58004f6b42a71228a5a3e2",
    "checker_64x48_seed0x204080.raw": "12779eb36b1949960129604b0f81f19cc8ec5c58d70c43240b3f744c0884d529",
    # Screen 1 of the two-screen test profile (S10-15, DISPLAY_DEVICE.md 16.13 item 10).
    "checker_80x60_seed0x204080.raw": "9d7970a72f5c37a0c4035d948b307dacd0b09763717aff81658bec7b0d0eccdb",
}


def run_script(script: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(script), *args],
        capture_output=True,
        text=True,
        check=False,
    )


class DisplayPatternTest(unittest.TestCase):
    def test_same_arguments_produce_same_hash(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            first = Path(tmp) / "first.raw"
            second = Path(tmp) / "second.raw"
            args = ["--width", "64", "--height", "48", "--pattern", "bars", "--seed", "3"]
            self.assertEqual(run_script(PATTERN_SCRIPT, *args, "--output", str(first)).returncode, 0)
            self.assertEqual(run_script(PATTERN_SCRIPT, *args, "--output", str(second)).returncode, 0)
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_committed_fixtures_match_generator_output(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bars = Path(tmp) / "bars.raw"
            checker = Path(tmp) / "checker.raw"
            run_script(
                PATTERN_SCRIPT, "--width", "64", "--height", "48", "--pattern", "bars",
                "--seed", "0", "--output", str(bars),
            )
            run_script(
                PATTERN_SCRIPT, "--width", "64", "--height", "48", "--pattern", "checker",
                "--seed", "0x204080", "--output", str(checker),
            )
            checker_second = Path(tmp) / "checker_second.raw"
            run_script(
                PATTERN_SCRIPT, "--width", "80", "--height", "60", "--pattern", "checker",
                "--seed", "0x204080", "--output", str(checker_second),
            )
            generated = {
                "bars_64x48_seed0.raw": bars,
                "checker_64x48_seed0x204080.raw": checker,
                "checker_80x60_seed0x204080.raw": checker_second,
            }
            for name, expected_hash in FIXTURE_HASHES.items():
                with self.subTest(fixture=name):
                    committed = (FIXTURES / name).read_bytes()
                    self.assertEqual(hashlib.sha256(committed).hexdigest(), expected_hash)
                    self.assertEqual(committed, generated[name].read_bytes())

    def test_rejects_out_of_range_size(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = run_script(
                PATTERN_SCRIPT, "--width", "0", "--height", "1", "--pattern", "bars",
                "--output", str(Path(tmp) / "x.raw"),
            )
            self.assertEqual(result.returncode, 2)

    def test_gradient_has_expected_first_and_last_pixels(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "gradient.raw"
            result = run_script(
                PATTERN_SCRIPT, "--width", "4", "--height", "2", "--pattern", "gradient",
                "--seed", "0", "--output", str(out),
            )
            self.assertEqual(result.returncode, 0)
            data = out.read_bytes()
            # Pixel (0,0): B=0, G=0, R=0, X=0. Pixel (3,1): R=3*256//4=192, G=1*256//2=128, B=(3+1)&0xFF=4.
            self.assertEqual(data[0:4], bytes([0, 0, 0, 0]))
            self.assertEqual(data[-4:], bytes([4, 128, 192, 0]))

    def test_help_exits_zero(self) -> None:
        self.assertEqual(run_script(PATTERN_SCRIPT, "--help").returncode, 0)


class DisplayCompareTest(unittest.TestCase):
    def setUp(self) -> None:
        self.reference = (FIXTURES / "bars_64x48_seed0.raw").read_bytes()
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, name: str, data: bytes) -> Path:
        path = self.dir / name
        path.write_bytes(data)
        return path

    def compare(self, actual: Path, *extra: str) -> subprocess.CompletedProcess[str]:
        reference = self.write("expected.raw", self.reference)
        return run_script(
            COMPARE_SCRIPT, str(reference), str(actual), "--width", "64", "--height", "48", *extra
        )

    def test_identical_frames_pass(self) -> None:
        self.assertEqual(self.compare(self.write("same.raw", self.reference)).returncode, 0)

    def test_one_byte_shift_is_detected(self) -> None:
        changed = bytearray(self.reference)
        changed[1000] ^= 1
        result = self.compare(self.write("shifted.raw", bytes(changed)))
        self.assertEqual(result.returncode, 1)
        self.assertIn("differing_pixels=1", result.stdout)

    def test_tolerance_allows_small_channel_difference(self) -> None:
        changed = bytearray(self.reference)
        changed[1000] ^= 1
        result = self.compare(self.write("tolerant.raw", bytes(changed)), "--tolerance", "1")
        self.assertEqual(result.returncode, 0)

    def test_lists_first_ten_differences_and_summarizes_rest(self) -> None:
        changed = bytearray(self.reference)
        for pixel in range(12):
            changed[pixel * 4] ^= 1
        result = self.compare(self.write("many.raw", bytes(changed)))
        self.assertEqual(result.returncode, 1)
        self.assertIn("differing_pixels=12", result.stdout)
        self.assertEqual(result.stdout.count("diff at "), 10)
        self.assertIn("2 more differing pixels not listed", result.stdout)

    def test_help_exits_zero(self) -> None:
        self.assertEqual(run_script(COMPARE_SCRIPT, "--help").returncode, 0)

    def test_size_mismatch_is_usage_error(self) -> None:
        result = self.compare(self.write("short.raw", self.reference[:-4]))
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
