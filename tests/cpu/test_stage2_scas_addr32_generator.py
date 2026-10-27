import hashlib
import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "tests/cpu/generate_stage2_scas_addr32.py"
FIXTURES = ROOT / "tests/fixtures/x86/stage2"

spec = importlib.util.spec_from_file_location("scas_addr32_generator", GENERATOR)
generator = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(generator)


class ScasAddr32GeneratorTests(unittest.TestCase):
    def test_literal_ids_expected_outputs_and_hashes(self):
        expected = json.loads(
            (FIXTURES / "s2_18m_scas_addr32_expected.json").read_text()
        )
        manifest = json.loads(
            (FIXTURES / "s2_18m_scas_addr32_manifest.json").read_text()
        )
        rows = {row[0]: row for row in generator.CASES}
        self.assertEqual(len(rows), 39)
        self.assertEqual(set(rows), set(expected))
        self.assertEqual(set(rows), set(manifest["fixture_ids"]))
        wrap_up_starts = {
            "s2_18m_scasb_wrap_up": 0xFFFFFFFF,
            "s2_18m_scasw_wrap_up": 0xFFFFFFFE,
            "s2_18m_scasd_wrap_up": 0xFFFFFFFC,
            "s2_18m_scasq_wrap_up": 0xFFFFFFF8,
        }
        for case_id, start in wrap_up_starts.items():
            row = rows[case_id]
            self.assertEqual(row[2] & 0xFFFFFFFF, start)
            self.assertEqual(row[3], 0)
            self.assertEqual(row[9], start)
        self.assertEqual(
            manifest["generator_sha256"], hashlib.sha256(GENERATOR.read_bytes()).hexdigest()
        )
        for case_id, row in rows.items():
            path = FIXTURES / f"{case_id}.case.json"
            self.assertEqual(
                manifest["fixture_sha256"][path.name],
                hashlib.sha256(path.read_bytes()).hexdigest(),
            )
            answer = expected[case_id]
            # Expected pointer, flags, definedness, retirement, and bytes are
            # independent literals in CASES and are only serialized here.
            self.assertEqual(answer["gpr"][7], f"0x{row[3]:016x}")
            self.assertEqual(answer["rflags"], f"0x{row[5]:016x}")
            self.assertEqual(answer["rflags_defined"], f"0x{row[6]:016x}")
            self.assertEqual(answer["retired"], row[8])
            self.assertEqual(answer["memory"]["data"], row[10])
            if case_id in generator.HARNESS_ERRORS:
                kind, address, access = generator.HARNESS_ERRORS[case_id]
                self.assertEqual(
                    answer["harness_error"],
                    {
                        "kind": kind,
                        "address": f"0x{address:016x}",
                        "access": access,
                    },
                )
        self.assertEqual(len(generator.HARNESS_ERRORS), 5)

    def test_generator_is_serialization_only_and_fresh(self):
        generated = generator.materialize()
        self.assertEqual(len(generated), 41)
        for name, contents in generated.items():
            self.assertEqual((FIXTURES / name).read_bytes(), contents, name)


if __name__ == "__main__":
    unittest.main()
