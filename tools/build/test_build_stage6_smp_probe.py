#!/usr/bin/env python3
"""Reproducibility and validator tests for the Stage6SmpProbeV2 builder (S6-16e, S6-16f).

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


def struct_gate(handler: int, selector: int) -> bytes:
    """A 64-bit interrupt gate written out by hand (SDM Vol. 3A 6.14.1)."""
    return (
        (handler & 0xFFFF).to_bytes(2, "little") + selector.to_bytes(2, "little") + bytes([0, 0x8E])
        + (handler >> 16).to_bytes(2, "little") + bytes(8)
    )


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
    found = re.findall(r"pub const (STAGE6_SMP_PROBE_\w+): \w+ = (0x[0-9A-Fa-f_]+|[\d_]+);", text)
    return {name: int(value.replace("_", ""), 0) for name, value in found}


class Stage6SmpProbeBuilderTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)

    def image(self, variant: str = "normal", scenario: str = "normal") -> bytes:
        out = self.base / f"{variant}-{scenario}"
        build(out, "--variant", variant, "--scenario", scenario)
        return (out / "probe.bin").read_bytes()

    def manifest(self, *flags: str) -> dict:
        out = self.base / ("m" + "".join(flag.replace("-", "") for flag in flags))
        build(out, *flags)
        return json.loads((out / "build_manifest.json").read_text())

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
        self.assertEqual(manifests[0]["profile"], "Stage6SmpProbeV2")

    def test_every_variant_is_reproducible_and_differs_from_the_others(self) -> None:
        digests = {}
        for variant in ("normal", "no_long_mode", "no_flag"):
            first, second = self.image(variant), self.image(variant)
            self.assertEqual(first, second)
            digests[variant] = hashlib.sha256(first).hexdigest()
        self.assertEqual(len(set(digests.values())), 3)

    def test_every_scenario_is_reproducible_and_differs_from_the_others(self) -> None:
        digests = {}
        for scenario in ("normal", "exchange", "resipi", "bad-ipi", "nmi"):
            first, second = self.image("normal", scenario), self.image("normal", scenario)
            self.assertEqual(first, second)
            digests[scenario] = hashlib.sha256(first).hexdigest()
            manifests = [self.manifest("--scenario", scenario) for _ in range(2)]
            self.assertEqual(manifests[0], manifests[1])
            self.assertEqual(manifests[0]["scenario"], scenario)
        self.assertEqual(len(set(digests.values())), 5)
        # K is part of the image: another K is another image, and the manifest says which.
        four = self.manifest("--scenario", "exchange", "--rounds", "4")
        self.assertEqual(four["rounds"], 4)
        self.assertNotEqual(self.image("normal", "exchange"), (self.base / "mscenarioexchangerounds4" / "probe.bin").read_bytes())
        self.assertEqual(self.manifest("--scenario", "exchange")["rounds"], 16)
        for bad in ("0", "101"):
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--output-dir", str(self.base / "badk"), "--scenario", "exchange", "--rounds", bad],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0, bad)

    def test_idt_gates_and_idtr_follow_the_sdm(self) -> None:
        raw = self.image("normal", "exchange")
        at = lambda gpa, length: raw[gpa - 0x2000 : gpa - 0x2000 + length]  # noqa: E731
        # IDTR descriptor of the trampoline (limit u16, 32-bit base): 0x3FF, 0x9800.
        self.assertEqual(at(0x8140, 6), bytes([0xFF, 0x03, 0x00, 0x98, 0x00, 0x00]))
        # The BSP's descriptor (limit u16, base u64) for `LIDT [RSP+0]`, and the three instructions
        # around it: mov esp, 0x8150; lidt [rsp+0] (0F 01 /3, ModRM 0x5C, SIB 0x24, disp8 0); mov esp, 0xE000.
        self.assertEqual(at(0x8150, 10), bytes([0xFF, 0x03, 0x00, 0x98, 0, 0, 0, 0, 0, 0]))
        bsp = [step["bytes"] for step in self.manifest("--scenario", "exchange")["code"]["bsp"]]
        index = bsp.index("0f015c2400")
        self.assertEqual(bsp[index - 1], "bc50810000")
        self.assertEqual(bsp[index + 1], "bc00e00000")
        # SDM Vol. 3A 6.14.1: 64-bit interrupt gate = offset 15:0, selector, IST, type/attr
        # (0x8E: present, DPL 0, type 0xE), offset 31:16, offset 63:32, reserved.
        for vector, handler in ((0x02, 0x9E00), (0x30, 0x9C00), (0x31, 0x9D00)):
            gate = at(0x9800 + vector * 16, 16)
            self.assertEqual(gate, struct_gate(handler, 0x10), hex(vector))
        for vector in range(0x40):
            if vector not in (2, 0x30, 0x31):
                self.assertEqual(at(0x9800 + vector * 16, 16), bytes(16), hex(vector))
        # The handler of the AP: count, EOI to 0xFEE000B0, Fixed 0x31 to APIC ID 0, IRETQ.
        ap = at(0x9C00, 0x80).hex()
        self.assertIn("41b8" + "18c00000", ap)  # mov r8d, AP_RX (0xC018)
        self.assertIn("4589" + "90b0000000", ap)  # mov [r8+0xB0], r10d: the EOI
        self.assertIn("41ba31000000", ap)  # mov r10d, Fixed | 0x31
        handlers = self.manifest("--scenario", "exchange")["code"]["handlers"]
        self.assertTrue(handlers["0x9c00"]["bytes"].endswith("48cf"))
        self.assertTrue(handlers["0x9d00"]["bytes"].endswith("48cf"))
        self.assertTrue(handlers["0x9e00"]["bytes"].endswith("48cf"))
        self.assertIn("41b820c00000", handlers["0x9d00"]["bytes"])  # BSP counter 0xC020

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
        self.assertEqual(constants["STAGE6_SMP_PROBE_AP_RX_GPA"], BUILDER.AP_RX)
        self.assertEqual(constants["STAGE6_SMP_PROBE_BSP_RX_GPA"], BUILDER.BSP_RX)
        self.assertEqual(constants["STAGE6_SMP_PROBE_NMI_GPA"], BUILDER.NMI_CELL)
        self.assertEqual(constants["STAGE6_SMP_PROBE_IDT_GPA"], BUILDER.IDT_AT)
        self.assertEqual(constants["STAGE6_SMP_PROBE_IDT_LIMIT"], BUILDER.IDT_LIMIT)
        self.assertEqual(constants["STAGE6_SMP_PROBE_IDTR_BSP_GPA"], BUILDER.IDTR_BSP_AT)
        self.assertEqual(constants["STAGE6_SMP_PROBE_VECTOR_AP"], BUILDER.VECTOR_AP)
        self.assertEqual(constants["STAGE6_SMP_PROBE_VECTOR_BSP"], BUILDER.VECTOR_BSP)
        self.assertEqual(constants["STAGE6_SMP_PROBE_MAX_FIXED_QUANTUM"], BUILDER.MAX_FIXED_QUANTUM)
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
        # Fixed: delivery mode 000, vector in 7:0, level 0 / edge, physical. NMI: delivery mode 100b
        # at bits 10:8 (vector ignored).
        self.assertEqual(BUILDER.ICR_FIXED_AP, 0x30)
        self.assertEqual(BUILDER.ICR_FIXED_BSP, 0x31)
        self.assertEqual(BUILDER.ICR_NMI, 0x400)
        # The waits: 5 BSP steps per iteration; the largest Fixed quantum has to fit with room.
        self.assertEqual(BUILDER.WAIT_UP, 0x100000)
        self.assertEqual(BUILDER.WAIT_RX, 0x100000)
        self.assertLess(BUILDER.MAX_FIXED_QUANTUM + 1_000_000, BUILDER.WAIT_UP * 5)

    def test_expected_uart_is_derived_from_the_protocol(self) -> None:
        head2 = "BSP0 UP {nonce}\nAP1 UP {nonce}\n"
        absent = "BSP0 UP {nonce}\nSMP PROBE AP ABSENT {nonce}\n"
        for flags, vcpus, expected, error in [
            (("--variant", "normal"), 1, absent, None),
            (("--variant", "normal"), 2, head2 + "SMP PROBE OK {nonce}\n", None),
            (("--variant", "no_flag"), 2, head2 + "SMP PROBE FAIL TIMEOUT {nonce}\n", None),
            (("--variant", "no_flag"), 1, absent, None),
            (("--variant", "no_long_mode"), 2, None, "ApLegacyRefused"),
            (("--scenario", "exchange"), 2, head2 + "SMP PROBE OK K=16 {nonce}\n", None),
            (("--scenario", "exchange", "--rounds", "3"), 2, head2 + "SMP PROBE OK K=3 {nonce}\n", None),
            (("--scenario", "resipi"), 2, head2 + "SMP PROBE OK K=16 {nonce}\n", None),
            (("--scenario", "bad-ipi"), 2, head2 + "SMP PROBE BAD-IPI {nonce}\n", "LapicDeviceError"),
            (("--scenario", "bad-ipi"), 1, absent, None),
            (("--scenario", "nmi"), 2, head2 + "SMP PROBE NMI OK {nonce}\n", None),
            (("--scenario", "exchange"), 1, absent, None),
        ]:
            manifest = self.manifest(*flags)
            out = self.base / ("m" + "".join(flag.replace("-", "") for flag in flags))
            entry = manifest["expected"][f"n{vcpus}"]
            self.assertEqual(entry["uart_template"], expected, (flags, vcpus))
            path = out / f"expected_uart_n{vcpus}.txt"
            if expected is None:
                self.assertFalse(path.exists())
            else:
                self.assertEqual(path.read_text(), expected)
            self.assertEqual(entry["run_error"], error, (flags, vcpus))

    def test_expected_memory_cells(self) -> None:
        out = self.base / "mem"
        build(out)
        memory = json.loads((out / "build_manifest.json").read_text())["expected"]["n2"]["memory"]
        self.assertEqual(memory["start_count"]["value"], "0x1")
        self.assertEqual(memory["flag_ap_up"]["value"], "0x1")
        self.assertEqual(memory["lock_entered"]["value"], "0x1")
        self.assertEqual(memory["marker_ap"]["apic_id_raw"], "0x1000000")
        self.assertEqual(memory["marker_bsp"]["apic_id_raw"], "0x0")
        self.assertEqual(memory["ap_rx"]["value"], "0x0")
        single = json.loads((out / "build_manifest.json").read_text())["expected"]["n1"]["memory"]
        self.assertEqual(single["start_count"]["value"], "0x0")
        self.assertNotIn("marker_ap", single)
        # By scenario: K rounds give K at both reply cells (the AP handler and the BSP handler),
        # the start counter stays 1 (also after a second SIPI), the NMI cell is 1 only for `nmi`.
        for scenario, rx, nmi in [("exchange", "0x10", "0x0"), ("resipi", "0x10", "0x0"), ("bad-ipi", "0x0", "0x0"), ("nmi", "0x0", "0x1")]:
            manifest = self.manifest("--scenario", scenario)
            n2 = manifest["expected"]["n2"]
            self.assertEqual(n2["memory"]["ap_rx"]["value"], rx, scenario)
            self.assertEqual(n2["memory"]["bsp_rx"]["value"], rx, scenario)
            self.assertEqual(n2["memory"]["nmi_cell"]["value"], nmi, scenario)
            self.assertEqual(n2["memory"]["start_count"]["value"], "0x1", scenario)
            self.assertEqual((n2["ap_init_applied"], n2["ap_started"]), (1, 1), scenario)
            self.assertEqual(n2["memory"]["ap_rx"]["gpa"], "0xc018")
            self.assertEqual(n2["memory"]["bsp_rx"]["gpa"], "0xc020")
            self.assertEqual(n2["memory"]["nmi_cell"]["gpa"], "0xc028")
        self.assertEqual(self.manifest("--scenario", "exchange", "--rounds", "5")["expected"]["n2"]["memory"]["ap_rx"]["value"], "0x5")

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
        allowed = {"mov", "add", "inc", "dec", "cmp", "test", "shr", "shl", "out", "sti", "cli", "hlt", "lidt", "jmp", "jz", "jnz", "jne", "je", "jb", "ja"}
        for scenario in ("normal", "exchange", "resipi", "bad-ipi", "nmi"):
            out_s = self.base / f"forms-{scenario}"
            build(out_s, "--scenario", scenario)
            scode = json.loads((out_s / "build_manifest.json").read_text())["code"]
            for part in ("bsp", "ap"):
                for step in scode[part]:
                    self.assertIn(step["asm"].split()[0], allowed, (scenario, step))
                self.assertTrue(scode[part], part)
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
