"""Structural checks for handwritten S2-17k memory PUSH fixtures."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_push_memory16.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_push_memory16", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class PushMemory16GeneratorTests(unittest.TestCase):
    def test_literal_word_eas_rsp_source_and_fault_answers(self) -> None:
        files = GENERATOR.materialize()
        expected = json.loads(files["s2_17k_push_memory16_expected.json"])
        self.assertEqual(len(expected), 8)
        encodings = {
            "base": "66ff30", "rex_base": "6641ff30", "sib": "66ff348b",
            "addr32": "6766ff30", "rsp_source": "66ff3424",
            "read_fault": "66ff30", "stack_fault": "66ff30",
        }
        for suffix, encoding in encodings.items():
            case_id = f"s2_17k_push_mem_{suffix}"
            fixture = json.loads(files[f"{case_id}.case.json"])
            code = next(region for region in fixture["memory"] if region["region_id"] == "code")
            self.assertEqual(code["initial_bytes_hex"], encoding, case_id)
            self.assertEqual(expected[case_id]["gpr"][4], fixture["initial_state"]["gpr"][4] if "fault" in suffix else GENERATOR.h(GENERATOR.STACK_RSP - 2))
        rip = json.loads(files["s2_17k_push_mem_rip_relative.case.json"])
        rip_code = next(region for region in rip["memory"] if region["region_id"] == "code")
        self.assertEqual(rip_code["initial_bytes_hex"][:6], "66ff35")
        rsp_answer = expected["s2_17k_push_mem_rsp_source"]["memory"]["stack"]
        rsp_bytes = bytes.fromhex(rsp_answer)
        self.assertEqual(rsp_bytes[GENERATOR.STACK_RSP - GENERATOR.STACK_BASE - 2:][:2], GENERATOR.SOURCE_WORD.to_bytes(2, "little"))
        for suffix, vector, error_code in [("read_fault", 14, 5), ("stack_fault", 14, 6)]:
            answer = expected[f"s2_17k_push_mem_{suffix}"]
            self.assertEqual(answer["exit"], "exception")
            self.assertEqual(answer["exception"]["vector"], vector)
            self.assertEqual(answer["exception"]["error_code"], error_code)
            self.assertEqual(answer["instructions_executed"], 0)

    def test_fixture_set_is_deterministic_and_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
