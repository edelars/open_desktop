import hashlib
import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/cpu/generate_stage2_movs_addr32.py"
SPEC = importlib.util.spec_from_file_location("movs_addr32_generator", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)
FIXTURES = ROOT / "tests/fixtures/x86/stage2"


class MovsAddr32GeneratorTests(unittest.TestCase):
    def test_literal_case_categories_manifest_and_prefix_forms(self):
        rows = {row[0]: row for row in GENERATOR.CASES}
        self.assertEqual(len(rows), 73)
        self.assertEqual(sum(name.startswith("s2_18o_pos_") for name in rows), 8)
        self.assertEqual(sum(name.startswith("s2_18o_unmapped_") for name in rows), 8)
        self.assertEqual(sum(name.startswith("s2_18o_readonly_dst_") for name in rows), 4)
        self.assertEqual(sum(name.startswith("s2_18o_budget0_") for name in rows), 4)
        self.assertEqual(sum(name.startswith("s2_18o_wrap_") for name in rows), 16)
        self.assertEqual(sum(name.startswith("s2_18o_cross4g_") for name in rows), 6)
        self.assertEqual(sum(name.startswith("s2_18o_overlap_") for name in rows), 6)
        self.assertEqual(sum(name.startswith("s2_18o_dual_unmapped_") for name in rows), 4)
        expected = json.loads((FIXTURES / "s2_18o_movs_addr32_expected.json").read_text())
        manifest = json.loads((FIXTURES / "s2_18o_movs_addr32_manifest.json").read_text())
        self.assertEqual(set(expected), set(rows))
        self.assertEqual(len(list(FIXTURES.glob("s2_18o_*.case.json"))), 73)
        self.assertEqual(set(manifest["fixture_ids"]), set(rows))
        self.assertEqual(set(manifest["fixture_files"]), set(manifest["fixture_sha256"]))
        self.assertEqual(manifest["generator_sha256"], hashlib.sha256(SCRIPT.read_bytes()).hexdigest())
        self.assertEqual(expected, GENERATOR.EXPECTED)
        self.assertEqual(
            hashlib.sha256((FIXTURES / manifest["expected_file"]).read_bytes()).hexdigest(),
            manifest["expected_sha256"],
        )
        self.assertIn("s2_18o_normalize_rex67_d", rows)
        self.assertIn("s2_18o_normalize_676648_q", rows)
        self.assertEqual(sum(name.startswith("s2_18o_normalize_") for name in rows), 2)
        self.assertEqual(sum(name.startswith("s2_18o_cld_") or name.startswith("s2_18o_std_") for name in rows), 8)
        for case_id, answer in expected.items():
            if case_id.startswith("s2_18o_dual_unmapped_"):
                self.assertEqual(answer["harness_error"]["address"], "0x0000000000300000")
                self.assertEqual(answer["harness_error"]["access"], "Read")
            if case_id.startswith("s2_18o_readonly_dst_"):
                self.assertEqual(answer["harness_error"]["kind"], "PermissionDenied")
                self.assertEqual(answer["harness_error"]["access"], "Write")
        for filename, digest in manifest["fixture_sha256"].items():
            self.assertEqual(hashlib.sha256((FIXTURES / filename).read_bytes()).hexdigest(), digest)

    def test_generated_fixtures_are_fresh(self):
        generated = GENERATOR.materialize()
        self.assertEqual(sum(name.endswith(".case.json") for name in generated), 73)
        for filename, contents in generated.items():
            path = FIXTURES / filename
            self.assertEqual(path.read_bytes(), contents, filename)


if __name__ == "__main__":
    unittest.main()
