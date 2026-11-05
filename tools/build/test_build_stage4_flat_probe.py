#!/usr/bin/env python3
"""Negative validator test for the Stage 4 flat-probe builder."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/build_stage4_flat_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_flat_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


class Stage4FlatProbeBuilderTest(unittest.TestCase):
    def test_validator_rejects_opcode_substitution_even_when_elf_matches_raw(self) -> None:
        output_dir = ROOT / "work/stage4/test-builder-opcode-reject"
        if output_dir.exists():
            for path in output_dir.iterdir():
                if path.is_file():
                    path.unlink()
        output_dir.mkdir(parents=True, exist_ok=True)
        try:
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--payload",
                    "0x1122334455667788",
                    "--output-dir",
                    "work/stage4/test-builder-opcode-reject",
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            elf_path = output_dir / "flat_probe.elf"
            raw_path = output_dir / "flat_probe.bin"
            elf = bytearray(elf_path.read_bytes())
            raw = bytearray(raw_path.read_bytes())
            _, segments = BUILDER.parse_elf_load_segments(elf)
            code_segment = segments[1]
            elf[code_segment["offset"] + 3] = 0x89
            raw[0x6003] = 0x89
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(
                    bytes(elf),
                    bytes(raw),
                    "flat-probe",
                    0x1122334455667788,
                    0,
                    0,
                )
        finally:
            for path in output_dir.iterdir():
                if path.is_file():
                    path.unlink()
            output_dir.rmdir()


if __name__ == "__main__":
    unittest.main()
