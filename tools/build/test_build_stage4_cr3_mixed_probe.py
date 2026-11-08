#!/usr/bin/env python3
"""Reproducibility and validator tests for the mixed-page CR3 probe builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_cr3_mixed_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_cr3_mixed_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


class Stage4Cr3MixedProbeBuilderTest(unittest.TestCase):
    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            outputs = [Path(temporary) / "one", Path(temporary) / "two"]
            for output in outputs:
                subprocess.run(
                    [sys.executable, str(SCRIPT), "--output-dir", str(output)],
                    cwd=ROOT,
                    check=True,
                    capture_output=True,
                    text=True,
                )
            raw = [(output / "probe.bin").read_bytes() for output in outputs]
            self.assertEqual(raw[0], raw[1])
            self.assertEqual(len(raw[0]), BUILDER.IMAGE_SIZE)
            self.assertEqual(raw[0], BUILDER.expected_image())
            manifests = [
                json.loads((output / "build_manifest.json").read_text()) for output in outputs
            ]
            digest = hashlib.sha256(raw[0]).hexdigest()
            self.assertEqual([m["raw_sha256"] for m in manifests], [digest, digest])
            self.assertEqual(manifests[0]["profile"], "Stage4Cr3MixedProbeV1")
            self.assertEqual((manifests[0]["root_a"], manifests[0]["root_b"]), ("0x1000", "0x5000"))

    def test_validator_rejects_a_substituted_instruction_even_when_elf_matches_raw(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            subprocess.run(
                [sys.executable, str(SCRIPT), "--output-dir", str(output)],
                cwd=ROOT,
                check=True,
                capture_output=True,
                text=True,
            )
            elf = bytearray((output / "probe.elf").read_bytes())
            raw = bytearray((output / "probe.bin").read_bytes())
            _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
            code_b = segments[2]
            # Turn `MOV CR3,RAX` in the root-B code into `MOV CR3,RCX` in both files.
            elf[code_b["offset"] + 8] = 0xD9
            raw[0x200000 - BUILDER.IMAGE_BASE + 8] = 0xD9
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(bytes(elf), bytes(raw))


if __name__ == "__main__":
    unittest.main()
