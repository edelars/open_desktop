#!/usr/bin/env python3
"""Literal materialization checks for S2-17c near CALL and RET."""

import json
import unittest

import generate_stage2_call_ret as generator


class CallRetGeneratorTests(unittest.TestCase):
    def setUp(self):
        self.files = generator.materialize()
        self.answers = json.loads(self.files["s2_17c_call_ret_expected.json"])

    def case(self, case_id):
        return json.loads(self.files[f"{case_id}.case.json"])

    def code(self, case_id):
        return next(r["initial_bytes_hex"] for r in self.case(case_id)["memory"] if r["region_id"] == "code")

    def test_materializes_signed_call_forms_nested_returns_and_budget_boundary(self):
        self.assertEqual(self.code("s2_17c_call_forward")[:4], "e805")
        self.assertTrue(self.code("s2_17c_call_nested").startswith("e805000000"))
        self.assertTrue(self.code("s2_17c_call_negative_rel32").endswith("e8f6ffffff"))
        nested = self.answers["s2_17c_call_nested"]
        self.assertEqual(nested["instructions_executed"], 4)
        self.assertEqual(nested["rip"], "0x0000000000100005")
        partial = self.answers["s2_17c_call_budget"]
        self.assertEqual(partial["exit"], "instruction_budget_exhausted")
        self.assertEqual(partial["instructions_executed"], 2)
        self.assertEqual(partial["rip"], "0x0000000000100014")

    def test_canonical_target_faults_have_literal_unchanged_state_answers(self):
        for case_id in ("s2_17c_call_noncanonical", "s2_17c_ret_noncanonical"):
            with self.subTest(case_id=case_id):
                case = self.case(case_id)
                answer = self.answers[case_id]
                self.assertEqual(case["expected_exit"], "exception")
                self.assertEqual(answer["exit"], "exception")
                self.assertEqual(answer["instructions_executed"], 0)
                self.assertEqual(answer["gpr"], case["initial_state"]["gpr"])
                self.assertEqual(answer["rip"], case["initial_state"]["rip"])
                self.assertEqual(answer["exception"]["vector"], 13)
                self.assertEqual(answer["exception"]["error_code"], 0)


if __name__ == "__main__":
    unittest.main()
