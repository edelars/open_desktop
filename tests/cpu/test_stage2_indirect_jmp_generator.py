#!/usr/bin/env python3
"""Literal materialization checks for the S2-16e memory JMP corpus."""

import json
import unittest

import generate_stage2_indirect_jmp_memory as generator


class IndirectMemoryJumpGeneratorTests(unittest.TestCase):
    def setUp(self):
        self.files = generator.materialize()
        self.answers = json.loads(self.files["s2_16e_indirect_jmp_memory_expected.json"])

    def case(self, case_id):
        return json.loads(self.files[f"{case_id}.case.json"])

    def test_generates_each_supported_effective_address_encoding(self):
        expected = {
            "s2_16e_jmp_mem_base": "ff20",
            "s2_16e_jmp_mem_rex_base": "41ff20",
            "s2_16e_jmp_mem_sib_disp": "ff648810",
            "s2_16e_jmp_mem_rip_relative": "ff2500000000",
            "s2_16e_jmp_mem_addr32": "67ff20",
        }
        self.assertEqual(len(expected), 5)
        for case_id, prefix in expected.items():
            with self.subTest(case_id=case_id):
                case = self.case(case_id)
                code = next(r["initial_bytes_hex"] for r in case["memory"] if r["region_id"] == "code")
                self.assertTrue(code.startswith(prefix))
                self.assertEqual(case["expected_exit"], "stop_address")
                self.assertEqual(self.answers[case_id]["instructions_executed"], 2)

    def test_memory_source_faults_have_literal_unchanged_state_answers(self):
        for case_id, vector, error_code in (
            ("s2_16e_jmp_mem_read_fault", 14, 5),
            ("s2_16e_jmp_mem_noncanonical", 13, 0),
        ):
            with self.subTest(case_id=case_id):
                case = self.case(case_id)
                answer = self.answers[case_id]
                self.assertEqual(case["expected_exit"], "exception")
                self.assertEqual(answer["instructions_executed"], 0)
                self.assertEqual(answer["exit"], "exception")
                self.assertEqual(answer["exception"]["vector"], vector)
                self.assertEqual(answer["exception"]["error_code"], error_code)
                self.assertEqual(answer["rip"], case["initial_state"]["rip"])
                self.assertEqual(answer["gpr"], case["initial_state"]["gpr"])
                if case_id == "s2_16e_jmp_mem_read_fault":
                    data = next(r for r in case["memory"] if r["region_id"] == "data")
                    self.assertFalse(data["permissions"]["read"])


if __name__ == "__main__":
    unittest.main()
