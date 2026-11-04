"""Structural and freshness checks for the literal S2-21h LOCK XCHG fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_lock_xchg_qword_memory.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_lock_xchg_qword_memory", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class LockedQwordMemoryXchgGeneratorTests(unittest.TestCase):
    def test_literal_lock_alias_split_and_provenance(self) -> None:
        fixture = json.loads(
            GENERATOR.materialize()["s2_21h_lock_xchg_qword_memory_alias.case.json"]
        )
        self.assertEqual(fixture["memory"][0]["initial_bytes_hex"], "f0488700")
        self.assertEqual(fixture["initial_state"]["gpr"][0], "0x0000000000200ffe")
        self.assertEqual(fixture["memory"][1]["guest_address"], "0x0000000000200ff8")
        self.assertEqual(fixture["memory"][2]["guest_address"], "0x0000000000201000")
        self.assertEqual(fixture["instruction_budget"], 1)
        self.assertIn("revision 093", fixture["provenance"]["specification_version"])
        self.assertIn("SMP", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.FIXTURES / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
