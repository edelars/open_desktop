"""Structural checks for literal S2-17t near RET prefix fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_ret66.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_ret66", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class Ret66GeneratorTests(unittest.TestCase):
    def test_literal_encodings_use_qword_return_slot_and_preserve_other_state(self) -> None:
        files = GENERATOR.materialize()
        for case_id, code_hex, target, _form in GENERATOR.CASES:
            fixture = json.loads(files[f"{case_id}.case.json"])
            answer = json.loads(files[f"{case_id}_expected.json"])[case_id]
            code = next(region for region in fixture["memory"] if region["region_id"] == "code")
            stack = next(region for region in fixture["memory"] if region["region_id"] == "stack")
            self.assertEqual(code["initial_bytes_hex"], code_hex)
            self.assertEqual(fixture["expected_exit"], "stop_address")
            self.assertEqual(answer["instructions_executed"], 1)
            self.assertEqual(answer["rip"], f"0x{target:016x}")
            self.assertEqual(answer["gpr"][4], f"0x{GENERATOR.RSP + 8:016x}")
            self.assertEqual(answer["rflags"], fixture["initial_state"]["rflags"])
            self.assertEqual(stack["initial_bytes_hex"], answer["memory"]["stack"])
            self.assertIn("Vol.1 §6.4.7", fixture["provenance"]["specification"])
            self.assertIn("Vol.2A §2.2.1", fixture["provenance"]["specification"])
            self.assertIn("Vol.2B RET", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
