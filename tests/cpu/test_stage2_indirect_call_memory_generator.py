from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "generate_stage2_indirect_call_memory",
    ROOT / "tests/cpu/generate_stage2_indirect_call_memory.py",
)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class IndirectCallMemoryGeneratorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = MODULE.materialize()
        cls.answers = __import__("json").loads(
            cls.files["s2_17e_indirect_call_memory_expected.json"]
        )

    def code(self, case_id: str) -> bytes:
        import json

        obj = json.loads(self.files[f"{case_id}.case.json"])
        return bytes.fromhex(next(r for r in obj["memory"] if r["region_id"] == "code")["initial_bytes_hex"])

    def test_supported_ea_forms_and_budget_are_literal(self) -> None:
        expected = {
            "s2_17e_call_mem_base": "ff10",
            "s2_17e_call_mem_rex_base": "41ff10",
            "s2_17e_call_mem_sib": "ff548810",
            "s2_17e_call_mem_rip_relative": "ff1500000000",
            "s2_17e_call_mem_addr32": "67ff10",
        }
        for case_id, prefix in expected.items():
            self.assertTrue(self.code(case_id).startswith(bytes.fromhex(prefix)), case_id)
            self.assertEqual(self.answers[case_id]["instructions_executed"], 3, case_id)
        self.assertEqual(self.answers["s2_17e_call_mem_budget"]["exit"], "instruction_budget_exhausted")
        self.assertEqual(self.answers["s2_17e_call_mem_budget"]["instructions_executed"], 1)

    def test_source_target_and_stack_faults_have_literal_no_commit_answers(self) -> None:
        for case_id, vector in [
            ("s2_17e_call_mem_read_fault", 14),
            ("s2_17e_call_mem_noncanonical", 13),
            ("s2_17e_call_mem_stack_fault", 12),
        ]:
            answer = self.answers[case_id]
            self.assertEqual(answer["exception"]["vector"], vector, case_id)
            self.assertEqual(answer["instructions_executed"], 0, case_id)
            self.assertEqual(answer["rip"], "0x0000000000100000", case_id)
        self.assertEqual(len(self.answers), 9)


if __name__ == "__main__":
    unittest.main()
