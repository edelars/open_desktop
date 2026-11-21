"""Unit tests of the ROM mode helpers of s7_31a_efi_stub_isa_inventory.py (no objdump, no probe)."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import s7_31a_efi_stub_isa_inventory as tool  # noqa: E402


class DemangleTests(unittest.TestCase):
    def test_v0_path(self) -> None:
        name = "_RNvNtNtCseWy8E3VS3N2_11od_firmware2pe6loader9load_into"
        self.assertEqual(tool.demangle(name), "od_firmware::pe::loader::load_into")

    def test_legacy_path_drops_the_hash(self) -> None:
        name = "_ZN11od_firmware2pe6loader9load_into17h0123456789abcdefE"
        self.assertEqual(tool.demangle(name), "od_firmware::pe::loader::load_into")

    def test_plain_symbol_is_unchanged(self) -> None:
        self.assertEqual(tool.demangle("od_fw_main"), "od_fw_main")


class CalibrationTests(unittest.TestCase):
    def insn(self, address: int, data: bytes) -> dict:
        return {"addr": address, "bytes": data, "asm": "and qword ptr [rsp + 0x8], -0x2"}

    def test_found_only_inside_load_into(self) -> None:
        insns = [self.insn(0x10, tool.CALIBRATION_BYTES), self.insn(0x80, tool.CALIBRATION_BYTES)]
        names = {0x10: "od_firmware::pe::loader::load_into", 0x80: "od_firmware::pe::loader::launch"}
        verdicts = {tool.CALIBRATION_BYTES: ("unsupported", "Unsupported")}
        result = tool.calibrate(insns, names.get, verdicts)
        self.assertTrue(result["ok"])
        self.assertEqual(result["sites_total"], 1)
        self.assertEqual(result["firmware_profile_verdict"], "unsupported")

    def test_missing_form_fails_the_calibration(self) -> None:
        insns = [self.insn(0x10, bytes.fromhex("90"))]
        result = tool.calibrate(insns, lambda address: "od_firmware::pe::loader::load_into", {})
        self.assertFalse(result["ok"])


class FunctionLookupTests(unittest.TestCase):
    def test_locate_by_range(self) -> None:
        ranges = [(0x100, 0x140, "a"), (0x140, 0x200, "b")]
        starts = [r[0] for r in ranges]
        self.assertEqual(tool.locate_function(ranges, starts, 0x13F), "a")
        self.assertEqual(tool.locate_function(ranges, starts, 0x140), "b")
        self.assertIsNone(tool.locate_function(ranges, starts, 0x200))
        self.assertIsNone(tool.locate_function(ranges, starts, 0x10))


if __name__ == "__main__":
    unittest.main()
