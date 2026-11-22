#!/usr/bin/env python3
"""Tests for the Stage 10 guest display probe builder (S10-08)."""

from __future__ import annotations

import importlib.util
import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/build_stage10_display_probe.py"
FIXTURES = ROOT / "tests/fixtures/guest/stage10"
SPEC = importlib.util.spec_from_file_location("stage10_display_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)

STEM = "display_probe_bars_64x48_seed0"


class Stage10DisplayProbeBuilderTest(unittest.TestCase):
    def test_committed_fixtures_are_what_the_generator_produces(self) -> None:
        image, manifest = BUILDER.build("bars", 64, 48, 0)
        self.assertEqual((FIXTURES / f"{STEM}.img").read_bytes(), image)
        self.assertEqual(
            json.loads((FIXTURES / f"{STEM}.json").read_text(encoding="utf-8")), manifest
        )

    def test_manifest_frame_hash_is_the_committed_reference(self) -> None:
        manifest = json.loads((FIXTURES / f"{STEM}.json").read_text(encoding="utf-8"))
        reference = (FIXTURES / "bars_64x48_seed0.raw").read_bytes()
        self.assertEqual(manifest["expected_frame_sha256"], BUILDER.sha256(reference))
        self.assertEqual(manifest["frame_bytes"], len(reference))
        self.assertEqual(manifest["reference_frame"], "bars_64x48_seed0.raw")

    def test_guest_algorithm_equals_the_reference_for_other_sizes_and_seeds(self) -> None:
        pattern = BUILDER.DISPLAY_PATTERN
        for width, height, seed in [
            (8, 3, 0),
            (13, 5, 3),
            (64, 4, 7),
            (100, 7, 0x12345678),
            (97, 2, 0xFFFFFFFF),
            (4096, 1, 5),
        ]:
            with self.subTest(width=width, height=height, seed=seed):
                self.assertEqual(
                    BUILDER.simulate_guest_bars(width, height, seed),
                    pattern.render("bars", width, height, seed),
                )

    def test_image_is_a_valid_firmware_container(self) -> None:
        image = (FIXTURES / f"{STEM}.img").read_bytes()
        self.assertEqual(image[:8], b"ODFWIMG1")
        self.assertEqual(struct.unpack_from("<I", image, 0x0C)[0], 0x40)
        self.assertEqual(struct.unpack_from("<Q", image, 0x10)[0], BUILDER.IMAGE_BYTES)
        self.assertEqual(struct.unpack_from("<Q", image, 0x18)[0], BUILDER.LINK_BASE)
        self.assertEqual(struct.unpack_from("<Q", image, 0x20)[0], BUILDER.CODE_OFFSET)
        self.assertTrue(BUILDER.image_crc_is_valid(image))
        self.assertLess(len(image), BUILDER.IMAGE_BYTES)
        # The color table is the reference bars order, 0x00RRGGBB per dword.
        table = image[BUILDER.TABLE_OFFSET : BUILDER.TABLE_OFFSET + 32]
        self.assertEqual(
            list(struct.unpack("<8I", table)),
            [0xFFFFFF, 0xFFFF00, 0x00FFFF, 0x00FF00, 0xFF00FF, 0xFF0000, 0x0000FF, 0x000000],
        )

    def test_crc_check_rejects_a_changed_byte(self) -> None:
        image = bytearray((FIXTURES / f"{STEM}.img").read_bytes())
        image[BUILDER.CODE_OFFSET + 1] ^= 0x01
        self.assertFalse(BUILDER.image_crc_is_valid(bytes(image)))

    def test_seed_changes_only_the_start_index_and_the_manifest(self) -> None:
        image0, manifest0 = BUILDER.build("bars", 64, 48, 0)
        image3, manifest3 = BUILDER.build("bars", 64, 48, 3)
        self.assertNotEqual(image0, image3)
        self.assertNotEqual(manifest0["expected_frame_sha256"], manifest3["expected_frame_sha256"])
        self.assertEqual(manifest0["expected_retired"], manifest3["expected_retired"])

    def test_retired_count_formula(self) -> None:
        for width, height in [(64, 48), (8, 1), (100, 7)]:
            _, manifest = BUILDER.build("bars", width, height, 0)
            self.assertEqual(
                manifest["expected_retired"], 11 + height * (5 + 8 * width + 24) + 4
            )

    def test_parameters_are_validated(self) -> None:
        for args in [
            ("checker", 64, 48, 0),
            ("bars", 7, 48, 0),
            ("bars", 4097, 48, 0),
            ("bars", 64, 0, 0),
            ("bars", 64, 2305, 0),
            ("bars", 64, 48, -1),
            ("bars", 64, 48, 2**32),
        ]:
            with self.subTest(args=args), self.assertRaises(BUILDER.BuildError):
                BUILDER.build(*args)

    def test_command_line_writes_image_and_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--width",
                    "64",
                    "--height",
                    "48",
                    "--output-dir",
                    directory,
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                (Path(directory) / f"{STEM}.img").read_bytes(),
                (FIXTURES / f"{STEM}.img").read_bytes(),
            )
            self.assertEqual(
                (Path(directory) / f"{STEM}.json").read_text(encoding="utf-8"),
                (FIXTURES / f"{STEM}.json").read_text(encoding="utf-8"),
            )

    def test_command_line_reports_bad_parameters(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--width", "4", "--height", "48", "--output-dir", directory],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("width", result.stderr)
            self.assertEqual(list(Path(directory).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
