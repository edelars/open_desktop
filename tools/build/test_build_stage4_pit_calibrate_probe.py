#!/usr/bin/env python3
"""Reproducibility, validator and reference-model tests for the Stage4PitCalibrateProbeV1 builder."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/build_stage4_pit_calibrate_probe.py"
RUST_TEST = ROOT / "crates/od-machine/tests/stage4_pit_calibrate_probe.rs"
SPEC = importlib.util.spec_from_file_location("stage4_pit_calibrate_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


def build(output: Path, *flags: str) -> None:
    subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *flags],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )


def rust_hex_constant(name: str) -> str:
    """The hex string of a `const NAME: &str = concat!(...)` / literal in the Rust test."""
    source = RUST_TEST.read_text()
    match = re.search(rf"const {name}: &str = (concat!\((.*?)\)|\"(.*?)\");", source, re.S)
    assert match, name
    body = match.group(2) or match.group(3)
    return "".join(re.findall(r'"([0-9a-f]*)"', body)) if match.group(2) else body


class Stage4PitCalibrateProbeBuilderTest(unittest.TestCase):
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
        self.assertEqual(manifest["artifacts"]["probe.bin"]["sha256"], hashlib.sha256(raw[0]).hexdigest())
        self.assertEqual(manifest["profile"], "Stage4PitCalibrateProbeV1")
        self.assertTrue(manifest["accepted_profile"])
        self.assertEqual(manifest["layout"]["entry_gpa"], "0x4000")
        code, _ = BUILDER.expected_code(False)
        self.assertEqual(manifest["code_hex"], code.hex())
        offset = 0x4000 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][offset : offset + len(code)], code)

    def test_manifest_expected_values_come_from_the_reference_model(self) -> None:
        output = self.base / "build"
        build(output)
        manifest = json.loads((output / "build_manifest.json").read_text())
        self.assertEqual(manifest["expected"], BUILDER.reference_model())
        self.assertEqual(manifest["model"]["port_tick_ns"], 1000)
        self.assertEqual(manifest["model"]["pit_input_hz"], 1_193_182)

    def test_reference_model_matches_hand_derived_values(self) -> None:
        """Closed forms from floor(t * 1193182 / 1e9) and the 1 us port tick."""
        expected = BUILDER.reference_model()
        a, b = expected["a"], expected["b"]
        # The count is loaded after the first read of 0x61: t = 1000 ns, T0 = 1.
        self.assertEqual(a["load_ns"], 1000)
        self.assertEqual(a["start_pit_tick"], 1000 * 1_193_182 // 1_000_000_000)
        self.assertEqual(a["start_pit_tick"], 1)
        # First loop read j (t = 1000 j) that sees OUT2: floor(1.193182 j) - 1 >= 11931.
        j = next(j for j in range(1, 100_000) if (1000 * j * 1_193_182) // 1_000_000_000 - 1 >= 11_931)
        self.assertEqual(j, 10_001)
        self.assertEqual(a["pitcnt"], j - 1)
        self.assertEqual(a["delta_tsc"], 1000 * a["pitcnt"])
        self.assertEqual((a["tscmin"], a["tscmax"]), (1000, 1000))
        self.assertEqual(a["freq_khz"], a["delta_tsc"] // 10)
        self.assertEqual(a["freq_khz"], 1_000_000)
        self.assertEqual(a["port_b_reads"], 1 + j)  # gate read + loop reads incl. the last
        # MSB stages: 256 ticks each, so about 214.5 us of 1 us passes.
        self.assertEqual((b["stage1_passes"], b["stage2_passes"]), (214, 213))
        self.assertEqual(b["freq_khz"], b["delta_tsc"] * 1_193_182 // 256_000)
        self.assertEqual(expected["port_b_reads_total"], a["port_b_reads"] + b["port_b_reads"])
        self.assertEqual(expected["clock_ns"], 1000 * expected["port_b_reads_total"])
        self.assertEqual(len(expected["serial_bytes"]), 77)

    def test_coarse_tick_model_trips_the_guest_thresholds(self) -> None:
        expected = BUILDER.reference_model(100_000)
        self.assertLess(expected["a"]["pitcnt"], BUILDER.CAL_LOOPMIN)
        self.assertLessEqual(expected["b"]["stage1_passes"], BUILDER.QUICK_MIN_PASSES)
        serial = bytes(expected["serial_bytes"])
        self.assertEqual((serial[1], serial[43]), (1, 3))

    def test_rust_test_pins_the_builder_code_and_serial_records(self) -> None:
        code, _ = BUILDER.expected_code(False)
        self.assertEqual(rust_hex_constant("GUEST_CODE_HEX"), code.hex())
        self.assertEqual(rust_hex_constant("EXPECTED_SERIAL_HEX"), BUILDER.reference_model()["serial_hex"])
        self.assertEqual(rust_hex_constant("COARSE_SERIAL_HEX"), BUILDER.reference_model(100_000)["serial_hex"])

    def test_omit_gate_variant_drops_only_the_gate_sequence_and_is_unaccepted(self) -> None:
        accepted = self.base / "accepted"
        omitted = self.base / "omitted"
        build(accepted)
        build(omitted, "--omit-gate")
        manifest = json.loads((omitted / "build_manifest.json").read_text())
        self.assertFalse(manifest["accepted_profile"])
        self.assertEqual(manifest["variant"], {"omit_gate": True})
        full, _ = BUILDER.expected_code(False)
        short, _ = BUILDER.expected_code(True)
        gate = BUILDER.gate_high()
        self.assertEqual(len(gate), 11)
        self.assertEqual(full[32 : 32 + 11], gate)
        self.assertEqual(full[:32] + full[43:], short)
        self.assertNotEqual((accepted / "probe.bin").read_bytes(), (omitted / "probe.bin").read_bytes())

    def test_validator_rejects_substituted_code_even_when_elf_matches_raw(self) -> None:
        output = self.base / "build"
        build(output)
        elf = bytearray((output / "probe.elf").read_bytes())
        raw = bytearray((output / "probe.bin").read_bytes())
        BUILDER.validate_image(bytes(elf), bytes(raw), False)
        _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
        code = segments[1]
        # Change the latch count immediate of the first PIT load (0x9b) to 0x9c.
        position = bytes(raw).index(bytes([0xB0, 0x9B, 0xEE]), 0x4000 - BUILDER.IMAGE_BASE) + 1
        elf[code["offset"] + position - (0x4000 - BUILDER.IMAGE_BASE)] = 0x9C
        raw[position] = 0x9C
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.validate_image(bytes(elf), bytes(raw), False)

    def test_validator_rejects_a_stray_byte_and_a_variant_mismatch(self) -> None:
        output = self.base / "build"
        build(output)
        elf = (output / "probe.elf").read_bytes()
        raw = bytearray((output / "probe.bin").read_bytes())
        for gpa, value in ((0x3000, 1), (0x4FFF, 1), (0x2008, 0x00)):
            damaged = bytearray(raw)
            damaged[gpa - BUILDER.IMAGE_BASE] = value
            with self.assertRaises(BUILDER.BuildError, msg=hex(gpa)):
                BUILDER.validate_image(elf, bytes(damaged), False)
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.validate_image(elf, bytes(raw), True)


if __name__ == "__main__":
    unittest.main()
