#!/usr/bin/env python3
"""Reproducibility and validator tests for the Stage4InterruptPumpProbeV1 builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_interrupt_pump_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_interrupt_pump_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


def namespace(**overrides: object) -> argparse.Namespace:
    values = {"scenario": "sti_hlt", "omit_eoi": False}
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


class Stage4InterruptPumpProbeBuilderTest(unittest.TestCase):
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
        self.assertEqual(manifest["profile"], "Stage4InterruptPumpProbeV1")
        self.assertTrue(manifest["accepted_profile"])
        self.assertEqual(manifest["layout"]["entry_gpa"], "0x4000")
        self.assertEqual(manifest["layout"]["vector_a"], "0x51")
        self.assertEqual(manifest["layout"]["vector_b"], "0x41")
        # The prologue starts with MOV EDI,0xFEE00000 at GPA 0x4000.
        offset = 0x4000 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][offset : offset + 5], bytes.fromhex("bf0000e0fe"))

    def test_expected_values_follow_the_sdm_frame_arithmetic(self) -> None:
        # Five qwords below an aligned stack top; the pushed RFLAGS has IF set.
        wake = BUILDER.expected_evidence("sti_hlt", BUILDER.VECTOR_B, False)
        self.assertEqual(wake["hlt"], "0x4011")
        self.assertEqual(wake["frames"][0]["address"], "0x6fc8")
        self.assertEqual(wake["frames"][0]["rip"], "0x4012")  # RIP after HLT
        self.assertEqual(wake["frames"][0]["rflags"], "0x202")
        self.assertEqual(wake["handler_rflags"], "0x2")  # IF cleared by the gate
        boundary = BUILDER.expected_evidence("if1_hlt", BUILDER.VECTOR_B, False)
        self.assertEqual(boundary["frames"][0]["rip"], boundary["hlt"])  # RIP at HLT
        cli = BUILDER.expected_evidence("cli_hlt", BUILDER.VECTOR_B, False)
        self.assertEqual(cli["final"]["vectors"]["0x41"], [True, False])
        self.assertEqual([e["event"] for e in cli["events"]], ["irr", "cli", "hlt"])
        nested = BUILDER.expected_evidence("sti_hlt", BUILDER.VECTOR_A, False)
        names = [(e["event"], e.get("vector")) for e in nested["events"]]
        self.assertLess(names.index(("eoi", 0x51)), names.index(("entry", 0x41)))
        self.assertLess(names.index(("iretq", None)), names.index(("entry", 0x41)))
        no_eoi = BUILDER.expected_evidence("sti_hlt", BUILDER.VECTOR_A, True)
        self.assertEqual(no_eoi["final"]["vectors"]["0x41"], [True, False])
        self.assertEqual(no_eoi["final"]["vectors"]["0x51"], [False, True])
        self.assertNotIn(("entry", 0x41), [(e["event"], e.get("vector")) for e in no_eoi["events"]])

    def test_every_scenario_builds_and_differs(self) -> None:
        images = {}
        for scenario in BUILDER.SCENARIOS:
            output = self.base / scenario
            build(output, "--scenario", scenario)
            manifest = json.loads((output / "build_manifest.json").read_text())
            self.assertTrue(manifest["accepted_profile"], scenario)
            self.assertEqual(manifest["variant"]["scenario"], scenario)
            images[scenario] = (output / "probe.bin").read_bytes()
        self.assertEqual(len(set(images.values())), len(images))

    def test_omit_eoi_variant_is_marked_unaccepted(self) -> None:
        accepted = self.base / "accepted"
        build(accepted)
        output = self.base / "noeoi"
        build(output, "--omit-eoi")
        manifest = json.loads((output / "build_manifest.json").read_text())
        self.assertFalse(manifest["accepted_profile"])
        self.assertTrue(manifest["variant"]["omit_eoi"])
        self.assertNotEqual((output / "probe.bin").read_bytes(), (accepted / "probe.bin").read_bytes())

    def test_validator_rejects_substituted_code_even_when_elf_matches_raw(self) -> None:
        output = self.base / "build"
        build(output)
        elf = bytearray((output / "probe.elf").read_bytes())
        raw = bytearray((output / "probe.bin").read_bytes())
        BUILDER.validate_image(bytes(elf), bytes(raw), namespace())
        _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
        code = segments[2]
        handler_b, _ = BUILDER.elf_symbol(bytes(elf), "stage4_handler_b")
        # Turn the EOI store `MOV [RDI+0xB0],EAX` of handler B into offset 0xB1.
        eoi = handler_b - 0x4000 + len(BUILDER.handler_b(False)) - 2 - 4
        elf[code["offset"] + eoi] = 0xB1
        raw[0x4000 - BUILDER.IMAGE_BASE + eoi] = 0xB1
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.validate_image(bytes(elf), bytes(raw), namespace())

    def test_validator_rejects_an_extra_gate_or_stray_byte(self) -> None:
        output = self.base / "build"
        build(output)
        elf = (output / "probe.elf").read_bytes()
        raw = bytearray((output / "probe.bin").read_bytes())
        for gpa, value in ((0x3000, 1), (0x4800, 1), (0x4FFF, 1), (0x1000, 1), (0x2008, 0x00)):
            damaged = bytearray(raw)
            damaged[gpa - BUILDER.IMAGE_BASE] = value
            with self.assertRaises(BUILDER.BuildError, msg=hex(gpa)):
                BUILDER.validate_image(elf, bytes(damaged), namespace())

    def test_validator_rejects_a_variant_mismatch(self) -> None:
        output = self.base / "build"
        build(output)
        elf = (output / "probe.elf").read_bytes()
        raw = (output / "probe.bin").read_bytes()
        for override in ({"omit_eoi": True}, {"scenario": "if1_hlt"}):
            with self.assertRaises(BUILDER.BuildError, msg=str(override)):
                BUILDER.validate_image(elf, raw, namespace(**override))


if __name__ == "__main__":
    unittest.main()
