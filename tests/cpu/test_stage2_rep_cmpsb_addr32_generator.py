import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = Path(__file__).with_name("generate_stage2_rep_cmpsb_addr32.py")


class RepCmpsbAddress32GeneratorTests(unittest.TestCase):
    def test_materializes_exact_fixture_answers(self):
        spec = importlib.util.spec_from_file_location("rep_cmpsb_addr32_generator", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(module)
        files = module.materialize()
        self.assertEqual(len(files), 14)
        self.assertEqual(sum(name.endswith(".case.json") for name in files), 13)

    def test_checked_in_bytes_are_current(self):
        subprocess.run([sys.executable, str(SCRIPT), "--check"], cwd=ROOT, check=True)


if __name__ == "__main__":
    unittest.main()
