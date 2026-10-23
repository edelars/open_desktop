from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "generate_stage2_push_pop16",
    ROOT / "tests/cpu/generate_stage2_push_pop16.py",
)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PushPop16GeneratorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = MODULE.materialize()
        cls.answers = json.loads(cls.files["s2_17g_push_pop16_expected.json"])

    def test_word_forms_extended_registers_and_stack_alias_ordering(self) -> None:
        expected_bytes = {
            "s2_17g_push_ax": "6650",
            "s2_17g_pop_ax": "6658",
            "s2_17g_push_r8w": "664150",
            "s2_17g_pop_r9w": "664159",
            "s2_17g_push_sp": "6654",
            "s2_17g_pop_sp": "665c",
            "s2_17g_pop_sp_carry": "665c",
        }
        for case_id, encoding in expected_bytes.items():
            case = json.loads(self.files[f"{case_id}.case.json"])
            code = next(region for region in case["memory"] if region["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding)

        push_sp = self.answers["s2_17g_push_sp"]
        self.assertEqual(push_sp["gpr"][4], "0x00000000003000fe")
        stack = bytes.fromhex(push_sp["memory"]["stack"])
        self.assertEqual(stack[-2:], b"\x00\x01")  # Old SP, before decrement.

        pop_sp = self.answers["s2_17g_pop_sp"]
        self.assertEqual(pop_sp["gpr"][4], "0x000000000030beef")
        pop_sp_carry = self.answers["s2_17g_pop_sp_carry"]
        self.assertEqual(pop_sp_carry["gpr"][4], "0x0000000000312468")

    def test_stack_faults_and_truncated_prefix_are_literal_no_commit(self) -> None:
        for case_id, linear in [
            ("s2_17g_push_fault_bottom", 0x002F_FFFE),
            ("s2_17g_pop_fault_top", 0x0030_0100),
            ("s2_17g_truncated_fetch", 0x0010_0001),
        ]:
            answer = self.answers[case_id]
            self.assertEqual(answer["exception"]["vector"], 14)
            self.assertEqual(answer["exception"]["linear_address"], f"0x{linear:016x}")
            self.assertEqual(answer["instructions_executed"], 0)
            self.assertEqual(answer["rip"], "0x0000000000100000")
        self.assertEqual(len(self.answers), 10)


if __name__ == "__main__":
    unittest.main()
