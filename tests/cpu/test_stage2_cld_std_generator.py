"""Freshness and exact-shape checks for literal S2-18j CLD/STD fixtures."""

from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_cld_std.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_cld_std", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class CldStdGeneratorTests(unittest.TestCase):
    def test_exactly_32_literal_streams_cover_the_requested_groups(self) -> None:
        generated = GENERATOR.materialize()
        self.assertEqual(len(GENERATOR.CASES), 32)
        self.assertEqual(len(generated), 33)
        answers = json.loads(generated["s2_18j_cld_std_expected.json"])
        self.assertEqual(len(answers), 32)
        self.assertEqual(
            sum(case["code"] in {"fc", "fd"} for case in GENERATOR.CASES), 6
        )
        self.assertEqual(
            sum(case["code"] in {"4885c0fc", "4885c0fd"} for case in GENERATOR.CASES),
            2,
        )
        self.assertEqual(
            sum(case["code"] in {"fca4", "fcaa", "fcac", "fcae", "fca6", "fda4", "fdaa", "fdac", "fdae", "fda6"} and case.get("exit") != "harness_error" for case in GENERATOR.CASES),
            10,
        )
        self.assertEqual(
            sum(case["code"] in {"fcf3a4", "fcf3aa", "fcf3ac", "fdf3a4", "fdf3aa", "fdf3ac"} for case in GENERATOR.CASES),
            6,
        )
        self.assertEqual(
            sum(case["code"] in {"4839c0fcf3ae", "4839c0fdf3ae", "4839c0fcf3a6", "4839c0fdf3a6"} and not case["id"].endswith("_reject") for case in GENERATOR.CASES),
            4,
        )
        self.assertEqual(
            sum(case["id"].endswith("_reject") for case in GENERATOR.CASES), 2
        )
        self.assertEqual(
            sum(case.get("exit") == "harness_error" for case in GENERATOR.CASES), 2
        )
        self.assertEqual(
            sum(case.get("exit") == "unsupported" for case in GENERATOR.CASES), 2
        )
        for case_id, fixture_answer in answers.items():
            fixture = json.loads(generated[f"{case_id}.case.json"])
            self.assertEqual(fixture["schema_version"], 2)
            self.assertEqual(fixture["profile"], "s2-long-integer-v1")
            self.assertIn("serializes these constants only", fixture["provenance"]["source"])
            self.assertEqual(len(fixture_answer["gpr"]), 16)

    def test_generated_files_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            self.assertEqual((GENERATOR.FIXTURES / name).read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
