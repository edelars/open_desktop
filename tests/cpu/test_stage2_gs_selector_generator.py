"""Provenance and freshness checks for the literal S2-22d fixture."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_gs_selector.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_gs_selector", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class GsSelectorFixtureTests(unittest.TestCase):
    def test_literal_state_and_provenance(self) -> None:
        fixture = next(iter(GENERATOR.materialize().values()))
        import json

        value = json.loads(fixture)
        self.assertEqual(value["profile"], "s2-long-gs-selector-v1")
        self.assertEqual(value["memory"][0]["initial_bytes_hex"], "488ce8")
        self.assertEqual(value["initial_state"]["gs_selector"], "0x000000000000f123")
        self.assertEqual(value["initial_state"]["gs_base"], "0xffff800000001000")
        self.assertIn("Vol.2B (253667-093US)", value["provenance"]["specification"])
        self.assertIn("Vol.1 §3.4.2", value["provenance"]["specification"])
        self.assertIn("253665-093US", value["provenance"]["specification"])
        self.assertIn("671241", value["provenance"]["specification_version"])
        self.assertIn("671436", value["provenance"]["specification_version"])

    def test_materialized_fixture_is_fresh(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--check"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
