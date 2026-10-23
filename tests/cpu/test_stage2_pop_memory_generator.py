"""Structural checks for handwritten S2-17j POP memory fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_pop_memory.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_pop_memory", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class PopMemoryGeneratorTests(unittest.TestCase):
    def test_fixtures_cover_ea_profile_rsp_order_and_unspecified_store_fault(self) -> None:
        files = GENERATOR.materialize()
        answers = json.loads(files["s2_17j_pop_memory_expected.json"])
        for case, encoding in {
            "base": "8f00", "rex_base": "418f00", "sib": "8f048b",
            "addr32": "678f00", "rsp_post_increment": "8f0424",
        }.items():
            fixture = json.loads(files[f"s2_17j_pop_mem_{case}.case.json"])
            code = next(r for r in fixture["memory"] if r["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding)
            self.assertEqual(answers[fixture["case_id"]]["gpr"][4], f"0x{GENERATOR.STACK_RSP + 8:016x}")
        rip = json.loads(files["s2_17j_pop_mem_rip_relative.case.json"])
        rip_code = next(r for r in rip["memory"] if r["region_id"] == "code")
        self.assertEqual(rip_code["initial_bytes_hex"][:4], "8f05")
        fault = answers["s2_17j_pop_mem_store_pf"]
        self.assertEqual(fault["exception"]["vector"], 14)
        self.assertNotIn("gpr", fault)
        self.assertNotIn("rip", fault)
        self.assertEqual(fault["memory"]["data"], "a5" * 8)

    def test_generated_fixtures_are_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
