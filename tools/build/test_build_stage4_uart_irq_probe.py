#!/usr/bin/env python3
"""Reproducibility and validator tests for the Stage4UartIrqProbeV1 builder."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/build_stage4_uart_irq_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_uart_irq_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


def namespace(**overrides: object) -> argparse.Namespace:
    values = {"ier": 1, "late_unmask": False, "omit_eoi": False, "omit_rbr_read": False}
    values.update(overrides)
    return argparse.Namespace(**values)


def build(output: Path, *flags: str) -> None:
    subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *flags],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )


class Stage4UartIrqProbeBuilderTest(unittest.TestCase):
    def setUp(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(dir=work)
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)

    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        outputs = [self.base / "one", self.base / "two"]
        for output in outputs:
            build(output)
        raw = [(output / "probe.bin").read_bytes() for output in outputs]
        self.assertEqual(raw[0], raw[1])
        self.assertEqual(len(raw[0]), BUILDER.IMAGE_SIZE)
        manifest = json.loads((outputs[0] / "build_manifest.json").read_text())
        digest = hashlib.sha256(raw[0]).hexdigest()
        self.assertEqual(manifest["artifacts"]["probe.bin"]["sha256"], digest)
        self.assertEqual(manifest["profile"], "Stage4UartIrqProbeV1")
        self.assertTrue(manifest["accepted_profile"])
        self.assertEqual(manifest["layout"]["entry_gpa"], "0x4000")
        self.assertEqual(manifest["layout"]["vector"], "0x51")
        # The entry code starts with LIDT [RSP+0] at GPA 0x4000.
        offset = 0x4000 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][offset : offset + 5], bytes.fromhex("0f015c2400"))

    def test_variants_change_only_the_selected_code_and_are_marked_unaccepted(self) -> None:
        accepted = self.base / "accepted"
        build(accepted)
        reference = (accepted / "probe.bin").read_bytes()
        for name, flags, field in (
            ("noeoi", ["--omit-eoi"], "omit_eoi"),
            ("nobr", ["--omit-rbr-read"], "omit_rbr_read"),
            ("late", ["--late-unmask"], "late_unmask"),
            ("ier0", ["--ier", "0"], "ier"),
        ):
            output = self.base / name
            build(output, *flags)
            manifest = json.loads((output / "build_manifest.json").read_text())
            self.assertFalse(manifest["accepted_profile"], name)
            self.assertNotEqual((output / "probe.bin").read_bytes(), reference, name)
            self.assertIn(field, manifest["variant"])

    def test_validator_rejects_substituted_code_even_when_elf_matches_raw(self) -> None:
        output = self.base / "build"
        build(output)
        elf = bytearray((output / "probe.elf").read_bytes())
        raw = bytearray((output / "probe.bin").read_bytes())
        BUILDER.validate_image(bytes(elf), bytes(raw), namespace())
        _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
        code = segments[2]
        handler, _ = BUILDER.elf_symbol(bytes(elf), "stage4_irq_handler")
        # Turn the EOI store `MOV [RDI+0xB0],EAX` into a store to offset 0xB1.
        eoi = handler - 0x4000 + len(BUILDER.expected_handler(False, False)) - 2 - 4
        elf[code["offset"] + eoi] = 0xB1
        raw[0x4000 - BUILDER.IMAGE_BASE + eoi] = 0xB1
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.validate_image(bytes(elf), bytes(raw), namespace())

    def test_validator_rejects_an_extra_gate_or_stray_byte(self) -> None:
        output = self.base / "build"
        build(output)
        elf = (output / "probe.elf").read_bytes()
        raw = bytearray((output / "probe.bin").read_bytes())
        for gpa, value in ((0x3000, 1), (0x5000, 1), (0x6000, 1), (0x2008, 0x00)):
            damaged = bytearray(raw)
            damaged[gpa - BUILDER.IMAGE_BASE] = value
            with self.assertRaises(BUILDER.BuildError, msg=hex(gpa)):
                BUILDER.validate_image(elf, bytes(damaged), namespace())

    def test_validator_rejects_a_variant_mismatch(self) -> None:
        output = self.base / "build"
        build(output)
        elf = (output / "probe.elf").read_bytes()
        raw = (output / "probe.bin").read_bytes()
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.validate_image(elf, raw, namespace(omit_eoi=True))


if __name__ == "__main__":
    unittest.main()
