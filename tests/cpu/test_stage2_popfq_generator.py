"""Structural checks for literal S2-17o POPFQ fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_popfq.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_popfq", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class PopfqGeneratorTests(unittest.TestCase):
    def test_literal_encodings_flags_and_stack_effects(self) -> None:
        files = GENERATOR.materialize()
        answers = json.loads(files["s2_17o_popfq_expected.json"])
        self.assertEqual(len(answers), 2)
        for case_id in answers:
            fixture = json.loads(files[f"{case_id}.case.json"])
            code = next(region for region in fixture["memory"] if region["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], "9d")
            self.assertEqual(fixture["rflags_mask"], "0x0000000000000cd7")
            self.assertEqual(answers[case_id]["gpr"][4], f"0x{GENERATOR.STACK_RSP + 8:016x}")
            self.assertEqual(answers[case_id]["rflags_defined"], "0x0000000000000cd7")

        self.assertEqual(
            answers["s2_17o_loads_status_flags_and_df"]["rflags"],
            "0x0000000000000cd7",
        )
        self.assertEqual(
            answers["s2_17o_clears_status_flags_and_df"]["rflags"],
            "0x0000000000000002",
        )

    def test_generated_fixtures_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
