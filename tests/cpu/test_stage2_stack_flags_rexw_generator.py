"""Structural checks for the literal S2-17s flags REX.W fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_stack_flags_rexw.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_stack_flags_rexw", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class StackFlagsRexwGeneratorTests(unittest.TestCase):
    def test_literal_prefixes_stack_round_trip_and_canary_mask(self) -> None:
        files = GENERATOR.materialize()
        case_id = "s2_17s_pushf_rexw_overrides_66_qword_roundtrip"
        fixture = json.loads(files[f"{case_id}.case.json"])
        answer = json.loads(files["s2_17s_pushf_rexw_expected.json"])[case_id]
        code = next(region for region in fixture["memory"] if region["region_id"] == "code")
        stack = next(region for region in fixture["memory"] if region["region_id"] == "stack")

        self.assertEqual(code["initial_bytes_hex"], "66489c66489d")
        self.assertEqual(fixture["instruction_budget"], 2)
        self.assertEqual(answer["gpr"][4], f"0x{GENERATOR.STACK_RSP:016x}")
        self.assertEqual(answer["rflags"], f"0x{GENERATOR.RFLAGS_MASK:016x}")
        self.assertEqual(answer["rflags_defined"], f"0x{GENERATOR.RFLAGS_MASK:016x}")
        self.assertEqual(fixture["stop_rip"], f"0x{GENERATOR.CODE_BASE + 6:016x}")

        offset = GENERATOR.STACK_RSP - GENERATOR.STACK_BASE - 8
        mask = bytes.fromhex(stack["comparison_mask_hex"])
        self.assertEqual(mask[offset:offset + 8], GENERATOR.PUSH_IMAGE_MASK.to_bytes(8, "little"))
        self.assertEqual(mask[offset - 1], 0xFF)
        self.assertEqual(mask[offset + 8], 0xFF)
        initial_stack = bytes.fromhex(stack["initial_bytes_hex"])
        expected_stack = (
            initial_stack[:offset]
            + (GENERATOR.RFLAGS_MASK & 0x00FC_FFFF).to_bytes(8, "little")
            + initial_stack[offset + 8:]
        )
        self.assertEqual(bytes.fromhex(answer["memory"]["stack"]), expected_stack)

    def test_generated_fixture_is_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
