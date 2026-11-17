#!/usr/bin/env python3
"""Reproducibility and validator tests for the Stage6SmpProbeV1 builder (S6-16e).

Expected values are written here by hand from the protocol and the Intel SDM, not read back from the
builder's own expectation functions.
"""

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
SCRIPT = ROOT / "tools/build/build_stage6_smp_probe.py"
RUST = ROOT / "crates/od-machine/src/stage6_smp_probe.rs"
SPEC = importlib.util.spec_from_file_location("stage6_smp_probe_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)

NONCE = "s6-16e-nonce"


def build(output: Path, *flags: str) -> None:
    subprocess.run(
        [sys.executable, str(SCRIPT), "--output-dir", str(output), *flags],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )


def rust_constants() -> dict[str, int]:
    text = RUST.read_text()
    found = re.findall(r"pub const (STAGE6_SMP_PROBE_\w+): \w+ = (0x[0-9A-Fa-f_]+|\d+);", text)
    return {name: int(value.replace("_", ""), 0) for name, value in found}


class Stage6SmpProbeBuilderTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)

    def image(self, variant: str = "normal") -> bytes:
        out = self.base / variant
        build(out, "--variant", variant)
        return (out / "probe.bin").read_bytes()

    def test_two_builds_are_byte_identical_and_match_the_manifest(self) -> None:
        outputs = [self.base / "one", self.base / "two"]
        for output in outputs:
            build(output)
        raw = [(output / "probe.bin").read_bytes() for output in outputs]
        self.assertEqual(raw[0], raw[1])
        self.assertEqual(len(raw[0]), 0xB000)
        manifests = [json.loads((output / "build_manifest.json").read_text()) for output in outputs]
        self.assertEqual(manifests[0], manifests[1])
        digest = hashlib.sha256(raw[0]).hexdigest()
        self.assertEqual(manifests[0]["artifacts"]["probe.bin"]["sha256"], digest)
        self.assertEqual(manifests[0]["profile"], "Stage6SmpProbeV1")

    def test_every_variant_is_reproducible_and_differs_from_the_others(self) -> None:
        digests = {}
        for variant in ("normal", "no_long_mode", "no_flag"):
            first, second = self.image(variant), self.image(variant)
            self.assertEqual(first, second)
            digests[variant] = hashlib.sha256(first).hexdigest()
        self.assertEqual(len(set(digests.values())), 3)

    def test_the_nonce_and_the_run_are_not_in_the_image(self) -> None:
        raw = self.image()
        info_start = 0x7000 - 0x2000
        self.assertEqual(raw[info_start : info_start + 0x1000], bytes(0x1000), "the info page is the loader's")
        data_start = 0xC000 - 0x2000
        self.assertEqual(raw[data_start : data_start + 0x1000], bytes(0x1000), "cells start at zero")
        self.assertNotIn(NONCE.encode(), raw)

    def test_fixed_addresses_match_the_rust_loader(self) -> None:
        constants = rust_constants()
        self.assertEqual(constants["STAGE6_SMP_PROBE_IMAGE_GPA"], BUILDER.IMAGE_BASE)
        self.assertEqual(constants["STAGE6_SMP_PROBE_IMAGE_BYTES"], BUILDER.IMAGE_SIZE)
        self.assertEqual(constants["STAGE6_SMP_PROBE_PML4_GPA"], BUILDER.PML4)
        self.assertEqual(constants["STAGE6_SMP_PROBE_INFO_GPA"], BUILDER.INFO)
        self.assertEqual(constants["STAGE6_SMP_PROBE_INFO_MAGIC"], BUILDER.INFO_MAGIC)
        self.assertEqual(constants["STAGE6_SMP_PROBE_INFO_VERSION"], BUILDER.INFO_VERSION)
        self.assertEqual(constants["STAGE6_SMP_PROBE_NONCE_MAX"], BUILDER.NONCE_MAX)
        self.assertEqual(constants["STAGE6_SMP_PROBE_SIPI_VECTOR"], BUILDER.SIPI_VECTOR)
        self.assertEqual(constants["STAGE6_SMP_PROBE_TRAMPOLINE_GPA"], BUILDER.REAL_AT)
        self.assertEqual(constants["STAGE6_SMP_PROBE_LOCK_GPA"], BUILDER.LOCK)
        self.assertEqual(constants["STAGE6_SMP_PROBE_FLAG_GPA"], BUILDER.FLAG)
        self.assertEqual(constants["STAGE6_SMP_PROBE_COUNT_GPA"], BUILDER.COUNT)
        self.assertEqual(constants["STAGE6_SMP_PROBE_MARKER_BSP_GPA"], BUILDER.MARKER_BSP)
        self.assertEqual(constants["STAGE6_SMP_PROBE_MARKER_AP_GPA"], BUILDER.MARKER_AP)
        self.assertEqual(constants["STAGE6_SMP_PROBE_MARKER_MAGIC"], BUILDER.MARKER_MAGIC)
        # Hand-derived: "SMP6" and "MARK" as little-endian ASCII, SIPI vector 0x08 = page 0x8000.
        self.assertEqual(BUILDER.INFO_MAGIC, int.from_bytes(b"SMP6", "little"))
        self.assertEqual(BUILDER.MARKER_MAGIC, int.from_bytes(b"MARK", "little"))
        self.assertEqual(BUILDER.REAL_AT, BUILDER.SIPI_VECTOR << 12)

    def test_icr_values_follow_the_sdm_and_the_contract(self) -> None:
        # ICR low (SDM Vol. 3A 10.6.1): vector 7:0, delivery mode 10:8 (INIT 101, start-up 110),
        # level 14 (1 = assert), trigger mode 15 (1 = level). INIT deassert: level 0, trigger level.
        self.assertEqual(BUILDER.ICR_INIT_ASSERT, 0xC500)
        self.assertEqual(BUILDER.ICR_INIT_DEASSERT, 0x8500)
        self.assertEqual(BUILDER.ICR_SIPI, 0x0608)

    def test_expected_uart_is_derived_from_the_protocol(self) -> None:
        for variant, vcpus, expected in [
            ("normal", 1, "BSP0 UP {nonce}\nSMP PROBE AP ABSENT {nonce}\n"),
            ("normal", 2, "BSP0 UP {nonce}\nAP1 UP {nonce}\nSMP PROBE OK {nonce}\n"),
            ("no_flag", 2, "BSP0 UP {nonce}\nAP1 UP {nonce}\nSMP PROBE FAIL TIMEOUT {nonce}\n"),
            ("no_flag", 1, "BSP0 UP {nonce}\nSMP PROBE AP ABSENT {nonce}\n"),
            ("no_long_mode", 2, None),
        ]:
            out = self.base / f"{variant}-expect"
            build(out, "--variant", variant)
            manifest = json.loads((out / "build_manifest.json").read_text())
            entry = manifest["expected"][f"n{vcpus}"]
            self.assertEqual(entry["uart_template"], expected, (variant, vcpus))
            path = out / f"expected_uart_n{vcpus}.txt"
            if expected is None:
                self.assertFalse(path.exists())
                self.assertEqual(entry["run_error"], "ApLegacyRefused")
            else:
                self.assertEqual(path.read_text(), expected)
                self.assertIsNone(entry["run_error"])

    def test_expected_memory_cells(self) -> None:
        out = self.base / "mem"
        build(out)
        memory = json.loads((out / "build_manifest.json").read_text())["expected"]["n2"]["memory"]
        self.assertEqual(memory["start_count"]["value"], "0x1")
        self.assertEqual(memory["flag_ap_up"]["value"], "0x1")
        self.assertEqual(memory["lock_entered"]["value"], "0x1")
        self.assertEqual(memory["marker_ap"]["apic_id_raw"], "0x1000000")
        self.assertEqual(memory["marker_bsp"]["apic_id_raw"], "0x0")
        single = json.loads((out / "build_manifest.json").read_text())["expected"]["n1"]["memory"]
        self.assertEqual(single["start_count"]["value"], "0x0")
        self.assertNotIn("marker_ap", single)

    def test_trampoline_uses_the_forms_of_s6_14a_and_the_variant_breaks_it(self) -> None:
        manifest = {}
        for variant in ("normal", "no_long_mode"):
            out = self.base / f"{variant}-code"
            build(out, "--variant", variant)
            manifest[variant] = json.loads((out / "build_manifest.json").read_text())["code"]
        real = [step["bytes"] for step in manifest["normal"]["trampoline_real"]]
        # cli; mov ax,cs; mov ds,ax; mov es,ax; mov ss,ax; lock bts [0x100],0; jae; pause; jmp;
        # mov esp; lidt; lgdt; mov dx,0x18; mov eax,0x50033; mov cr0,eax; jmp far 0x08:0x8200.
        self.assertEqual(real[0], "fa")
        self.assertEqual(real[1:5], ["8cc8", "8ed8", "8ec0", "8ed0"])
        self.assertEqual(real[5], "66f00fba2e000100")
        self.assertEqual(real[6:9], ["7304", "f390", "ebf2"])
        self.assertEqual(real[9], "66bc00f00000")
        self.assertEqual(real[10], "660f011e4001")
        self.assertEqual(real[11], "660f01163001")
        self.assertEqual(real[12:15], ["ba1800", "66b833000500", "0f22c0"])
        self.assertEqual(real[15], "66ea0082000008" + "00")
        protected = [step["bytes"] for step in manifest["normal"]["trampoline_protected"]]
        self.assertEqual(protected[:3], ["8ed2", "8eda", "8ec2"])
        self.assertEqual(protected[-1], "ea0083000010" + "00")
        self.assertIn("0f30", protected)  # WRMSR EFER
        compat = [step["bytes"] for step in manifest["normal"]["trampoline_long_entry"]]
        self.assertEqual(compat, ["ff25" + "02010000"])  # jmp [rip + 0x102]: cell 0x8408 - 0x8306
        broken = [step["bytes"] for step in manifest["no_long_mode"]["trampoline_protected"]]
        self.assertEqual(broken, ["8ed2", "8eda", "8ec2", "f4"])
        self.assertNotIn("0f22c0", broken)

    def test_long_mode_code_uses_only_the_planned_mnemonics(self) -> None:
        out = self.base / "forms"
        build(out)
        code = json.loads((out / "build_manifest.json").read_text())["code"]
        allowed = {"mov", "add", "inc", "dec", "cmp", "test", "shr", "out", "sti", "cli", "hlt", "jmp", "jz", "jnz", "jne", "jb", "ja"}
        for part in ("bsp", "ap"):
            for step in code[part]:
                self.assertIn(step["asm"].split()[0], allowed, step)
            self.assertTrue(code[part], part)
        # The AP prints before it raises the flag; the BSP prints its result after seeing the flag.
        ap = [step["asm"] for step in code["ap"]]
        flag_store = max(i for i, asm in enumerate(ap) if asm == f"mov edi, 0x{BUILDER.FLAG:x}")
        last_out = max(i for i, asm in enumerate(ap) if asm == "out dx, al")
        self.assertLess(last_out, flag_store)

    def test_no_flag_variant_has_no_flag_store(self) -> None:
        normal, no_flag = BUILDER.ap_code(BUILDER.strings_blob()[1], "normal")[0], BUILDER.ap_code(BUILDER.strings_blob()[1], "no_flag")[0]
        self.assertEqual(len(normal) - len(no_flag), 5 + 5 + 2)  # mov edi,imm32; mov eax,imm32; mov [rdi],eax

    def test_jump_range_and_overlap_errors_are_build_errors(self) -> None:
        asm = BUILDER.Asm(0x1000)
        asm.jump("jmp far", 0xEB, "far")
        asm.op("nop", bytes([0x90]) * 200)
        asm.label("far")
        with self.assertRaises(BUILDER.BuildError):
            asm.assemble()
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.build_image("unknown")

    def test_cli_refuses_an_unknown_variant(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--output-dir", str(self.base / "bad"), "--variant", "x"],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
