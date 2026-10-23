"""Structural and freshness checks for the literal S2-21b XADD fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_xadd_qword_register.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_xadd_qword_register", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class QwordRegisterXaddGeneratorTests(unittest.TestCase):
    def test_literal_alias_encoding_and_expected_result_inputs(self) -> None:
        files = GENERATOR.materialize()
        fixture = json.loads(files["s2_21b_xadd_qword_register_alias.case.json"])
        code = fixture["memory"][0]
        self.assertEqual(code["initial_bytes_hex"], "480fc1c0")
        self.assertEqual(fixture["expected_exit"], "stop_address")
        self.assertEqual(fixture["instruction_budget"], 1)
        self.assertEqual(fixture["initial_state"]["gpr"][0], "0x7fffffffffffffff")
        self.assertEqual(fixture["initial_state"]["rflags"], "0x0000000000000cd7")
        self.assertIn("same RAX register", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.FIXTURES / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
