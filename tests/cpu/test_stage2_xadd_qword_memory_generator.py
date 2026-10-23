"""Structural and freshness checks for the literal S2-21d XADD fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_xadd_qword_memory.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_xadd_qword_memory", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class QwordMemoryXaddGeneratorTests(unittest.TestCase):
    def test_literal_alias_split_and_provenance(self) -> None:
        fixture = json.loads(
            GENERATOR.materialize()["s2_21d_xadd_qword_memory_alias_split.case.json"]
        )
        self.assertEqual(fixture["memory"][0]["initial_bytes_hex"], "480fc100")
        self.assertEqual(fixture["initial_state"]["gpr"][0], "0x00000000003000fb")
        self.assertEqual(fixture["memory"][1]["guest_address"], "0x00000000003000f8")
        self.assertEqual(fixture["memory"][2]["guest_address"], "0x0000000000300100")
        self.assertEqual(fixture["instruction_budget"], 1)
        self.assertIn("local-only", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.FIXTURES / name
            self.assertTrue(path.exists(), name)
            self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
