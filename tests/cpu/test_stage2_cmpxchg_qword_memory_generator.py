"""Structural and freshness checks for literal S2-21e CMPXCHG fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_cmpxchg_qword_memory.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_cmpxchg_qword_memory", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class QwordMemoryCmpxchgGeneratorTests(unittest.TestCase):
    def test_literal_equal_and_unequal_alias_encodings(self) -> None:
        files = GENERATOR.materialize()
        equal = json.loads(files["s2_21e_cmpxchg_qword_memory_equal.case.json"])
        unequal = json.loads(files["s2_21e_cmpxchg_qword_memory_unequal_alias.case.json"])
        self.assertEqual(len(files), 2)
        self.assertEqual(equal["memory"][0]["initial_bytes_hex"], "480fb118")
        self.assertEqual(unequal["memory"][0]["initial_bytes_hex"], "480fb100")
        for fixture in (equal, unequal):
            self.assertEqual(fixture["memory"][1]["guest_address"], "0x0000000000002000")
            self.assertEqual(fixture["memory"][1]["length"], 7)
            self.assertEqual(fixture["memory"][2]["guest_address"], "0x0000000000002007")
            self.assertEqual(fixture["memory"][2]["length"], 8)
            self.assertEqual(fixture["expected_exit"], "stop_address")
            self.assertEqual(fixture["instruction_budget"], 1)
            self.assertIn("Intel SDM revision 093", fixture["provenance"]["specification"])
        self.assertEqual(equal["initial_state"]["gpr"][0], "0x0000000000002003")
        self.assertEqual(unequal["initial_state"]["gpr"][0], "0x0000000000002003")

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.FIXTURES / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
