#!/usr/bin/env python3
"""Reproducibility and validator tests for the S4-27i4a paged STI/HLT probe builder."""

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
SCRIPT = ROOT / "tools/build/build_stage4_paged_sti_hlt_probe.py"
SPEC = importlib.util.spec_from_file_location("stage4_paged_sti_hlt_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


def build(output: Path, *extra: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *extra],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


def qword(raw: bytes, gpa: int) -> int:
    offset = gpa - BUILDER.IMAGE_BASE
    return int.from_bytes(raw[offset : offset + 8], "little")


class Stage4PagedStiHltProbeBuilderTest(unittest.TestCase):
    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            outputs = [Path(temporary) / "one", Path(temporary) / "two"]
            for output in outputs:
                self.assertEqual(build(output).returncode, 0)
            raw = [(output / "paged_sti_hlt_probe.bin").read_bytes() for output in outputs]
            self.assertEqual(raw[0], raw[1])
            self.assertEqual(len(raw[0]), BUILDER.IMAGE_SIZE)
            self.assertEqual(raw[0], BUILDER.expected_image())
            digest = hashlib.sha256(raw[0]).hexdigest()
            for output in outputs:
                manifest = json.loads((output / "build_manifest.json").read_text())
                self.assertEqual(manifest["artifacts"]["paged_sti_hlt_probe.bin"]["sha256"], digest)
                self.assertEqual(manifest["profile"], "Stage4PagedStiHltProbeV1")
                layout = manifest["layout"]
                self.assertEqual(layout["main_code"]["bytes"], "fbf4f4")
                self.assertEqual(layout["handler_code"]["bytes"], "890348cf")
                self.assertEqual(layout["vector"], "0x41")
                self.assertEqual(layout["idt"]["gate_vector"], 0x41)
                self.assertEqual(layout["idt"]["gate"], "0020080000" "8e4000" "00000000" "00000000")
                self.assertEqual(layout["idt"]["limit"], "0x41f")
                self.assertEqual(layout["eoi"]["physical"], "0xfee000b0")
                self.assertEqual(len(layout["mappings"]), 6)
                ptes = layout["expected_ptes"]
                self.assertEqual(ptes["lapic"]["final"], "0xfee00063")
                self.assertEqual(ptes["stack"]["final"], "0xa063")
                self.assertEqual(ptes["idt"]["bits"], "A")
            # Initial leaf PTEs are present+writable with A and D clear.
            for gpa, value in (
                (0x4008, 0x6003),
                (0x4010, 0x7003),
                (0x5000, 0x8003),
                (0x5008, 0x9003),
                (0x5010, 0xFEE00003),
                (0x5800, 0xA003),
            ):
                self.assertEqual(qword(raw[0], gpa), value)
                self.assertEqual(value & 0x60, 0)
            # The IDT gate of vector 0x41 is a present interrupt gate (type 0xe, P=1).
            gate = BUILDER.IDT_GPA + BUILDER.GATE_OFFSET - BUILDER.IMAGE_BASE
            self.assertEqual(raw[0][gate + 5], 0x8E)
            # The stack page is zero-filled.
            self.assertEqual(raw[0][0x9000:0xA000], bytes(0x1000))

    def test_validator_rejects_a_main_without_sti_or_hlt(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = bytearray((output / "paged_sti_hlt_probe.elf").read_bytes())
            raw = bytearray((output / "paged_sti_hlt_probe.bin").read_bytes())
            _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
            main = segments[1]
            for index, original in ((0, 0xFB), (1, 0xF4)):
                broken_elf, broken_raw = bytearray(elf), bytearray(raw)
                broken_elf[main["offset"] + index] = 0x90
                broken_raw[BUILDER.MAIN_CODE_GPA - BUILDER.IMAGE_BASE + index] = 0x90
                self.assertEqual(raw[BUILDER.MAIN_CODE_GPA - BUILDER.IMAGE_BASE + index], original)
                with self.assertRaises(BUILDER.BuildError):
                    BUILDER.validate_image(bytes(broken_elf), bytes(broken_raw))

    def test_expected_values_follow_the_sdm_rules_not_a_code_run(self) -> None:
        initial_rsp = 0x700FF8
        # SDM Vol. 3A 6.14.2: align RSP down to 16, then push five qwords (SS, RSP, RFLAGS, CS, RIP).
        new_rsp = (initial_rsp & ~0xF) - 5 * 8
        self.assertEqual(new_rsp, 0x700FC8)
        for name, scenario in BUILDER.SCENARIOS.items():
            self.assertEqual(int(scenario["new_rsp"], 16), new_rsp, name)
            frame = scenario["frame_low_to_high"]
            self.assertEqual(frame[2], "rflags=0x202", name)  # saved RFLAGS keeps IF
            self.assertEqual(frame[3], "rsp=0x700ff8", name)
            self.assertEqual(frame[4], "ss=0x10", name)
        # STI at IF=0 sets an interrupt shadow; HLT retires inside it and ends it. The wake saves
        # the RIP after HLT (0x401001 + 1). With IF=1 before STI there is no shadow and an entry at
        # the boundary before HLT saves the address of HLT.
        wake = BUILDER.SCENARIOS["wake_from_hlt"]
        boundary = BUILDER.SCENARIOS["boundary_before_hlt"]
        self.assertEqual(wake["frame_low_to_high"][0], "rip=0x401002")
        self.assertEqual(boundary["frame_low_to_high"][0], "rip=0x401001")
        self.assertEqual(wake["after"][0][3:5], [False, True])  # not halted, shadow active
        self.assertEqual(wake["after"][1][3:5], [True, False])  # halted, shadow ended
        self.assertEqual(boundary["after"][0][4], False)  # IF was 1: no shadow
        # Interrupt gate clears IF in the live RFLAGS while the vector is in service (ISR set).
        self.assertEqual(wake["after"][3][2], "0x2")
        self.assertTrue(wake["after"][3][6])
        # Initial leaf PTEs have A and D clear; the final A/D bits are A on use, D on a first write.
        ptes = BUILDER.EXPECTED_PTES
        self.assertEqual(ptes["stack"]["bits"], "A,D")
        self.assertEqual(ptes["lapic"]["bits"], "A,D")
        for name in ("main", "handler", "idt", "gdt"):
            self.assertEqual(ptes[name]["bits"], "A")
        # An unreadable gate (P=0) and a read-only stack differ in A bits of GDT and stack.
        idt = BUILDER.EXPECTED_INJECTIONS["idt_not_present"]["ptes"]
        stack = BUILDER.EXPECTED_INJECTIONS["stack_read_only"]["ptes"]
        self.assertEqual((idt["gdt"], idt["stack"]), ("0x9003", "0xa003"))
        self.assertEqual((stack["gdt"], stack["stack"]), ("0x9023", "0xa021"))

    def test_validator_rejects_a_missing_eoi_even_when_elf_matches_raw(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = bytearray((output / "paged_sti_hlt_probe.elf").read_bytes())
            raw = bytearray((output / "paged_sti_hlt_probe.bin").read_bytes())
            _, segments = BUILDER.parse_elf_load_segments(bytes(elf))
            handler = segments[2]
            # Turn the handler's MOV [RBX],EAX (89 03) into NOPs in both files.
            for index in range(2):
                elf[handler["offset"] + index] = 0x90
                raw[BUILDER.HANDLER_CODE_GPA - BUILDER.IMAGE_BASE + index] = 0x90
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(bytes(elf), bytes(raw))

    def test_validator_rejects_a_not_present_gate_and_a_read_only_stack(self) -> None:
        work = ROOT / "work/stage4"
        work.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=work) as temporary:
            output = Path(temporary) / "build"
            self.assertEqual(build(output).returncode, 0)
            elf = (output / "paged_sti_hlt_probe.elf").read_bytes()
            raw = (output / "paged_sti_hlt_probe.bin").read_bytes()
            gate_type = BUILDER.IDT_GPA + BUILDER.GATE_OFFSET + 5 - BUILDER.IMAGE_BASE
            broken = bytearray(raw)
            broken[gate_type] = 0x0E  # P=0
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(broken))
            broken = bytearray(raw)
            broken[0x5800 - BUILDER.IMAGE_BASE] = 0x01  # stack PTE 0xa001: read-only
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(broken))
            broken = bytearray(raw)
            broken[0x5010 - BUILDER.IMAGE_BASE + 2] = 0xEF  # APIC PTE frame no longer 0xfee00000
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.validate_image(elf, bytes(broken))


if __name__ == "__main__":
    unittest.main()
