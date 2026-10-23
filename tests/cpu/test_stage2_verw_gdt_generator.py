"""Provenance and freshness checks for the literal S2-22e VERW cases."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_verw_gdt.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_verw_gdt", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class VerwGdtFixtureTests(unittest.TestCase):
    def test_literals_freeze_descriptor_type_privilege_and_limit_inputs(self) -> None:
        cases = {json.loads(raw)["case_id"]: json.loads(raw)
                 for raw in GENERATOR.materialize().values()}
        self.assertEqual(len(cases), 8)
        for case_id, value in cases.items():
            self.assertEqual(value["profile"], "s2-long-verw-gdt-v1")
            self.assertEqual(value["memory"][0]["initial_bytes_hex"], "0f00e8")
            self.assertEqual(value["initial_state"]["gpr"][0][-4:]
                             in {f"{selector:04x}" for selector in (0x0b, 0x03, 0x08)}, True)
            self.assertEqual(value["initial_state"]["gdtr_base"], "0x0000000000200000")
            self.assertIn("326018-093US", value["provenance"]["specification"])
            self.assertIn("325384-093US", value["provenance"]["specification"])
            self.assertIn("671072", value["provenance"]["specification_version"])
            self.assertIn("671447", value["provenance"]["specification_version"])
        table = cases["s2_22e_verw_table_limit"]
        self.assertEqual(table["initial_state"]["gdtr_limit"], "0x000000000000000e")
        self.assertEqual(table["memory"][1]["permissions"],
                         {"read": True, "write": False, "execute": False})
        self.assertEqual(table["memory"][1]["comparison_mask_hex"], "ff" * 16)

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
