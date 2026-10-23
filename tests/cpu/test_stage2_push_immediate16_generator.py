from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "generate_stage2_push_immediate16",
    ROOT / "tests/cpu/generate_stage2_push_immediate16.py",
)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PushImmediate16GeneratorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = MODULE.materialize()
        cls.answers = json.loads(cls.files["s2_17h_push_immediate16_expected.json"])

    def test_imm8_sign_extension_and_imm16_literal_boundaries(self) -> None:
        expected = {
            "s2_17h_push_imm8_zero": ("666a00", 0x0000),
            "s2_17h_push_imm8_max": ("666a7f", 0x007f),
            "s2_17h_push_imm8_min": ("666a80", 0xff80),
            "s2_17h_push_imm8_minus_one": ("666aff", 0xffff),
            "s2_17h_push_imm16_zero": ("66680000", 0x0000),
            "s2_17h_push_imm16_max": ("6668ff7f", 0x7fff),
            "s2_17h_push_imm16_min": ("66680080", 0x8000),
            "s2_17h_push_imm16_minus_one": ("6668ffff", 0xffff),
        }
        for case_id, (encoding, value) in expected.items():
            case = json.loads(self.files[f"{case_id}.case.json"])
            code = next(region for region in case["memory"] if region["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding)
            answer = self.answers[case_id]
            self.assertEqual(answer["gpr"][4], "0x00000000003000fe")
            stack = bytes.fromhex(answer["memory"]["stack"])
            self.assertEqual(stack[-2:], value.to_bytes(2, "little"))

    def test_stack_boundary_and_truncated_fetch_are_literal_no_commit(self) -> None:
        for case_id, linear in [
            ("s2_17h_push_fault_bottom", 0x002f_fffe),
            ("s2_17h_push_truncated_imm8", 0x0010_0002),
            ("s2_17h_push_truncated_imm16", 0x0010_0003),
        ]:
            answer = self.answers[case_id]
            self.assertEqual(answer["exception"]["vector"], 14)
            self.assertEqual(answer["exception"]["linear_address"], f"0x{linear:016x}")
            self.assertEqual(answer["instructions_executed"], 0)
            self.assertEqual(answer["rip"], "0x0000000000100000")
        self.assertEqual(len(self.answers), 11)


if __name__ == "__main__":
    unittest.main()
