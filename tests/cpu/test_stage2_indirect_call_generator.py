#!/usr/bin/env python3
"""Literal materialization checks for S2-17d register near indirect CALL."""

import json
import unittest

import generate_stage2_indirect_call as generator


class IndirectCallGeneratorTests(unittest.TestCase):
    def setUp(self):
        self.files = generator.materialize()
        self.answers = json.loads(self.files["s2_17d_indirect_call_expected.json"])

    def case(self, case_id):
        return json.loads(self.files[f"{case_id}.case.json"])

    def code(self, case_id):
        return next(r["initial_bytes_hex"] for r in self.case(case_id)["memory"] if r["region_id"] == "code")

    def test_register_forms_and_targets_are_literal(self):
        self.assertTrue(self.code("s2_17d_call_rax").startswith("ffd0"))
        self.assertTrue(self.code("s2_17d_call_r8").startswith("41ffd0"))
        self.assertEqual(self.answers["s2_17d_call_rax"]["instructions_executed"], 2)
        self.assertEqual(self.answers["s2_17d_call_r8"]["instructions_executed"], 2)
        self.assertEqual(self.answers["s2_17d_call_budget"]["exit"], "instruction_budget_exhausted")

    def test_target_and_stack_faults_have_literal_no_commit_answers(self):
        for case_id, vector in (
            ("s2_17d_call_noncanonical", 13),
            ("s2_17d_call_stack_fault", 12),
        ):
            with self.subTest(case_id=case_id):
                case = self.case(case_id)
                answer = self.answers[case_id]
                self.assertEqual(case["expected_exit"], "exception")
                self.assertEqual(answer["exit"], "exception")
                self.assertEqual(answer["instructions_executed"], 0)
                self.assertEqual(answer["gpr"], case["initial_state"]["gpr"])
                self.assertEqual(answer["rip"], case["initial_state"]["rip"])
                self.assertEqual(answer["exception"]["vector"], vector)
                self.assertEqual(answer["exception"]["error_code"], 0)


if __name__ == "__main__":
    unittest.main()
