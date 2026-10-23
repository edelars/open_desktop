"""Structural and freshness checks for the literal S2-21g MFENCE fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_mfence.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_mfence", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class MfenceGeneratorTests(unittest.TestCase):
    def test_literal_store_fence_load_and_provenance(self) -> None:
        fixture = json.loads(
            GENERATOR.materialize()["s2_21g_mfence_store_fence_load.case.json"]
        )
        self.assertEqual(fixture["memory"][0]["initial_bytes_hex"], "4889180faef0488b08")
        self.assertEqual(fixture["instruction_budget"], 3)
        self.assertEqual(
            fixture["memory"][1]["permissions"],
            {"read": True, "write": True, "execute": False},
        )
        self.assertIn("Vol.2B MFENCE p.4-15", fixture["provenance"]["specification"])
        self.assertIn("Other 0F AE forms are outside", fixture["provenance"]["specification"])

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            self.assertEqual((GENERATOR.FIXTURES / name).read_bytes(), content)


if __name__ == "__main__":
    unittest.main()
