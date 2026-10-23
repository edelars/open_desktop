"""Structural checks for handwritten S2-17m mixed-prefix memory stack fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_stack_rexw_override.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_stack_rexw_override", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class StackRexwOverrideGeneratorTests(unittest.TestCase):
    def test_literal_encodings_and_qword_side_effects(self) -> None:
        files = GENERATOR.materialize()
        answers = json.loads(files["s2_17m_memory_stack_rexw_expected.json"])
        encodings = {
            "s2_17m_push_base": "6648ff30",
            "s2_17m_push_rsp_source": "6648ff3424",
            "s2_17m_pop_base": "66488f00",
            "s2_17m_pop_rsp_post_increment": "66488f0424",
        }
        for case_id, encoding in encodings.items():
            fixture = json.loads(files[f"{case_id}.case.json"])
            code = next(region for region in fixture["memory"] if region["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding)
            self.assertEqual(answers[case_id]["instructions_executed"], 1)
            self.assertEqual(answers[case_id]["rflags"], "0x0000000000000802")

        pushed = answers["s2_17m_push_base"]
        self.assertEqual(pushed["gpr"][4], f"0x{GENERATOR.STACK_RSP - 8:016x}")
        self.assertEqual(bytes.fromhex(pushed["memory"]["stack"])[112:120], GENERATOR.VALUE.to_bytes(8, "little"))
        popped = answers["s2_17m_pop_base"]
        self.assertEqual(popped["gpr"][4], f"0x{GENERATOR.STACK_RSP + 8:016x}")
        self.assertEqual(popped["memory"]["data"], GENERATOR.VALUE.to_bytes(8, "little").hex())
        alias = answers["s2_17m_pop_rsp_post_increment"]
        self.assertEqual(bytes.fromhex(alias["memory"]["stack"])[128:136], GENERATOR.VALUE.to_bytes(8, "little"))

    def test_generated_fixtures_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
