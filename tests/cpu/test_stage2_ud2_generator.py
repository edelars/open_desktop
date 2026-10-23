"""Structural checks for the literal S2-20b UD2 exception fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_ud2.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_ud2", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class Ud2GeneratorTests(unittest.TestCase):
    def test_complete_opcode_and_literal_no_commit_answer(self) -> None:
        files = GENERATOR.materialize()
        fixture = json.loads(files["s2_20b_ud2.case.json"])
        answer = json.loads(files["s2_20b_ud2_expected.json"])
        code = next(region for region in fixture["memory"] if region["region_id"] == "code")
        self.assertEqual(code["initial_bytes_hex"], "0f0b")
        self.assertEqual(code["length"], 2)
        self.assertEqual(fixture["expected_exit"], "exception")
        self.assertEqual(answer["exception"]["vector"], 6)
        self.assertEqual(answer["exception"]["error_code"], "unavailable")
        self.assertEqual(answer["exception"]["fault_address"], "unavailable")
        self.assertEqual(answer["exception"]["faulting_rip"], answer["rip"])
        self.assertEqual(answer["gpr"], fixture["initial_state"]["gpr"])
        self.assertEqual(answer["rflags"], fixture["initial_state"]["rflags"])
        self.assertEqual(answer["memory"]["code"], code["initial_bytes_hex"])
        self.assertEqual(answer["instructions_executed"], 0)
        self.assertIn("Vol.2A UD2", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.FIXTURES / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
