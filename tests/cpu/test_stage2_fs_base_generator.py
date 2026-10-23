"""Provenance and freshness checks for the literal S2-22a fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_fs_base.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_fs_base", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class FsBaseGeneratorTests(unittest.TestCase):
    def test_literal_encoding_address_and_provenance(self) -> None:
        fixture = json.loads(next(iter(GENERATOR.materialize().values())))
        self.assertEqual(fixture["profile"], "s2-long-fs-base-v1")
        self.assertEqual(fixture["memory"][0]["initial_bytes_hex"], "64488b00")
        self.assertEqual(fixture["initial_state"]["fs_base"], "0xffff800000000000")
        self.assertEqual(fixture["memory"][1]["guest_address"], "0x0000000000400000")
        self.assertIn("Vol.1", fixture["provenance"]["specification"])
        self.assertIn("Vol.2B", fixture["provenance"]["specification"])

    def test_generated_file_is_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            self.assertEqual((GENERATOR.FIXTURES / name).read_bytes(), content)


if __name__ == "__main__":
    unittest.main()
