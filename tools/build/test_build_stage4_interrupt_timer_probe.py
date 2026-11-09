#!/usr/bin/env python3
"""Reproducibility and validator tests for the Stage4InterruptTimerProbeV1 builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_interrupt_timer_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_interrupt_timer_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


def namespace(**overrides: object) -> argparse.Namespace:
    values = {"scenario": "periodic", "wakeups": 5}
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


class Stage4InterruptTimerProbeBuilderTest(unittest.TestCase):
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
        self.assertEqual(manifest["profile"], "Stage4InterruptTimerProbeV1")
        self.assertTrue(manifest["accepted_profile"])
        self.assertEqual(manifest["layout"]["entry_gpa"], "0x4000")
        self.assertEqual(manifest["layout"]["vector"], "0x61")
        # The prologue starts with MOV EDI,0xFEE00000 at GPA 0x4000.
        offset = 0x4000 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][offset : offset + 5], bytes.fromhex("bf0000e0fe"))

    def test_divide_configuration_follows_the_sdm_table(self) -> None:
        # SDM Vol. 3A 10.5.4: bits 3, 1, 0 select the divisor.
        expected = {0x0: 2, 0x1: 4, 0x2: 8, 0x3: 16, 0x8: 32, 0x9: 64, 0xA: 128, 0xB: 1}
        for config, value in expected.items():
            self.assertEqual(BUILDER.divisor(config), value, hex(config))

    def test_expected_values_follow_the_timer_formula_and_frame_arithmetic(self) -> None:
        evidence = BUILDER.expected_evidence("periodic", 5)
        # 625 ticks * 16 / 100 MHz = 100 us = 100000 ns; N * P after N wake-ups.
        self.assertEqual(evidence["timer"]["divisor"], 16)
        self.assertEqual(evidence["timer"]["period_ns"], 100_000)
        self.assertEqual(evidence["wakeups"], 5)
        self.assertEqual(evidence["counter"], 5)
        self.assertEqual(evidence["wake_times_ns"], [100_000, 200_000, 300_000, 400_000, 500_000])
        self.assertEqual(evidence["time_ns"], 500_000)
        # Prologue: 5 (MOV EDI) + 4 * 11 (four register writes) = 49 bytes, then STI, then HLT.
        self.assertEqual(evidence["hlt"], f"0x{0x4000 + 49 + 1:x}")
        self.assertEqual(evidence["handler"], f"0x{0x4000 + 49 + 4:x}")
        self.assertEqual(evidence["frame"]["address"], "0x6fc8")  # 0x6ff0 - 5 * 8
        self.assertEqual(evidence["frame"]["rip"], f"0x{0x4000 + 49 + 2:x}")  # RIP after HLT
        self.assertEqual(evidence["frame"]["rflags"], "0x202")
        self.assertEqual(evidence["handler_rflags"], "0x2")  # IF cleared by the gate
        names = [event["event"] for event in evidence["events"]]
        self.assertEqual(names[:2], ["sti", "hlt"])
        self.assertEqual(names[2:7], ["irr", "entry", "eoi", "iretq", "hlt"])
        self.assertEqual(names.count("entry"), 5)
        self.assertEqual(evidence["final"]["vector"]["0x61"], [False, False])
        unarmed = BUILDER.expected_evidence("unarmed", 5)
        self.assertEqual((unarmed["wakeups"], unarmed["counter"], unarmed["time_ns"]), (0, 0, 0))

    def test_wakeups_variant_scales_the_expected_time(self) -> None:
        self.assertEqual(BUILDER.expected_evidence("periodic", 3)["time_ns"], 300_000)
        output = self.base / "three"
        build(output, "--wakeups", "3")
        manifest = json.loads((output / "build_manifest.json").read_text())
        self.assertEqual(manifest["variant"]["wakeups"], 3)
        self.assertEqual(manifest["expected"]["time_ns"], 300_000)
        accepted = self.base / "five"
        build(accepted)
        self.assertNotEqual(
            (output / "probe.bin").read_bytes(), (accepted / "probe.bin").read_bytes()
        )

    def test_scenarios_build_and_differ(self) -> None:
        images = {}
        for scenario in BUILDER.SCENARIOS:
            output = self.base / scenario
            build(output, "--scenario", scenario)
            manifest = json.loads((output / "build_manifest.json").read_text())
            self.assertEqual(manifest["variant"]["scenario"], scenario)
            images[scenario] = (output / "probe.bin").read_bytes()
        self.assertEqual(len(set(images.values())), len(images))

    def test_out_of_range_wakeups_are_rejected(self) -> None:
        for value in ("0", "128"):
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--output-dir", str(self.base / value), "--wakeups", value],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 1, value)

    def test_validator_rejects_substituted_code_even_when_elf_matches_raw(self) -> None:
        output = self.base / "build"
        build(output)
        elf = bytearray((output / "probe.elf").read_bytes())
        raw = bytearray((output / "probe.bin").read_bytes())
        BUILDER.validate_image(bytes(elf), bytes(raw), namespace())
        _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
        code = segments[2]
        handler, _ = BUILDER.elf_symbol(bytes(elf), "stage4_timer_handler")
        # Turn the EOI store `MOV [RDI+0xB0],EAX` into offset 0xB1.
        eoi = handler - 0x4000 + len(BUILDER.handler_code(5)) - 2 - 4
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
        for override in ({"wakeups": 4}, {"scenario": "unarmed"}):
            with self.assertRaises(BUILDER.BuildError, msg=str(override)):
                BUILDER.validate_image(elf, raw, namespace(**override))


if __name__ == "__main__":
    unittest.main()
