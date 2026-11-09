#!/usr/bin/env python3
"""Reproducibility and validator tests for the S4-33d cross-page probe builder."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/build_stage4_cross_page_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_cross_page_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)
VALUE_A = 0x8877665544332211
# Same first-page bytes (11 22 33), different bytes on the second page.
VALUE_B = 0xA1B2C3D4E5332211


def build(output: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *extra],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


def le(raw: bytes, offset: int, length: int) -> int:
    return int.from_bytes(raw[offset : offset + length], "little")


class Stage4CrossPageProbeBuilderTest(unittest.TestCase):
    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            outputs = [Path(temporary) / "one", Path(temporary) / "two"]
            for output in outputs:
                self.assertEqual(build(output).returncode, 0)
            raw = [(output / "cross_page_probe.bin").read_bytes() for output in outputs]
            self.assertEqual(raw[0], raw[1])
            self.assertEqual(len(raw[0]), BUILDER.IMAGE_SIZE)
            self.assertEqual(raw[0], BUILDER.expected_image(VALUE_A))
            digest = hashlib.sha256(raw[0]).hexdigest()
            for output in outputs:
                manifest = json.loads((output / "build_manifest.json").read_text())
                self.assertEqual(manifest["artifacts"]["cross_page_probe.bin"]["sha256"], digest)
                self.assertEqual(manifest["profile"], "Stage4CrossPageProbeV1")
                self.assertEqual(manifest["first_page_bytes"], "112233")
                self.assertEqual(manifest["second_page_bytes"], "4455667788")
                layout = manifest["layout"]
                self.assertEqual(layout["code"]["head"]["bytes"], "668b")
                self.assertEqual(layout["code"]["tail"]["bytes"], "03488b038b03668b03eef4")
                self.assertEqual(layout["operand"]["rbx"], "0x800ffd")
                # Both data mappings and both code mappings are separate, non-adjacent frames.
                gpas = [int(m["gpa"], 16) for m in layout["mappings"]]
                self.assertEqual(gpas, [0x7000, 0x9000, 0xB000, 0xD000])
                self.assertTrue(all(b - a >= 0x2000 for a, b in ((gpas[0], gpas[1]), (gpas[2], gpas[3]))))

    def test_value_bytes_land_on_both_pages_and_the_second_value_changes_only_page_one(self) -> None:
        raw_a = BUILDER.expected_image(VALUE_A)
        raw_b = BUILDER.expected_image(VALUE_B)
        base = BUILDER.IMAGE_BASE
        for raw, value in ((raw_a, VALUE_A), (raw_b, VALUE_B)):
            expected = value.to_bytes(8, "little")
            self.assertEqual(raw[0xBFFD - base : 0xC000 - base], expected[:3])
            self.assertEqual(raw[0xD000 - base : 0xD005 - base], expected[3:])
            self.assertEqual(raw[0xB800 - base : 0xB808 - base], expected)
        # The first-page bytes are equal, so any difference in the read value comes from page 2.
        self.assertEqual(raw_a[0xBFFD - base : 0xC000 - base], raw_b[0xBFFD - base : 0xC000 - base])
        self.assertNotEqual(raw_a[0xD000 - base : 0xD005 - base], raw_b[0xD000 - base : 0xD005 - base])
        # The data PTEs are supervisor RW NX with A and D clear.
        for entry in (0x5000, 0x5008):
            pte = le(raw_a, entry - base, 8)
            self.assertEqual(pte & 0x7FF, 0x003)
            self.assertEqual(pte >> 63, 1)

    def test_small_upper_dword_is_rejected(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            result = build(Path(temporary) / "out", "--value", "0x44332211")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("nonzero upper dword", result.stderr)

    def test_validator_rejects_frames_that_are_made_adjacent(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = (output / "cross_page_probe.elf").read_bytes()
            raw = bytearray((output / "cross_page_probe.bin").read_bytes())
            # Second data PTE points at the frame right after the first data frame.
            raw[0x5008 - BUILDER.IMAGE_BASE : 0x5010 - BUILDER.IMAGE_BASE] = (
                (0xC003 | BUILDER.NX).to_bytes(8, "little")
            )
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(raw), VALUE_A)

    def test_validator_rejects_a_code_frame_with_nx_and_a_changed_instruction(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = (output / "cross_page_probe.elf").read_bytes()
            raw = bytearray((output / "cross_page_probe.bin").read_bytes())
            raw[0x4008 - BUILDER.IMAGE_BASE : 0x4010 - BUILDER.IMAGE_BASE] = (
                (0x9001 | BUILDER.NX).to_bytes(8, "little")
            )
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(raw), VALUE_A)
            raw = bytearray((output / "cross_page_probe.bin").read_bytes())
            # Turn the qword load (48 8b 03 at 0x9001) into a dword load plus NOP in raw only.
            raw[0x9001 - BUILDER.IMAGE_BASE] = 0x90
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(raw), VALUE_A)


if __name__ == "__main__":
    unittest.main()
