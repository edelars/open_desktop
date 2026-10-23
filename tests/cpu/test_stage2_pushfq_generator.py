"""Structural checks for the literal S2-17p PUSHFQ fixture."""
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("generate_stage2_pushfq.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_pushfq", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)


class PushfqGeneratorTests(unittest.TestCase):
    def test_literal_encoding_stack_effect_and_architectural_comparison_mask(self) -> None:
        files = GENERATOR.materialize()
        case_id = "s2_17p_pushfq_masked_rflags_image"
        fixture = json.loads(files[f"{case_id}.case.json"])
        answer = json.loads(files["s2_17p_pushfq_expected.json"])[case_id]
        code = next(region for region in fixture["memory"] if region["region_id"] == "code")
        stack = next(region for region in fixture["memory"] if region["region_id"] == "stack")
        self.assertEqual(code["initial_bytes_hex"], "9c")
        self.assertEqual(answer["gpr"][4], f"0x{GENERATOR.STACK_RSP - 8:016x}")
        self.assertEqual(answer["rflags"], f"0x{GENERATOR.RFLAGS_MASK:016x}")
        self.assertEqual(answer["rflags_defined"], f"0x{GENERATOR.RFLAGS_MASK:016x}")
        offset = GENERATOR.STACK_RSP - GENERATOR.STACK_BASE - 8
        self.assertEqual(
            bytes.fromhex(stack["comparison_mask_hex"])[offset:offset + 8],
            GENERATOR.PUSH_IMAGE_MASK.to_bytes(8, "little"),
        )

    def test_generated_fixture_is_fresh(self) -> None:
        for name, content in GENERATOR.materialize().items():
            path = GENERATOR.DEST / name
            if path.exists():
                self.assertEqual(path.read_bytes(), content, name)


if __name__ == "__main__":
    unittest.main()
