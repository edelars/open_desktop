"""Structural checks for the literal S2-17r POPFW fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_popfw.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_popfw", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class PopfwGeneratorTests(unittest.TestCase):
    def test_literal_encoding_flags_and_word_stack_effect(self) -> None:
        files = GENERATOR.materialize()
        case_id = "s2_17r_popfw_updates_allowed_flags"
        fixture = json.loads(files[f"{case_id}.case.json"])
        answer = json.loads(files["s2_17r_popfw_expected.json"])[case_id]
        code = next(region for region in fixture["memory"] if region["region_id"] == "code")
        self.assertEqual(code["initial_bytes_hex"], "669d")
        self.assertEqual(fixture["rflags_mask"], "0x0000000000000cd7")
        self.assertEqual(answer["gpr"][4], f"0x{GENERATOR.STACK_RSP + 2:016x}")
        self.assertEqual(answer["rflags"], "0x0000000000000cd7")
        self.assertEqual(answer["rflags_defined"], "0x0000000000000cd7")
        stack = next(region for region in fixture["memory"] if region["region_id"] == "stack")
        offset = GENERATOR.STACK_RSP - GENERATOR.STACK_BASE
        self.assertEqual(
            bytes.fromhex(stack["initial_bytes_hex"])[offset:offset + 2], b"\xd5\x3c"
        )

    def test_generated_fixture_is_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
