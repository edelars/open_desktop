#!/usr/bin/env python3
"""Reproducibility and validator tests for the S4-33b CPL3 syscall probe builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_cpl3_syscall_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_cpl3_syscall_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)
INITIAL = 0x1122334455667788
STORE = 0x8877665544332211


def build(output: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *extra],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


class Stage4Cpl3SyscallProbeBuilderTest(unittest.TestCase):
    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            outputs = [Path(temporary) / "one", Path(temporary) / "two"]
            for output in outputs:
                self.assertEqual(build(output).returncode, 0)
            raw = [(output / "cpl3_syscall_probe.bin").read_bytes() for output in outputs]
            self.assertEqual(raw[0], raw[1])
            self.assertEqual(len(raw[0]), BUILDER.IMAGE_SIZE)
            self.assertEqual(raw[0], BUILDER.expected_image(INITIAL, STORE))
            manifests = [
                json.loads((output / "build_manifest.json").read_text()) for output in outputs
            ]
            digest = hashlib.sha256(raw[0]).hexdigest()
            for manifest in manifests:
                self.assertEqual(manifest["artifacts"]["cpl3_syscall_probe.bin"]["sha256"], digest)
                self.assertEqual(manifest["profile"], "Stage4Cpl3SyscallProbeV1")
                self.assertEqual(manifest["layout"]["user_code"]["bytes"], "0f05488b03")
                self.assertEqual(manifest["layout"]["kernel_code"]["bytes"], "4c8917480f07")
            # The two data qwords follow the requested values.
            self.assertEqual(int.from_bytes(raw[0][0x8000:0x8008], "little"), INITIAL)
            self.assertEqual(int.from_bytes(raw[0][0x8008:0x8010], "little"), STORE)

    def test_equal_qwords_are_rejected(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            result = build(
                Path(temporary) / "out", "--initial-value", "0x5", "--store-value", "0x5"
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must differ", result.stderr)

    def test_validator_rejects_a_substituted_instruction_even_when_elf_matches_raw(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = bytearray((output / "cpl3_syscall_probe.elf").read_bytes())
            raw = bytearray((output / "cpl3_syscall_probe.bin").read_bytes())
            _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
            kernel = segments[2]
            # Turn kernel `MOV [RDI],R10` into `MOV [RDI],R11` in both files.
            elf[kernel["offset"]] = 0x4D
            raw[0x8000 - BUILDER.IMAGE_BASE] = 0x4D
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(bytes(elf), bytes(raw), INITIAL, STORE)


if __name__ == "__main__":
    unittest.main()
