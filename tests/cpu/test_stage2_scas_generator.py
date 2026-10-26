"""Structural and freshness checks for literal S2-18h SCAS fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_scas.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_scas", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class ScasGeneratorTests(unittest.TestCase):
    def test_exact_24_literal_inputs_and_expected_snapshots(self) -> None:
        generated = GENERATOR.materialize()
        self.assertEqual(len(generated), 25)
        expected = json.loads(generated["s2_18h_scas_expected.json"])
        self.assertEqual(len(expected), 24)
        self.assertEqual(
            {name for name in expected if "test_" in name},
            {"s2_18h_scas_test_composition", "s2_18h_scas_test_fault"},
        )
        for case_id in expected:
            fixture = json.loads(generated[f"{case_id}.case.json"])
            self.assertEqual(fixture["profile"], "s2-long-integer-v1")
            self.assertIn("serializes these constants only", fixture["provenance"]["source"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            self.assertEqual((GENERATOR.FIXTURES / name).read_bytes(), content)


if __name__ == "__main__":
    unittest.main()
