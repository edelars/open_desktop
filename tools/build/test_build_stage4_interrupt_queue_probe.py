#!/usr/bin/env python3
"""Reproducibility and validator tests for the Stage4InterruptQueueProbeV1 builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_interrupt_queue_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_interrupt_queue_probe_builder", SCRIPT)
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


class Stage4InterruptQueueProbeBuilderTest(unittest.TestCase):
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
        self.assertEqual(manifest["profile"], "Stage4InterruptQueueProbeV1")
        self.assertEqual(manifest["schema"], "stage4-interrupt-queue-probe-build-v1")
        self.assertTrue(manifest["accepted_profile"])
        self.assertEqual(manifest["layout"]["entry_gpa"], "0x4000")
        self.assertEqual(manifest["layout"]["vector"], "0x62")
        # The prologue starts with MOV EDI,0xFEE00000 at GPA 0x4000.
        offset = 0x4000 - BUILDER.IMAGE_BASE
        self.assertEqual(raw[0][offset : offset + 5], bytes.fromhex("bf0000e0fe"))
        # The IDT gate of vector 0x62 points to the handler (offset 0x4014).
        gate = 0x3000 - BUILDER.IMAGE_BASE + 0x62 * 16
        self.assertEqual(raw[0][gate : gate + 2], (0x4014).to_bytes(2, "little"))

    def test_expected_values_follow_the_queue_rules_and_frame_arithmetic(self) -> None:
        evidence = BUILDER.expected_evidence()
        # Prologue: 5 (MOV EDI) + 11 (SVR write) = 16 bytes, then STI, then HLT; the
        # entry is 16 + STI + HLT + JMP rel8 = 20 bytes, so the handler is at 0x4014.
        self.assertEqual(evidence["hlt"], f"0x{0x4000 + 16 + 1:x}")
        self.assertEqual(evidence["handler"], f"0x{0x4000 + 20:x}")
        # Schedule: E1 is due now, F at 250000 ns; P1/P2 are due at the current time.
        self.assertEqual(
            evidence["schedule"]["phase1_apic_disabled"],
            [{"key": 1, "due_ns": 0}, {"key": 2, "due_ns": 250_000}],
        )
        self.assertEqual(evidence["phase1"]["queue_len"], 2)
        self.assertEqual(evidence["phase1"]["retired"], 0)
        # Wakes: E1 at 0 ns, F at its deadline, the merged pair at the current time.
        self.assertEqual(evidence["wake_times_ns"], [0, 250_000, 250_000])
        self.assertEqual(evidence["wakeups"], 3)
        self.assertEqual(evidence["counter"], 3)
        self.assertEqual(evidence["time_ns"], 250_000)
        self.assertEqual(evidence["coalesced"], 1)  # P1 and P2 make one IRR edge
        self.assertEqual(evidence["queue_len"], 0)
        self.assertEqual(evidence["frame"]["address"], "0x6fc8")  # 0x6ff0 - 5 * 8
        self.assertEqual(evidence["frame"]["rip"], f"0x{0x4000 + 16 + 2:x}")  # RIP after HLT
        self.assertEqual(evidence["frame"]["rflags"], "0x202")
        self.assertEqual(evidence["handler_rflags"], "0x2")  # IF cleared by the gate
        names = [event["event"] for event in evidence["events"]]
        self.assertEqual(names.count("entry"), 3)
        self.assertEqual(names.count("coalesced"), 1)
        self.assertEqual(names.count("irr"), 3)  # E1, F and the merged pair: one rise each
        self.assertEqual(names[:7], ["irr", "sti", "hlt", "entry", "eoi", "iretq", "hlt"])
        self.assertEqual(evidence["final"]["vector"]["0x62"], [False, False])
        # The injection loses E1: one wake less (two events remain, the pair merges).
        injected = evidence["injection_pop_before_request"]
        self.assertEqual((injected["wakeups"], injected["counter"]), (2, 2))
        self.assertEqual(injected["phase1_queue_len"], 1)

    def test_validator_rejects_substituted_code_even_when_elf_matches_raw(self) -> None:
        output = self.base / "build"
        build(output)
        elf = bytearray((output / "probe.elf").read_bytes())
        raw = bytearray((output / "probe.bin").read_bytes())
        BUILDER.validate_image(bytes(elf), bytes(raw))
        _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
        code = segments[2]
        handler, _ = BUILDER.elf_symbol(bytes(elf), "stage4_queue_handler")
        # Turn the EOI store `MOV [RDI+0xB0],EAX` into offset 0xB1.
        eoi = handler - 0x4000 + len(BUILDER.handler_code()) - 2 - 4
        elf[code["offset"] + eoi] = 0xB1
        raw[0x4000 - BUILDER.IMAGE_BASE + eoi] = 0xB1
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.validate_image(bytes(elf), bytes(raw))

    def test_validator_rejects_an_extra_gate_or_stray_byte(self) -> None:
        output = self.base / "build"
        build(output)
        elf = (output / "probe.elf").read_bytes()
        raw = bytearray((output / "probe.bin").read_bytes())
        for gpa, value in ((0x3000, 1), (0x4800, 1), (0x4FFF, 1), (0x1000, 1), (0x2008, 0x00)):
            damaged = bytearray(raw)
            damaged[gpa - BUILDER.IMAGE_BASE] = value
            with self.assertRaises(BUILDER.BuildError, msg=hex(gpa)):
                BUILDER.validate_image(elf, bytes(damaged))


if __name__ == "__main__":
    unittest.main()
