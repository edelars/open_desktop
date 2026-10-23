"""Provenance and freshness checks for literal S2-22h MOV GS,AX cases."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_gs_load.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_gs_load", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class GsLoadFixtureTests(unittest.TestCase):
    def test_literals_freeze_instruction_selector_descriptor_and_provenance(self) -> None:
        cases = {
            json.loads(raw)["case_id"]: json.loads(raw)
            for raw in GENERATOR.materialize().values()
        }
        self.assertEqual(len(cases), 11)
        for case_id, value in cases.items():
            self.assertEqual(value["profile"], "s2-long-gs-load-v1")
            self.assertEqual(value["memory"][0]["initial_bytes_hex"], "8ee8")
            self.assertIn("253667-093US", value["provenance"]["specification"])
            self.assertIn("325384-093US", value["provenance"]["specification"])
            self.assertIn("671241", value["provenance"]["specification_version"])
            self.assertIn("671447", value["provenance"]["specification_version"])
            self.assertEqual(value["initial_state"]["gdtr_base"], "0x0000000000200000")
            self.assertEqual(value["initial_state"]["gpr"][0][-4:],
                             "0008" if "privilege" in case_id or "conforming" in case_id else "000b")
        readonly = cases["s2_22h_gs_load_readonly_g1"]
        self.assertEqual(readonly["memory"][1]["initial_bytes_hex"][28:30], "80")
        self.assertEqual(readonly["memory"][1]["permissions"],
                         {"read": True, "write": False, "execute": False})
        conforming = cases["s2_22h_gs_load_conforming_code"]
        self.assertEqual(conforming["memory"][1]["initial_bytes_hex"][26:28], "9f")
        writable_g0 = cases["s2_22h_gs_load_writable_g0"]
        self.assertEqual(
            writable_g0["memory"][1]["initial_bytes_hex"],
            "00000000000000000000efcdabf30089",
        )
        self.assertIn("base32=0x89abcdef", writable_g0["provenance"]["specification_version"])
        bounds = cases["s2_22h_gs_load_table_limit_gp"]
        self.assertEqual(bounds["initial_state"]["gdtr_limit"], "0x000000000000000e")
        self.assertEqual(len(bounds["memory"][1]["initial_bytes_hex"]), 32)
        for case_id, access in [
            ("s2_22h_gs_load_a0_writable", "f2"),
            ("s2_22h_gs_load_a0_readonly", "f0"),
        ]:
            case = cases[case_id]
            self.assertEqual(case["memory"][1]["initial_bytes_hex"][26:28], access)
            self.assertTrue(case["memory"][1]["permissions"]["write"])
            self.assertIn("11.1.2.1", case["provenance"]["specification"])
            self.assertIn("S2-22h", case["provenance"]["specification_version"])

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
