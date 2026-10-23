"""Structural and freshness checks for the literal S2-21a fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_xchg_qword_memory.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_xchg_qword_memory", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class MemoryXchgGeneratorTests(unittest.TestCase):
    def test_literal_profile_encodes_unaligned_alias_exchange(self) -> None:
        files = GENERATOR.materialize()
        fixture = json.loads(files["s2_21a_xchg_qword_memory_alias.case.json"])
        self.assertEqual(fixture["expected_exit"], "stop_address")
        self.assertEqual(fixture["instruction_budget"], 1)
        self.assertEqual(fixture["initial_state"]["gpr"][0], GENERATOR.hx(GENERATOR.ADDRESS))
        self.assertEqual([region["initial_bytes_hex"] for region in fixture["memory"]], [
            GENERATOR.CODE,
            GENERATOR.LEFT_BEFORE,
            GENERATOR.RIGHT_BEFORE,
        ])
        self.assertIn("GuestMemory atomic", fixture["provenance"]["specification"])
        self.assertIn("does not establish SMP", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.FIXTURES / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
