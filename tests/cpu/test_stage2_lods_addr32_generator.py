"""Freshness and shape checks for literal S2-18k address-size-32 LODS fixtures."""

from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_lods_addr32.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_lods_addr32", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class LodsAddr32GeneratorTests(unittest.TestCase):
    def test_exact_39_cases_cover_the_address_size_32_fixture_groups(self) -> None:
        generated = GENERATOR.materialize()
        self.assertEqual(len(GENERATOR.CASES), 39)
        self.assertEqual(len(generated), 40)
        answers = json.loads(generated["s2_18k_lods_addr32_expected.json"])
        self.assertEqual(len(answers), 39)
        ids = [case[0] for case in GENERATOR.CASES]
        self.assertEqual(len(set(ids)), 39)
        self.assertEqual(sum("_df" in case_id for case_id in ids), 8)
        self.assertEqual(sum("_unmapped" in case_id for case_id in ids), 5)
        self.assertEqual(sum("_budget0" in case_id for case_id in ids), 4)
        self.assertEqual(sum("_wrap_" in case_id for case_id in ids), 8)
        self.assertEqual(sum("_cross_4g" in case_id for case_id in ids), 3)
        self.assertEqual(sum("_normalize_" in case_id for case_id in ids), 2)
        self.assertEqual(sum(case_id.startswith("s2_18k_test_") for case_id in ids), 2)
        self.assertEqual(sum(case_id.startswith(("s2_18k_cld_", "s2_18k_std_")) for case_id in ids), 8)

    def test_cases_use_v2_profile_and_truncate_only_the_starting_pointer(self) -> None:
        generated = GENERATOR.materialize()
        answers = json.loads(generated["s2_18k_lods_addr32_expected.json"])
        for case_id, answer in json.loads(generated["s2_18k_lods_addr32_expected.json"]).items():
            fixture = json.loads(generated[f"{case_id}.case.json"])
            self.assertEqual(fixture["schema_version"], 2, case_id)
            self.assertEqual(fixture["profile"], "s2-long-integer-v1", case_id)
            self.assertEqual(len(fixture["initial_state"]["gpr"]), 16, case_id)
            self.assertEqual(len(answer["gpr"]), 16, case_id)
        by_id = {case[0]: case for case in GENERATOR.CASES}
        for case_id, row in by_id.items():
            self.assertEqual(
                int(answers[case_id]["gpr"][6], 16),
                row[-1],
                f"{case_id} must serialize its handwritten expected RSI",
            )
        for case_id in ("s2_18k_lodsw_cross_4g", "s2_18k_lodsd_cross_4g", "s2_18k_lodsq_cross_4g"):
            row = by_id[case_id]
            self.assertEqual(row[7], 0xFFFFFFFF, case_id)
            self.assertGreater(int(row[8], 16), 0, case_id)
            self.assertEqual(
                int(answers[case_id]["gpr"][6], 16),
                {"s2_18k_lodsw_cross_4g": 1, "s2_18k_lodsd_cross_4g": 3, "s2_18k_lodsq_cross_4g": 7}[case_id],
                case_id,
            )

    def test_checked_in_fixture_bytes_are_current(self) -> None:
        for name, contents in GENERATOR.materialize().items():
            self.assertEqual((GENERATOR.FIXTURES / name).read_bytes(), contents, name)


if __name__ == "__main__":
    unittest.main()
