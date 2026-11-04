from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/cpu/generate_stage2_rep_stosb_corpus.py"
SPEC = importlib.util.spec_from_file_location("stage2_rep_stosb_corpus", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)

TEST_BUILD = {
    "revision": "synthetic-revision",
    "compiler": "synthetic-compiler",
    "target": "synthetic-target",
    "profile": module.PROFILE,
}
TEST_MACHINES = {
    "interpreter": {"host_os": "darwin", "host_arch": "arm64"},
    "native_x86": {"host_os": "linux", "host_arch": "x86_64"},
}


def _result(expected: dict, digest: str, backend: str) -> dict:
    return {
        "schema_version": 2,
        "case_id": expected["case_id"],
        "input_sha256": digest,
        "profile": module.PROFILE,
        "backend": backend,
        "outcome": expected["outcome"],
        "instructions_executed": expected["instructions_executed"],
        "build": TEST_BUILD,
        "machine": TEST_MACHINES[backend],
        "snapshot": {
            "status": "available",
            "value": {
                "state": {key: expected["state"][key] for key in ("mode", "gpr", "rip", "rflags")},
                "rflags_defined": expected["state"]["rflags_defined"],
                "memory": expected["memory"],
                "exception": {"status": "available", "value": None},
            },
        },
    }


class RepStosbCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = module.materialize()

    def test_deterministic_manifest_unique_cases_and_full_count_df_matrix(self) -> None:
        self.assertEqual(self.files, module.materialize())
        fixtures = {name: raw for name, raw in self.files.items() if name.endswith(".case.json")}
        self.assertEqual(len(fixtures), module.TOTAL_CASE_COUNT)
        self.assertEqual(len(set(fixtures.values())), module.TOTAL_CASE_COUNT)
        manifest = json.loads(self.files[module.MANIFEST_NAME])
        self.assertEqual(manifest["profile"], module.PROFILE)
        self.assertEqual(manifest["instruction_hex"], "f3aa")
        self.assertEqual(manifest["address_size"], 64)
        self.assertEqual(manifest["architectural_retirements"], 1)
        self.assertEqual(manifest["generated_case_count"], 1024)
        self.assertEqual(manifest["handwritten_case_count"], 36)
        generated = [item for item in manifest["case_digests"] if item["kind"] == "generated"]
        handwritten = [item for item in manifest["case_digests"] if item["kind"] == "handwritten"]
        self.assertEqual(len(generated), 1024)
        self.assertEqual(len(handwritten), 36)
        self.assertEqual({(item["count"], item["df"]) for item in generated},
                         {(count, df) for count in module.COUNTS for df in module.DIRECTIONS})

    def test_independent_oracle_handles_zero_maximum_df_and_fill_boundaries(self) -> None:
        controls = module.handwritten_fixtures()
        by_vector = {item[1]["case"]["vector"]: (item[0], item[1]) for item in controls}
        for df in module.DIRECTIONS:
            zero, zero_expected = by_vector[f"first_count_0_df_{df}"]
            self.assertEqual(zero["initial_state"]["gpr"][7], module._hx(module.UNMAPPED_RDI))
            self.assertEqual(zero_expected["state"]["gpr"][7], module._hx(module.UNMAPPED_RDI))
            self.assertEqual(zero_expected["state"]["gpr"][1], module._hx(0))
            self.assertEqual(zero_expected["memory"][1]["bytes_hex"], zero["memory"][1]["initial_bytes_hex"])

        for df, name in ((0, "first_count_8_df_0"), (0, "last_count_8_df_0"),
                         (1, "first_count_8_df_1"), (1, "last_count_8_df_1"),
                         (0, "unaligned_count_8_df_0"), (1, "unaligned_count_8_df_1")):
            fixture, expected = by_vector[name]
            initial_data = bytes.fromhex(fixture["memory"][1]["initial_bytes_hex"])
            actual_data = bytes.fromhex(expected["memory"][1]["bytes_hex"])
            touched = set(expected["case"]["touched_offsets"])
            fill = int(fixture["initial_state"]["gpr"][0], 16) & 0xFF
            self.assertEqual(len(touched), 8)
            for offset, (before, after) in enumerate(zip(initial_data, actual_data, strict=True)):
                self.assertEqual(after, fill if offset in touched else before)
            self.assertEqual(expected["state"]["gpr"][1], module._hx(0))
            expected_rdi = int(fixture["initial_state"]["gpr"][7], 16) + (8 if df == 0 else -8)
            self.assertEqual(expected["state"]["gpr"][7], module._hx(expected_rdi))
            self.assertEqual(expected["instructions_executed"]["value"], module._hx(1))
            self.assertEqual(expected["state"]["rip"], module._hx(module.CODE_ADDRESS + 2))

    def test_handwritten_vectors_cover_zero_first_last_and_unaligned_spans(self) -> None:
        vectors = module.handwritten_fixtures()
        self.assertEqual(len(vectors), 36)
        labels = {expected["case"]["vector"] for _, expected in vectors}
        self.assertIn("first_count_0_df_0", labels)
        self.assertIn("first_count_0_df_1", labels)
        self.assertIn("first_count_8_df_0", labels)
        self.assertIn("last_count_8_df_0", labels)
        self.assertIn("first_count_8_df_1", labels)
        self.assertIn("last_count_8_df_1", labels)
        self.assertIn("unaligned_count_8_df_0", labels)
        self.assertIn("unaligned_count_8_df_1", labels)

    def test_strict_bundle_verification_rejects_missing_or_tampered_files(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            directory = Path(temporary) / "corpus"
            module.write_directory(directory)
            module.verify_directory(directory)
            fixture = next(directory.glob("*.case.json"))
            fixture.write_bytes(fixture.read_bytes() + b" ")
            with self.assertRaises(module.CorpusError):
                module.verify_directory(directory)
            fixture.unlink()
            with self.assertRaises(module.CorpusError):
                module.verify_directory(directory)

    def test_saved_run_verifier_binds_result_identity_and_backend_provenance(self) -> None:
        fixture, expected = module.generate_fixture(module.FIXED_SEEDS[0], 0)
        fixture_digest = module.sha256(module.json_bytes(fixture))
        case_id = fixture["case_id"]
        manifest = {"profile": module.PROFILE, "corpus_digest": "test-corpus",
                    "case_count": 1, "case_digests": [{"case_id": case_id, "sha256": fixture_digest}]}
        expected_document = {"profile": module.PROFILE, "cases": [expected]}
        inputs = [{"case_id": case_id, "schema_version": 2,
                   "profile": module.PROFILE, "fixture_sha256": fixture_digest}]
        records = []
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            run = Path(temporary)
            for repeat in (1, 2):
                builds = {backend: TEST_BUILD for backend in ("interpreter", "native_x86")}
                machines = TEST_MACHINES
                records.append({
                    "schema_version": 2, "case_id": case_id, "repeat": repeat,
                    "fixture_sha256": fixture_digest,
                    "differences": {"interpreter_vs_native": 0},
                    "stable_repeat": repeat > 1, "build": builds, "machines": machines,
                })
                for backend in ("interpreter", "native_x86"):
                    result = _result(expected, fixture_digest, backend)
                    path = run / f"repeat-{repeat:02d}" / case_id / f"{backend}.json"
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(module.json_bytes(result))
            run_manifest = {"status": "passed", "repeats": 2, "profile": module.PROFILE,
                            "suite": "integer", "inputs": inputs, "fixtures": records}
            summary = module.verify_saved_run(manifest, expected_document, run_manifest, run)
            self.assertEqual(summary["status"], "passed")
            self.assertEqual(summary["native_x86"], "matched_linux_x86_64")

            native_path = run / "repeat-01" / case_id / "native_x86.json"
            valid_native = json.loads(native_path.read_bytes())
            mutations = (
                ("backend", "interpreter"),
                ("input_sha256", "0" * 64),
                ("profile", "wrong-profile"),
                ("schema_version", 1),
            )
            for key, value in mutations:
                invalid = dict(valid_native)
                invalid[key] = value
                native_path.write_bytes(module.json_bytes(invalid))
                with self.assertRaises(module.CorpusError):
                    module.verify_saved_run(manifest, expected_document, run_manifest, run)
            native_path.write_bytes(module.json_bytes(valid_native))

            for section, field in (("build", "revision"), ("machine", "host_os")):
                for value in (True, ""):
                    invalid = dict(valid_native)
                    invalid[section] = dict(valid_native[section], **{field: value})
                    expected_provenance = dict(invalid[section])
                    with self.assertRaises(module.CorpusError):
                        module._verify_result(
                            invalid, expected, fixture_digest, "native_x86",
                            expected_provenance if section == "build" else TEST_BUILD,
                            expected_provenance if section == "machine" else TEST_MACHINES["native_x86"],
                            f"invalid {section} provenance",
                        )

            invalid_record = [dict(records[0], fixture_sha256="0" * 64), records[1]]
            with self.assertRaises(module.CorpusError):
                module.verify_saved_run(manifest, expected_document,
                                        dict(run_manifest, fixtures=invalid_record), run)

    def test_selected_preflight_failure_replaces_stale_success_summary(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            output = root / "oracle"
            module.write_directory(corpus)
            next(corpus.glob("*.case.json")).unlink()
            output.mkdir()
            summary_path = output / "s2-42i-summary.json"
            summary_path.write_text('{"status":"passed"}')
            with patch.object(module, "run_oracle") as run_oracle:
                status = module.main([
                    "--corpus-dir", str(corpus), "--verify", "--run-oracle",
                    "--oracle-output-root", str(output),
                ])
            self.assertEqual(status, 1)
            run_oracle.assert_not_called()
            summary = json.loads(summary_path.read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertNotIn("corpus_digest", summary)
            self.assertIn("missing or unexpected", summary["error"])

    def test_symlink_corpus_preflight_replaces_stale_success_summary(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            corpus_link = root / "corpus-link"
            output = root / "oracle"
            module.write_directory(corpus)
            corpus_link.symlink_to(corpus, target_is_directory=True)
            output.mkdir()
            summary_path = output / "s2-42i-summary.json"
            summary_path.write_text('{"status":"passed"}')
            with patch.object(module, "run_oracle") as run_oracle:
                status = module.main([
                    "--corpus-dir", str(corpus_link), "--verify", "--run-oracle",
                    "--oracle-output-root", str(output),
                ])
            self.assertEqual(status, 1)
            run_oracle.assert_not_called()
            summary = json.loads(summary_path.read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertIn("symlinks", summary["error"])

    def test_missing_native_backend_writes_failed_machine_summary(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            output = root / "oracle"
            module.write_directory(corpus)
            failure = (255, None, "", "native unavailable")
            with patch.object(module, "run_oracle", return_value=failure):
                status = module.main([
                    "--corpus-dir", str(corpus), "--run-oracle",
                    "--oracle-output-root", str(output),
                ])
            self.assertEqual(status, 1)
            summary = json.loads((output / "s2-42i-summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual(summary["oracle_exit_status"], 255)
            self.assertEqual(summary["physical_mac_acceptance"], "not_run")
            self.assertIn("native unavailable", summary["oracle_stderr"])


if __name__ == "__main__":
    unittest.main()
