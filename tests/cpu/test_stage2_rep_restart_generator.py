from __future__ import annotations

import importlib.util
import hashlib
import json
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "tests/cpu/generate_stage2_rep_restart.py"
FIXTURES = ROOT / "tests/fixtures/x86/stage2"


class RepRestartGeneratorTests(unittest.TestCase):
    def test_generated_fixtures_are_fresh(self) -> None:
        subprocess.run([sys.executable, str(GENERATOR), "--check"], check=True)

    def test_six_schema_v2_cases_and_literal_two_pass_snapshots(self) -> None:
        spec = importlib.util.spec_from_file_location("rep_restart_generator", GENERATOR)
        module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(module)
        generated = module.materialize()
        self.assertEqual(len(generated), 7)
        expected = json.loads(generated["s2_19i_rep_restart_expected.json"])
        self.assertEqual(len(expected), 6)
        self.assertEqual(
            set(expected),
            {
                f"s2_19i_rep_restart_{operation}_df{df}"
                for operation in ("stosb", "lodsb", "movsb")
                for df in (0, 1)
            },
        )
        for case_id, snapshots in expected.items():
            fixture = json.loads(generated[f"{case_id}.case.json"])
            self.assertEqual(fixture["schema_version"], 2)
            self.assertEqual(fixture["expected_exit"], "rep_iteration_budget_exhausted")
            self.assertEqual(fixture["instruction_budget"], 2)
            self.assertEqual(fixture["initial_state"]["gpr"][1], "0x0000000000000401")
            self.assertEqual(snapshots["partial"]["gpr"][1], "0x0000000000000001")
            self.assertEqual(snapshots["partial"]["rflags_defined"], "0x0000000000000cc7")
            self.assertEqual(snapshots["resumed"]["gpr"][1], "0x0000000000000000")
            self.assertEqual(snapshots["resumed"]["outcome"], "stop_address")
            for phase in ("partial", "resumed"):
                snapshot = snapshots[phase]
                self.assertEqual(len(snapshot["gpr"]), 16)
                self.assertEqual(snapshot["rflags_defined"], "0x0000000000000cc7")
                self.assertEqual(len(snapshot["memory"]["code"]), 10)
                for region_id, region_hex in snapshot["memory"].items():
                    if region_id != "code":
                        self.assertEqual(len(region_hex), 2 * 1025)
        self.assertEqual(
            hashlib.sha256(module.dump(module.EXPECTED)).hexdigest(),
            module.EXPECTED_SHA256,
        )


if __name__ == "__main__":
    unittest.main()
