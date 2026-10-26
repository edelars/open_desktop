"""Freshness and shape checks for literal S2-18i CMPS fixtures."""

import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_cmps.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_cmps", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class CmpsGeneratorTests(unittest.TestCase):
    def test_exact_fixture_count_and_streams(self):
        generated = GENERATOR.materialize()
        self.assertEqual(len(GENERATOR.CASES), 33)
        self.assertEqual(len(generated), 34)
        self.assertIn("s2_18i_cmps_test_first_fault.case.json", generated)
        self.assertIn("s2_18i_cmps_test_second_fault.case.json", generated)

    def test_generated_files_are_fresh(self):
        for name, contents in GENERATOR.materialize().items():
            self.assertEqual((GENERATOR.FIXTURES / name).read_bytes(), contents, name)


if __name__ == "__main__":
    unittest.main()
