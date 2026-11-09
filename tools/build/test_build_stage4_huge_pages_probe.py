#!/usr/bin/env python3
"""Reproducibility and validator tests for the S4-33e huge-page probe builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_huge_pages_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_huge_pages_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)
START = BUILDER.DEFAULT_START
MIDDLE = BUILDER.DEFAULT_MIDDLE
END = BUILDER.DEFAULT_END


def build(output: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *extra],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


def le(raw: bytes, gpa: int, length: int = 8) -> int:
    offset = gpa - BUILDER.IMAGE_BASE
    return int.from_bytes(raw[offset : offset + length], "little")


class Stage4HugePagesProbeBuilderTest(unittest.TestCase):
    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            outputs = [Path(temporary) / "one", Path(temporary) / "two"]
            for output in outputs:
                self.assertEqual(build(output).returncode, 0)
            raw = [(output / "huge_pages_probe.bin").read_bytes() for output in outputs]
            self.assertEqual(raw[0], raw[1])
            self.assertEqual(len(raw[0]), BUILDER.IMAGE_SIZE)
            self.assertEqual(raw[0], BUILDER.expected_image(START, MIDDLE, END))
            digest = hashlib.sha256(raw[0]).hexdigest()
            for output in outputs:
                manifest = json.loads((output / "build_manifest.json").read_text())
                self.assertEqual(manifest["artifacts"]["huge_pages_probe.bin"]["sha256"], digest)
                self.assertEqual(manifest["profile"], "Stage4HugePagesProbeV1")
                self.assertEqual(manifest["start_value_u64"], f"0x{START:016x}")
                self.assertEqual(manifest["middle_value_u64"], f"0x{MIDDLE:016x}")
                self.assertEqual(manifest["end_value_u64"], f"0x{END:016x}")
                layout = manifest["layout"]
                self.assertEqual(layout["code"]["bytes"], "488b03488b03488b0348890b488b03eef4")
                maps = {m["gva"]: m for m in layout["mappings"]}
                # The 2 MiB mapping is a PS=1 leaf at a 2 MiB-aligned frame (SDM Table 4-18).
                self.assertEqual(maps["0x800000"]["entry"], "0x8000000000200083")
                self.assertEqual(maps["0x800000"]["size"], "2 MiB")
                self.assertEqual(int(maps["0x800000"]["gpa"], 16) % 0x200000, 0)
                self.assertEqual(maps["0x8000000000"]["ps"], 1)
                self.assertEqual(layout["negative_1gib"]["pfec_cpl0_read"], "0x09")

    def test_values_land_at_start_middle_and_last_qword_of_the_2_mib_frame(self) -> None:
        raw = BUILDER.expected_image(START, MIDDLE, END)
        base = 0x200000
        # GPA = frame base + page offset (SDM Vol. 3A 4.5: bits 20:0 are the offset in a 2 MiB page).
        self.assertEqual(le(raw, base + 0x0), START)
        self.assertEqual(le(raw, base + 0x100000), MIDDLE)
        self.assertEqual(le(raw, base + 0x1FFFF8), END)
        self.assertEqual(le(raw, base + 0x1FFF00), BUILDER.STORE_DECOY)
        self.assertEqual(le(raw, base + 0x200000), BUILDER.SENTINEL)
        self.assertEqual(len({START, MIDDLE, END}), 3)
        # Page tables: PML4E[1] reaches PDPT 1 whose entry 0 has PS=1 (bit 7); PDE[4] has PS=1.
        self.assertEqual(le(raw, 0x1008), 0x5003)
        self.assertEqual(le(raw, 0x5000) & (1 << 7), 1 << 7)
        pde = le(raw, 0x3020)
        self.assertEqual(pde & 0xFF, 0x83)
        self.assertEqual(pde & 0x60, 0)  # A and D clear
        self.assertEqual(pde & 0x1FE000, 0)  # bits 20:13 reserved
        self.assertEqual(pde >> 63, 1)

    def test_equal_or_colliding_values_are_rejected(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            same = build(Path(temporary) / "a", "--middle-value", hex(START))
            self.assertNotEqual(same.returncode, 0)
            self.assertIn("pairwise different", same.stderr)
            decoy = build(Path(temporary) / "b", "--end-value", hex(BUILDER.SENTINEL))
            self.assertNotEqual(decoy.returncode, 0)
            self.assertIn("decoy, sentinel", decoy.stderr)

    def test_validator_rejects_a_pde_without_ps_and_a_changed_instruction(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = (output / "huge_pages_probe.elf").read_bytes()
            good = (output / "huge_pages_probe.bin").read_bytes()
            raw = bytearray(good)
            # PDE[4] turns into a 4 KiB table pointer: not the fixed profile.
            raw[0x3020 - BUILDER.IMAGE_BASE : 0x3028 - BUILDER.IMAGE_BASE] = (
                (0x6003 | BUILDER.NX).to_bytes(8, "little")
            )
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(raw), START, MIDDLE, END)
            raw = bytearray(good)
            # PDPT 1 entry loses PS: the negative would not be a 1 GiB leaf any more.
            raw[0x5000 - BUILDER.IMAGE_BASE : 0x5008 - BUILDER.IMAGE_BASE] = (0x3003).to_bytes(8, "little")
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(raw), START, MIDDLE, END)
            raw = bytearray(good)
            raw[0x7000 - BUILDER.IMAGE_BASE] = 0x90
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(raw), START, MIDDLE, END)


if __name__ == "__main__":
    unittest.main()
