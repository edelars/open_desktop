from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "generate_stage2_ret_imm16",
    ROOT / "tests/cpu/generate_stage2_ret_imm16.py",
)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RetImm16GeneratorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = MODULE.materialize()
        cls.answers = json.loads(cls.files["s2_17f_ret_imm16_expected.json"])

    def test_zero_small_max_and_nested_cleanup_answers(self) -> None:
        for case_id, immediate, rsp in [
            ("s2_17f_ret_imm0", 0, 0x00300080),
            ("s2_17f_ret_imm_small", 0x20, 0x00300070),
            ("s2_17f_ret_imm_max", 0xffff, 0x00300008),
        ]:
            case = json.loads(self.files[f"{case_id}.case.json"])
            code = bytes.fromhex(case["memory"][0]["initial_bytes_hex"])
            self.assertEqual(code, bytes((0xc2, immediate & 0xff, immediate >> 8)))
            self.assertEqual(
                self.answers[case_id]["gpr"][4],
                f"0x{rsp + 8 + immediate:016x}",
            )
        nested = self.answers["s2_17f_ret_imm_nested_cleanup"]
        self.assertEqual(nested["rip"], "0x0000000000100019")
        self.assertEqual(nested["gpr"][4], "0x0000000000300090")
        self.assertEqual(nested["instructions_executed"], 5)

    def test_truncation_read_stack_and_target_faults_are_literal_no_commit(self) -> None:
        for case_id, vector in [
            ("s2_17f_ret_imm_noncanonical", 13),
            ("s2_17f_ret_imm_stack_fault", 12),
            ("s2_17f_ret_imm_read_fault", 14),
            ("s2_17f_ret_imm_truncated", 14),
        ]:
            answer = self.answers[case_id]
            self.assertEqual(answer["exception"]["vector"], vector, case_id)
            self.assertEqual(answer["instructions_executed"], 0, case_id)
            self.assertEqual(answer["rip"], "0x0000000000100000", case_id)
        self.assertEqual(self.answers["s2_17f_ret_imm_budget"]["exit"], "instruction_budget_exhausted")
        self.assertEqual(len(self.answers), 9)


if __name__ == "__main__":
    unittest.main()
