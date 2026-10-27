from __future__ import annotations

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = Path(__file__).with_name("generate_stage2_rep_scas_wdq.py")


class RepScasWidthGeneratorTests(unittest.TestCase):
    def test_materializes_exactly_59_literal_fixtures(self) -> None:
        spec = importlib.util.spec_from_file_location("rep_scas_wdq_generator", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(module)
        files = module.materialize()
        self.assertEqual(sum(name.endswith(".case.json") for name in files), 59)

    def test_checked_in_bytes_are_current(self) -> None:
        subprocess.run([sys.executable, str(SCRIPT), "--check"], cwd=ROOT, check=True)


if __name__ == "__main__":
    unittest.main()
