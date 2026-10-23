"""Structural checks for the literal S2-20c qword IDIV #DE fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_idiv_qword_error.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_idiv_qword_error", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class IdivQwordErrorGeneratorTests(unittest.TestCase):
    def test_qword_overflow_and_literal_no_commit_answer(self) -> None:
        files = GENERATOR.materialize()
        fixture = json.loads(files["s2_20c_idiv_qword_quotient_overflow.case.json"])
        answer = json.loads(files["s2_20c_idiv_qword_quotient_overflow_expected.json"])
        code = next(region for region in fixture["memory"] if region["region_id"] == "code")
        self.assertEqual(code["initial_bytes_hex"], "48f7f9")
        self.assertEqual(fixture["initial_state"]["gpr"][0], "0x0000000000000000")
        self.assertEqual(fixture["initial_state"]["gpr"][2], "0x8000000000000000")
        self.assertEqual(fixture["initial_state"]["gpr"][1], "0xffffffffffffffff")
        self.assertEqual(fixture["expected_exit"], "exception")
        self.assertEqual(answer["exception"]["vector"], 0)
        self.assertEqual(answer["exception"]["error_code"], "unavailable")
        self.assertEqual(answer["exception"]["fault_address"], "unavailable")
        self.assertEqual(answer["exception"]["faulting_rip"], answer["rip"])
        self.assertEqual(answer["gpr"], fixture["initial_state"]["gpr"])
        self.assertEqual(answer["rflags"], fixture["initial_state"]["rflags"])
        self.assertEqual(answer["memory"]["code"], code["initial_bytes_hex"])
        self.assertEqual(answer["instructions_executed"], 0)
        self.assertIn("signed 128-bit minimum divided by -1", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.FIXTURES / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
