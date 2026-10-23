"""Structural checks for handwritten S2-17i memory PUSH answers."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("generate_stage2_push_memory.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_push_memory", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class PushMemoryGeneratorTests(unittest.TestCase):
    def test_literal_addressing_and_fault_fixtures_are_complete(self) -> None:
        files = GENERATOR.materialize()
        expected = json.loads(files["s2_17i_push_memory_expected.json"])
        self.assertEqual(len(expected), 9)
        encodings = {
            "s2_17i_push_mem_base": "ff30",
            "s2_17i_push_mem_rex_base": "41ff30",
            "s2_17i_push_mem_sib": "ff348b",
            "s2_17i_push_mem_addr32": "67ff30",
            "s2_17i_push_mem_rsp_source": "ff3424",
            "s2_17i_push_mem_read_fault": "ff30",
            "s2_17i_push_mem_stack_fault": "ff30",
        }
        for case_id, encoding in encodings.items():
            fixture = json.loads(files[f"{case_id}.case.json"])
            code = next(region for region in fixture["memory"] if region["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding, case_id)
        rip = json.loads(files["s2_17i_push_mem_rip_relative.case.json"])
        rip_code = next(region for region in rip["memory"] if region["region_id"] == "code")
        self.assertEqual(rip_code["initial_bytes_hex"][:4], "ff35")

        for case_id in [
            "s2_17i_push_mem_base",
            "s2_17i_push_mem_rex_base",
            "s2_17i_push_mem_sib",
            "s2_17i_push_mem_rip_relative",
            "s2_17i_push_mem_addr32",
            "s2_17i_push_mem_budget",
        ]:
            answer = expected[case_id]
            stack = bytes.fromhex(answer["memory"]["stack"])
            self.assertEqual(stack[-8:], GENERATOR.SOURCE_VALUE.to_bytes(8, "little"), case_id)
        rsp_answer = expected["s2_17i_push_mem_rsp_source"]
        rsp_stack = bytes.fromhex(rsp_answer["memory"]["stack"])
        self.assertEqual(rsp_stack[-16:], GENERATOR.SOURCE_VALUE.to_bytes(8, "little") * 2)

        for case_id, vector, error_code in [
            ("s2_17i_push_mem_read_fault", 14, 5),
            ("s2_17i_push_mem_stack_fault", 12, 0),
        ]:
            answer = expected[case_id]
            self.assertEqual(answer["exit"], "exception")
            self.assertEqual(answer["instructions_executed"], 0)
            self.assertEqual(answer["exception"]["vector"], vector)
            self.assertEqual(answer["exception"]["error_code"], error_code)
            self.assertEqual(answer["gpr"], json.loads(files[f"{case_id}.case.json"])["initial_state"]["gpr"])

    def test_fixture_set_is_deterministic_and_fresh(self) -> None:
        files = GENERATOR.materialize()
        for name, content in files.items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
