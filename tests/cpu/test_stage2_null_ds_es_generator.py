"""Provenance and freshness checks for literal S2-22k null DS/ES fixtures."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_null_ds_es.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_null_ds_es", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class NullDsEsFixtureTests(unittest.TestCase):
    def test_literals_freeze_exact_bytes_initial_segments_and_provenance(self) -> None:
        cases = {
            json.loads(raw)["case_id"]: json.loads(raw)
            for raw in GENERATOR.materialize().values()
        }
        self.assertEqual(len(cases), 4)
        for value in cases.values():
            self.assertEqual(value["profile"], "s2-long-null-ds-es-v1")
            self.assertIn("325384-093US", value["provenance"]["specification"])
            self.assertIn("253667-093US", value["provenance"]["specification"])
            self.assertIn("S2-22k local-only", value["provenance"]["specification_version"])
            self.assertEqual(value["initial_state"]["gpr"][0], "0xfeedbeef00000000")
            self.assertEqual(value["initial_state"]["ds_selector"], "0x000000000000002b")
            self.assertEqual(value["initial_state"]["es_selector"], "0x0000000000000030")
            self.assertFalse(value["initial_state"]["ds_unusable"])
            self.assertFalse(value["initial_state"]["es_unusable"])
            self.assertEqual(value["memory"][0]["permissions"],
                             {"read": False, "write": False, "execute": True})
        self.assertEqual(cases["s2_22k_null_ds"]["memory"][0]["initial_bytes_hex"], "8ed8")
        self.assertEqual(cases["s2_22k_null_es"]["memory"][0]["initial_bytes_hex"], "8ec0")
        self.assertEqual(cases["s2_22k_null_ds_then_es"]["memory"][0]["initial_bytes_hex"],
                         "8ed88ec0")
        self.assertEqual(cases["s2_22k_null_ds_repeated"]["memory"][0]["initial_bytes_hex"],
                         "8ed88ed8")

    def test_materialized_fixtures_are_fresh(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--check"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
