from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/cpu/generate_stage2_repne_scasb_addr32.py"
spec = importlib.util.spec_from_file_location("repne_scasb_generator", SCRIPT)
generator = importlib.util.module_from_spec(spec)
assert spec and spec.loader
spec.loader.exec_module(generator)


class RepneScasbAddr32GeneratorTests(unittest.TestCase):
    def test_committed_fixtures_match_handwritten_serialization(self) -> None:
        for name, expected in generator.materialize().items():
            with self.subTest(name=name):
                self.assertEqual((generator.FIXTURES / name).read_bytes(), expected)


if __name__ == "__main__":
    unittest.main()
