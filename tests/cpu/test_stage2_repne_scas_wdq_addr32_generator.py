import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = Path(__file__).with_name("generate_stage2_repne_scas_wdq_addr32.py")

class RepneScasWdqAddress32GeneratorTests(unittest.TestCase):
    def test_handwritten_fixtures_are_current(self):
        subprocess.run([sys.executable, str(SCRIPT), "--check"], cwd=ROOT, check=True)

if __name__ == "__main__":
    unittest.main()
