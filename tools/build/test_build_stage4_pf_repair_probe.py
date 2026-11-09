#!/usr/bin/env python3
"""Reproducibility and validator tests for the S4-33c #PF repair probe builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_pf_repair_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_pf_repair_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)
INITIAL = 0x1122334455667788
STORE = 0x8877665544332211
CANARY = 0x6A3591C7D248B0EF


def build(output: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *extra],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


class Stage4PfRepairProbeBuilderTest(unittest.TestCase):
    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            outputs = [Path(temporary) / "one", Path(temporary) / "two"]
            for output in outputs:
                self.assertEqual(build(output).returncode, 0)
            raw = [(output / "pf_repair_probe.bin").read_bytes() for output in outputs]
            self.assertEqual(raw[0], raw[1])
            self.assertEqual(len(raw[0]), BUILDER.IMAGE_SIZE)
            self.assertEqual(raw[0], BUILDER.expected_image(INITIAL, STORE, CANARY))
            digest = hashlib.sha256(raw[0]).hexdigest()
            for output in outputs:
                manifest = json.loads((output / "build_manifest.json").read_text())
                self.assertEqual(manifest["artifacts"]["pf_repair_probe.bin"]["sha256"], digest)
                self.assertEqual(manifest["profile"], "Stage4PfRepairProbeV1")
                self.assertEqual(manifest["layout"]["main_code"]["bytes"], "48890b488b03")
                self.assertEqual(
                    manifest["layout"]["handler_code"]["bytes"], "4889370f013b488d64240848cf"
                )
                self.assertEqual(manifest["layout"]["mappings"][0]["access"], "supervisor-ro")
            # The data page carries initial, canary and store qwords.
            for offset, value in ((0xC000, INITIAL), (0xC008, CANARY), (0xC010, STORE)):
                self.assertEqual(int.from_bytes(raw[0][offset : offset + 8], "little"), value)
            # The data PTE is read-only and supervisor; the alias PTE maps its page table RW.
            self.assertEqual(int.from_bytes(raw[0][0x5000:0x5008], "little"), 0xD001)
            self.assertEqual(int.from_bytes(raw[0][0x3018:0x3020], "little"), 0x6027)

    def test_equal_qwords_are_rejected(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            result = build(
                Path(temporary) / "out", "--initial-value", "0x5", "--store-value", "0x5"
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must differ", result.stderr)

    def test_validator_rejects_a_dropped_lea_even_when_elf_matches_raw(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = bytearray((output / "pf_repair_probe.elf").read_bytes())
            raw = bytearray((output / "pf_repair_probe.bin").read_bytes())
            _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
            handler = segments[2]
            # Turn the handler's LEA RSP,[RSP+8] (48 8d 64 24 08) into NOPs in both files.
            for index in range(5):
                elf[handler["offset"] + 6 + index] = 0x90
                raw[0x8000 - BUILDER.IMAGE_BASE + 6 + index] = 0x90
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(bytes(elf), bytes(raw), INITIAL, STORE, CANARY)

    def test_validator_rejects_a_writable_data_pte(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = (output / "pf_repair_probe.elf").read_bytes()
            raw = bytearray((output / "pf_repair_probe.bin").read_bytes())
            raw[0x6000 - BUILDER.IMAGE_BASE] = 0x03  # data PTE 0xd003: RW from the start
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(raw), INITIAL, STORE, CANARY)


if __name__ == "__main__":
    unittest.main()
