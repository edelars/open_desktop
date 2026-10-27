import hashlib
import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "tests/cpu/generate_stage2_stos_addr32.py"
FIXTURES = ROOT / "tests/fixtures/x86/stage2"


spec = importlib.util.spec_from_file_location("stos_addr32_generator", GENERATOR)
generator = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(generator)


class StosAddr32GeneratorTests(unittest.TestCase):
    def test_declared_literal_ids_expected_pointers_and_hashes(self):
        manifest = json.loads((FIXTURES / "s2_18l_stos_addr32_manifest.json").read_text())
        expected = json.loads((FIXTURES / "s2_18l_stos_addr32_expected.json").read_text())
        rows = {row[0]: row for row in generator.CASES}
        self.assertEqual(len(rows), 43)
        self.assertEqual(set(manifest["fixture_ids"]), set(rows))
        self.assertEqual(set(expected), set(rows))
        self.assertEqual(
            manifest["generator_sha256"], hashlib.sha256(GENERATOR.read_bytes()).hexdigest()
        )
        for case_id, row in rows.items():
            path = FIXTURES / f"{case_id}.case.json"
            self.assertEqual(
                manifest["fixture_sha256"][path.name], hashlib.sha256(path.read_bytes()).hexdigest()
            )
            # Row field 3 is a literal, authored output pointer; it is not
            # derived from the opcode, direction flag, or operand width.
            self.assertEqual(expected[case_id]["gpr"][7], f"0x{row[3]:016x}")
            fixture = json.loads(path.read_text())
            self.assertEqual(len(fixture["initial_state"]["gpr"]), 16)
            self.assertEqual(expected[case_id]["retired"], row[8])
            if case_id in generator.HARNESS_ERRORS:
                kind, address, access = generator.HARNESS_ERRORS[case_id]
                self.assertEqual(
                    expected[case_id]["harness_error"],
                    {"kind": kind, "address": f"0x{address:016x}", "access": access},
                )
        self.assertEqual(len(generator.HARNESS_ERRORS), 8)

    def test_generator_is_serialization_only_and_fresh(self):
        generated = generator.materialize()
        self.assertEqual(len(generated), 45)
        for name, contents in generated.items():
            self.assertEqual((FIXTURES / name).read_bytes(), contents, name)


if __name__ == "__main__":
    unittest.main()
