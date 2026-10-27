import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/cpu/generate_stage2_rep_stos_addr32.py"
SPEC = importlib.util.spec_from_file_location("rep_stos_addr32_generator", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class RepStosAddr32GeneratorTests(unittest.TestCase):
    def test_checked_in_bytes_are_current(self):
        for name, content in GENERATOR.materialize().items():
            self.assertEqual((GENERATOR.FIXTURES / name).read_bytes(), content, name)

    def test_all_expected_answers_are_literal_full_snapshots(self):
        expected = json.loads(GENERATOR.materialize()["s2_19k_rep_stos_addr32_expected.json"])
        self.assertEqual(len(expected), 11)
        for answer in expected.values():
            self.assertEqual(len(answer["gpr"]), 16)
            self.assertEqual(answer["rflags_defined"], "0x0000000000000cd7")
            self.assertEqual(set(answer["memory"]), {"code", "data"})


if __name__ == "__main__":
    unittest.main()
