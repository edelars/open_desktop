import hashlib
import importlib.util
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/cpu/generate_stage2_cmps_addr32.py"
SPEC = importlib.util.spec_from_file_location("cmps_addr32_generator", SCRIPT)
GENERATOR = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(GENERATOR)
FIXTURES = ROOT / "tests/fixtures/x86/stage2"


class CmpsAddr32GeneratorTests(unittest.TestCase):
    def test_literal_case_categories_and_manifest(self):
        rows = {row[0]: row for row in GENERATOR.CASES}
        self.assertEqual(len(rows), 63)
        self.assertEqual(sum(case_id.startswith("s2_18n_pos_") for case_id in rows), 8)
        self.assertEqual(sum(case_id.startswith("s2_18n_unmapped_") for case_id in rows), 8)
        self.assertEqual(sum(case_id.startswith("s2_18n_budget0_") for case_id in rows), 4)
        self.assertEqual(sum(case_id.startswith("s2_18n_wrap_") for case_id in rows), 16)
        self.assertEqual(sum(case_id.startswith("s2_18n_cross4g_") for case_id in rows), 6)
        self.assertEqual(sum(case_id.startswith("s2_18n_normalize_") for case_id in rows), 2)
        self.assertEqual(sum(case_id.startswith("s2_18n_test_") for case_id in rows), 3)
        self.assertEqual(sum(case_id.startswith(("s2_18n_cld_", "s2_18n_std_")) for case_id in rows), 8)
        self.assertEqual(sum(case_id.startswith("s2_18n_alias_") for case_id in rows), 4)
        self.assertEqual(sum(case_id.startswith("s2_18n_dual_unmapped_") for case_id in rows), 4)
        expected = json.loads((FIXTURES / "s2_18n_cmps_addr32_expected.json").read_text())
        manifest = json.loads((FIXTURES / "s2_18n_cmps_addr32_manifest.json").read_text())
        explicit = json.loads((ROOT / "work/s2-18n-files.json").read_text())
        self.assertEqual(set(expected), set(rows))
        self.assertEqual(set(manifest["fixture_ids"]), set(rows))
        self.assertEqual(set(explicit["fixture_files"]), set(manifest["fixture_sha256"]))
        self.assertEqual(manifest["generator_sha256"], hashlib.sha256(SCRIPT.read_bytes()).hexdigest())
        for case_id, row in rows.items():
            answer = expected[case_id]
            width, esi, edi, df, outcome = row[2], row[3], row[4], row[5], row[6]
            if outcome == "stop_address":
                step = -width if df else width
                self.assertEqual(answer["gpr"][6], f"0x{((esi + step) & 0xffffffff):016x}")
                self.assertEqual(answer["gpr"][7], f"0x{((edi + step) & 0xffffffff):016x}")
                self.assertEqual(answer["rflags_defined"], f"0x{GENERATOR.ARITHMETIC_DEFINED:016x}")
            if "dual_unmapped" in case_id:
                self.assertEqual(answer["harness_error"]["address"], f"0x{esi:016x}")
        hashes = manifest["fixture_sha256"]
        for filename, digest in hashes.items():
            self.assertEqual(hashlib.sha256((FIXTURES / filename).read_bytes()).hexdigest(), digest)

    def test_generated_fixtures_are_fresh(self):
        generated = GENERATOR.materialize()
        self.assertEqual(len(generated), 66)
        for filename, contents in generated.items():
            path = ROOT / filename if filename.startswith("work/") else FIXTURES / filename
            self.assertEqual(path.read_bytes(), contents, filename)


if __name__ == "__main__":
    unittest.main()
