#!/usr/bin/env python3
"""Reproducibility and validator tests for the Stage4PitIrq0ProbeV1 builder (S4-29f)."""

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
SCRIPT = ROOT / "tools/build/build_stage4_pit_irq0_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_pit_irq0_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)

PIT_HZ = 1_193_182


def build(output: Path, *flags: str) -> None:
    subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *flags],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )


def args(interrupts: int = 3, control: int = 0x34):
    class Args:
        pass

    value = Args()
    value.interrupts = interrupts
    value.control = control
    return value


class Stage4PitIrq0ProbeBuilderTest(unittest.TestCase):
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
        self.assertEqual(manifest["profile"], "Stage4PitIrq0ProbeV1")
        self.assertEqual(manifest["schema"], "stage4-pit-irq0-probe-build-v1")
        self.assertTrue(manifest["accepted_profile"])
        self.assertEqual(manifest["layout"]["entry_gpa"], "0x4000")
        self.assertEqual(manifest["layout"]["vector"], "0x60")
        # The prologue starts with MOV EDI,0xFEE00000 at GPA 0x4000.
        offset = 0x4000 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][offset : offset + 5], bytes.fromhex("bf0000e0fe"))
        # The IDT gate of vector 0x60 points to the handler (entry is 0x80 bytes).
        gate = 0x3000 - BUILDER.IMAGE_BASE + 0x60 * 16
        self.assertEqual(raw[0][gate : gate + 2], (0x4080).to_bytes(2, "little"))
        # The injection patch sites hold the profile's own bytes (the Rust loader checks them).
        self.assertEqual(manifest["layout"]["unmask_value_gpa"], "0x4044")
        self.assertEqual(manifest["layout"]["eoi_store_gpa"], "0x40b1")
        site = 0x4044 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][site : site + 5], bytes.fromhex("b860000000"))
        site = 0x40B1 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][site : site + 6], bytes.fromhex("8987b0000000"))

    def test_mode0_variant_builds_and_changes_only_the_control_immediates(self) -> None:
        default, mode0 = self.base / "mode2", self.base / "mode0"
        build(default)
        build(mode0, "--control", "0x30", "--interrupts", "1")
        one = (default / "probe.bin").read_bytes()
        two = (mode0 / "probe.bin").read_bytes()
        self.assertNotEqual(one, two)
        manifest = json.loads((mode0 / "build_manifest.json").read_text())
        self.assertFalse(manifest["accepted_profile"])
        self.assertEqual(manifest["expected"]["wakeups"], 1)
        self.assertEqual(manifest["expected"]["time_ns"], 999_848)
        # Same instruction shape: the entry-2 write, the handler and the patch sites stay put.
        self.assertEqual(manifest["layout"]["handler_gpa"], "0x4080")
        self.assertEqual(manifest["layout"]["eoi_store_gpa"], "0x40b1")

    def test_expected_values_follow_the_pit_formula_and_the_bridge_rules(self) -> None:
        evidence = BUILDER.expected_evidence(3, 0x34)
        # Edge at tick E is due at ceil(E * 1e9 / 1193182); with N = 1193 the periods
        # are 999_847.47 ns long, so the deadlines are not multiples of one period.
        self.assertEqual(evidence["pit"]["edge_ticks"], [1193, 2386, 3579])
        self.assertEqual(evidence["wake_times_ns"], [999_848, 1_999_695, 2_999_543])
        self.assertEqual(evidence["time_ns"], 2_999_543)
        self.assertEqual(evidence["period_ns"], 999_848)
        self.assertEqual(evidence["pit"]["first_program_would_be_due_ns"], 1_500_191)
        for k, wake in enumerate(evidence["wake_times_ns"], start=1):
            ticks = wake * PIT_HZ // 1_000_000_000  # floor(t * f / 1e9)
            self.assertGreaterEqual(ticks, 1193 * k)
            self.assertLess((wake - 1) * PIT_HZ // 1_000_000_000, 1193 * k)
        self.assertEqual((evidence["wakeups"], evidence["counter"]), (3, 3))
        self.assertEqual(
            evidence["bridge"],
            {"edges": 3, "scheduled": 5, "generation_changes": 5, "cancelled": 2},
        )
        # Layout: 0x80-byte entry, HLT 3 bytes before its end (HLT, JMP rel8).
        self.assertEqual(evidence["handler"], "0x4080")
        self.assertEqual(evidence["hlt"], "0x407d")
        self.assertEqual(evidence["frame"]["address"], "0x6fc8")  # 0x6ff0 - 5 * 8
        self.assertEqual(evidence["frame"]["rip"], "0x407e")  # RIP after HLT
        self.assertEqual(evidence["frame"]["rflags"], "0x202")
        self.assertEqual(evidence["handler_rflags"], "0x2")  # IF cleared by the gate
        names = [event["event"] for event in evidence["events"]]
        self.assertEqual(names.count("entry"), 3)
        self.assertEqual(names.count("irr"), 3)
        self.assertEqual(names[:7], ["sti", "hlt", "irr", "entry", "eoi", "iretq", "hlt"])
        self.assertEqual(evidence["pic"]["pic_irr_read"], 0)
        self.assertEqual(evidence["final"]["vector"]["0x60"], [False, False])
        # Mode 0: one edge at tick N, no chain; the shutdown drops nothing pending.
        mode0 = BUILDER.expected_evidence(1, 0x30)
        self.assertEqual(mode0["pit"]["edge_ticks"], [1193])
        self.assertEqual((mode0["wakeups"], mode0["time_ns"]), (1, 999_848))
        self.assertEqual(
            mode0["bridge"],
            {"edges": 1, "scheduled": 2, "generation_changes": 5, "cancelled": 1},
        )

    def test_instruction_counts_and_injection_evidence(self) -> None:
        # 42 instructions up to and including STI; the HLT is the 43rd step.
        self.assertEqual(BUILDER.INSTRUCTIONS_TO_STI, 42)
        self.assertEqual(BUILDER.STEPS_TO_HLT, 43)
        # The entry is 128 bytes: 16 prologue + 40 PIC + 20 IOAPIC + 48 PIT + 4 loop.
        self.assertEqual(len(BUILDER.entry_code(0x34)), 16 + 40 + 20 + 48 + 4)
        self.assertEqual(BUILDER.unmask_gpa(), 0x4044)
        self.assertEqual(BUILDER.eoi_gpa(0x34), 0x40B1)
        evidence = BUILDER.injection_evidence(200)
        masked = evidence["masked_entry2"]
        # Step 43 + m has the clock at d_m: budget 200 -> m = 157.
        self.assertEqual(masked["time_ns"], -(-157 * 1193 * 10**9 // PIT_HZ))
        self.assertEqual((masked["stop"], masked["wakeups"], masked["counter"]), ("budget", 0, 0))
        noeoi = evidence["no_eoi"]
        self.assertEqual(noeoi["time_ns"], -(-144 * 1193 * 10**9 // PIT_HZ))  # d_(200 - 56)
        self.assertEqual((noeoi["wakeups"], noeoi["counter"]), (1, 1))
        nogen = evidence["no_generation"]
        self.assertEqual(nogen["wake_times_ns"], [999_848, 1_500_191, 1_999_695, 2_999_543])
        self.assertEqual((nogen["wakeups"], nogen["counter"]), (4, 4))
        # All three differ from the accepted run in (stop, wakeups, counter, time).
        accepted = ("idle_halt", 3, 3, 2_999_543)
        for name, record in (("masked", masked), ("no_eoi", noeoi), ("no_generation", nogen)):
            self.assertNotEqual(
                (record["stop"], record["wakeups"], record["counter"], record["time_ns"]),
                accepted,
                name,
            )

    def test_validator_rejects_substituted_code_even_when_elf_matches_raw(self) -> None:
        output = self.base / "build"
        build(output)
        elf = bytearray((output / "probe.elf").read_bytes())
        raw = bytearray((output / "probe.bin").read_bytes())
        BUILDER.validate_image(bytes(elf), bytes(raw), args())
        _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
        code = segments[2]
        # Turn the EOI store `MOV [RDI+0xB0],EAX` into offset 0xB1.
        eoi = BUILDER.eoi_gpa(0x34) - 0x4000 + 2
        elf[code["offset"] + eoi] = 0xB1
        raw[0x4000 - BUILDER.IMAGE_BASE + eoi] = 0xB1
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.validate_image(bytes(elf), bytes(raw), args())

    def test_validator_rejects_an_extra_gate_or_stray_byte(self) -> None:
        output = self.base / "build"
        build(output)
        elf = (output / "probe.elf").read_bytes()
        raw = bytearray((output / "probe.bin").read_bytes())
        for gpa, value in ((0x3000, 1), (0x4800, 1), (0x4FFF, 1), (0x1000, 1), (0x2008, 0x00)):
            damaged = bytearray(raw)
            damaged[gpa - BUILDER.IMAGE_BASE] = value
            with self.assertRaises(BUILDER.BuildError, msg=hex(gpa)):
                BUILDER.validate_image(elf, bytes(damaged), args())


if __name__ == "__main__":
    unittest.main()
