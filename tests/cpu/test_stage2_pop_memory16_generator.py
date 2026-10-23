"""Structural checks for handwritten S2-17l word POP memory fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_pop_memory16.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_pop_memory16", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class PopMemory16GeneratorTests(unittest.TestCase):
    def test_fixtures_cover_operand_width_eas_rsp_order_and_unknown_fault_state(self) -> None:
        files = GENERATOR.materialize()
        answers = json.loads(files["s2_17l_pop_memory16_expected.json"])
        for case, encoding in {
            "base": "668f00", "rex_base": "66418f00", "sib": "668f048b",
            "addr32": "67668f00", "rsp_post_increment": "668f0424",
        }.items():
            fixture = json.loads(files[f"s2_17l_pop_mem_{case}.case.json"])
            code = next(r for r in fixture["memory"] if r["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding)
            self.assertEqual(answers[fixture["case_id"]]["gpr"][4], f"0x{GENERATOR.STACK_RSP + 2:016x}")
        rip = json.loads(files["s2_17l_pop_mem_rip_relative.case.json"])
        rip_code = next(r for r in rip["memory"] if r["region_id"] == "code")
        self.assertTrue(rip_code["initial_bytes_hex"].startswith("668f05"))
        stack_case = answers["s2_17l_pop_mem_rsp_post_increment"]
        stack = bytes.fromhex(stack_case["memory"]["stack"])
        self.assertEqual(stack[122:124], b"\xef\xbe")
        fault = answers["s2_17l_pop_mem_store_pf"]
        self.assertEqual(fault["exception"]["vector"], 14)
        self.assertNotIn("gpr", fault)
        self.assertNotIn("rip", fault)
        self.assertEqual(fault["memory"]["data"], "a5" * 2)

    def test_generated_fixtures_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
