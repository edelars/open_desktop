"""Structural checks for literal S2-17n mixed-prefix register fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_stack_rexw_register.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_stack_rexw_register", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class StackRexwRegisterGeneratorTests(unittest.TestCase):
    def test_literal_encodings_and_operand_width_side_effects(self) -> None:
        files = GENERATOR.materialize()
        answers = json.loads(files["s2_17n_register_stack_rexw_expected.json"])
        encodings = {
            "s2_17n_push_qword_rax": "664850",
            "s2_17n_push_qword_r8": "664950",
            "s2_17n_push_word_last_rex_clears_w_rax": "66484050",
            "s2_17n_pop_qword_r9": "664959",
            "s2_17n_pop_word_last_rex_clears_w_r9": "6649414159",
            "s2_17n_pop_word_last_rex_clears_w_rax": "66484058",
        }
        for case_id, encoding in encodings.items():
            fixture = json.loads(files[f"{case_id}.case.json"])
            code = next(region for region in fixture["memory"] if region["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding)
            self.assertEqual(answers[case_id]["rflags"], "0x0000000000000802")

        self.assertEqual(answers["s2_17n_push_qword_r8"]["gpr"][4], f"0x{GENERATOR.STACK_RSP - 8:016x}")
        self.assertEqual(answers["s2_17n_push_word_last_rex_clears_w_rax"]["gpr"][4], f"0x{GENERATOR.STACK_RSP - 2:016x}")
        self.assertEqual(answers["s2_17n_pop_word_last_rex_clears_w_r9"]["gpr"][9], "0xaabbccddeeff2468")
        self.assertEqual(answers["s2_17n_pop_qword_r9"]["gpr"][9], f"0x{GENERATOR.QWORD_VALUE:016x}")

    def test_generated_fixtures_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
