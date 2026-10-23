"""Provenance and freshness checks for the literal S2-22c fixture."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_fs_selector.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_fs_selector", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class FsSelectorFixtureTests(unittest.TestCase):
    def test_literal_state_and_provenance(self) -> None:
        fixture = next(iter(GENERATOR.materialize().values()))
        import json

        value = json.loads(fixture)
        self.assertEqual(value["profile"], "s2-long-fs-selector-v1")
        self.assertEqual(value["memory"][0]["initial_bytes_hex"], "488ce0")
        self.assertEqual(value["initial_state"]["fs_selector"], "0x000000000000f123")
        self.assertEqual(value["initial_state"]["fs_base"], "0xffff800000001000")
        self.assertIn("Vol.2B (253667-093US)", value["provenance"]["specification"])
        self.assertIn("671241", value["provenance"]["specification_version"])

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
