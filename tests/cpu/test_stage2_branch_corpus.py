"""Integrity, matrix, independent-oracle, and evidence checks for S2-42r."""

from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = Path(__file__).with_name("generate_stage2_branch_corpus.py")
sys.path.insert(0, str(SCRIPT.parent))
SPEC = importlib.util.spec_from_file_location("s2_42r", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
s2_42r = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(s2_42r)


BUILD = {"revision": "test-revision", "compiler": "test-compiler", "target": "test-target",
         "profile": s2_42r.PROFILE}
FINGERPRINT = "f" * 64
RUN_REVISION = f"sha256:{FINGERPRINT}"
LOCAL_COMPILER = "rustc local-test-version"
REMOTE_COMPILER = "rustc remote-test-version"
INTERPRETER_MACHINE = {"host_os": "darwin", "host_arch": "arm64"}
NATIVE_MACHINE = {"host_os": "linux", "host_arch": "x86_64"}


def build_for(backend: str, profile: str = "debug") -> dict:
    return {"revision": RUN_REVISION,
            "compiler": LOCAL_COMPILER if backend == "interpreter" else REMOTE_COMPILER,
            "target": "test-target", "profile": profile}


def result_for(fixture_raw: bytes, expected: dict, backend: str = "interpreter") -> dict:
    machine = NATIVE_MACHINE if backend == "native_x86" else INTERPRETER_MACHINE
    state = {key: expected["state"][key] for key in ("mode", "gpr", "rip", "rflags")}
    return {
        "schema_version": 2, "case_id": expected["case_id"],
        "input_sha256": s2_42r.sha256(fixture_raw), "profile": s2_42r.PROFILE, "backend": backend,
        "outcome": expected["outcome"], "instructions_executed": expected["instructions_executed"],
        "build": BUILD, "machine": machine,
        "snapshot": {"status": "available", "value": {
            "state": state, "rflags_defined": expected["state"]["rflags_defined"],
            "memory": copy.deepcopy(expected["memory"]),
            "exception": {"status": "available", "value": None},
        }},
    }


class BranchCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = s2_42r.materialize()
        cls.expected = json.loads(cls.files[s2_42r.EXPECTED_NAME])["cases"]

    def test_exact_1056_case_matrix_and_literal_control_snapshots(self) -> None:
        self.assertNotEqual(s2_42r.DEFAULT_CORPUS, s2_42r.DEFAULT_RESULTS.parent)
        names = [name for name in self.files if name.endswith(".case.json")]
        self.assertEqual(len(names), 1056)
        self.assertEqual(len(set(names)), 1056)
        self.assertEqual(len({self.files[name] for name in names}), 1056)
        manifest = json.loads(self.files[s2_42r.MANIFEST_NAME])
        self.assertEqual((manifest["generated_case_count"], manifest["handwritten_case_count"],
                          manifest["case_count"]), (1024, 32, 1056))
        self.assertEqual(manifest["generated_state_variation"], {
            "gprs": "all-16-seeded", "non_zf_defined_flags": "seeded", "entry_offsets": [128, 192]})
        by_id = {item["case_id"]: item for item in self.expected}
        pinned_controls = json.loads(s2_42r.CONTROL_EXPECTED_PATH.read_bytes())["cases"]
        self.assertEqual(s2_42r.sha256(s2_42r.CONTROL_EXPECTED_PATH.read_bytes()),
                         s2_42r.CONTROL_EXPECTED_DOCUMENT_SHA256)
        self.assertEqual(len(pinned_controls), 32)
        self.assertEqual({item["case_id"] for item in pinned_controls},
                         {identifier for identifier in by_id if identifier.startswith("s2_42r_hand_")})
        for item in pinned_controls:
            identifier = item["case_id"]
            self.assertEqual(by_id[identifier], item)
            self.assertEqual(self.files[f"{identifier}.case.json"],
                             (s2_42r.CONTROL_FIXTURE_DIR / f"{identifier}.case.json").read_bytes())

        counts: dict[tuple[int, bool, int], int] = {}
        for seed in s2_42r.SEEDS:
            for row in s2_42r.matrix_rows(seed):
                opcode, zf, displacement, entry_offset = row
                self.assertIn(entry_offset, s2_42r.ENTRY_OFFSETS)
                cell = (opcode, zf, displacement)
                counts[cell] = counts.get(cell, 0) + 1
            self.assertEqual(len(s2_42r.matrix_rows(seed)), 256)
            per_seed = {(op, zf, disp): 0 for op in s2_42r.OPCODES
                        for zf in (False, True) for disp in s2_42r.DISPLACEMENTS}
            for op, zf, disp, _ in s2_42r.matrix_rows(seed):
                per_seed[(op, zf, disp)] += 1
            self.assertEqual(set(per_seed.values()), {8})
        self.assertEqual(set(counts), {(op, zf, disp) for op in s2_42r.OPCODES
                                       for zf in (False, True) for disp in s2_42r.DISPLACEMENTS})
        generated_states = []
        generated_offsets = set()
        for item in manifest["case_digests"]:
            if item["kind"] != "generated":
                continue
            fixture = json.loads(self.files[f"{item['case_id']}.case.json"])
            initial = fixture["initial_state"]
            generated_states.append((tuple(initial["gpr"]), initial["rflags"], initial["rip"]))
            generated_offsets.add(int(initial["rip"], 16) - s2_42r.CODE_ADDRESS)
        self.assertEqual(len(generated_states), 1024)
        self.assertEqual(len(set(generated_states)), 1024)
        self.assertEqual(generated_offsets, set(s2_42r.ENTRY_OFFSETS))
        generated_fixture_rows = [json.loads(self.files[f"{item['case_id']}.case.json"])
                                  for item in manifest["case_digests"] if item["kind"] == "generated"]
        for register_index in range(16):
            self.assertGreater(len({row["initial_state"]["gpr"][register_index]
                                    for row in generated_fixture_rows}), 900)
        self.assertGreater(len({row["initial_state"]["rflags"] for row in generated_fixture_rows}), 2)

    def test_generated_fixture_and_manifest_are_exact_and_tamper_evident(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "corpus"
            s2_42r.write_file_set(directory, s2_42r.materialize())
            s2_42r.verify_corpus(directory)
            tampered = directory / "manifest.json"
            tampered.write_bytes(tampered.read_bytes().replace(b'"matrix_repetitions": 8',
                                                                 b'"matrix_repetitions": 7'))
            with self.assertRaises(s2_42r.CorpusError):
                s2_42r.verify_corpus(directory)

    def test_independent_oracle_rejects_condition_base_signedness_retirement_and_equal_bad_backends(self) -> None:
        identifier = s2_42r.control_id(0x74, True, -128)
        fixture_raw = self.files[f"{identifier}.case.json"]
        expected = next(item for item in self.expected if item["case_id"] == identifier)
        fixture = json.loads(fixture_raw)
        entry = int(fixture["initial_state"]["rip"], 16)
        raw_disp = int(fixture["memory"][0]["initial_bytes_hex"][2 * (entry - s2_42r.CODE_ADDRESS + 1):
                                                                       2 * (entry - s2_42r.CODE_ADDRESS + 2)], 16)
        cases = {
            "wrong condition": entry + 2,
            "instruction-start displacement base": entry - 128,
            "unsigned displacement": entry + 2 + raw_disp,
        }
        for label, bad_rip in cases.items():
            with self.subTest(label=label):
                bad = result_for(fixture_raw, expected)
                bad["snapshot"]["value"]["state"]["rip"] = s2_42r.hx(bad_rip)
                with self.assertRaises(s2_42r.CorpusError):
                    s2_42r.compare_result(bad, fixture_raw, expected, "interpreter", BUILD,
                                          INTERPRETER_MACHINE, label)
        bad_retirement = result_for(fixture_raw, expected)
        bad_retirement["instructions_executed"] = {"status": "available", "value": s2_42r.hx(2)}
        with self.assertRaises(s2_42r.CorpusError):
            s2_42r.compare_result(bad_retirement, fixture_raw, expected, "interpreter", BUILD,
                                  INTERPRETER_MACHINE, "bad retirement")
        # Equal interpreter/native errors still fail because each backend is checked against the oracle.
        for backend, machine in (("interpreter", INTERPRETER_MACHINE), ("native_x86", NATIVE_MACHINE)):
            bad = result_for(fixture_raw, expected, backend)
            bad["snapshot"]["value"]["state"]["rip"] = s2_42r.hx(entry + 2)
            with self.assertRaises(s2_42r.CorpusError):
                s2_42r.compare_result(bad, fixture_raw, expected, backend, BUILD, machine,
                                      "equal bad backend result")

    def test_runner_build_configuration_is_distinct_from_cpu_profile(self) -> None:
        for configuration in ("debug", "release"):
            with self.subTest(configuration=configuration):
                identity = s2_42r.validate_run_build(build_for("interpreter", configuration),
                    RUN_REVISION, LOCAL_COMPILER, "build profile")
                self.assertEqual(identity[3], configuration)
        with self.assertRaisesRegex(s2_42r.CorpusError, "configuration identity"):
            s2_42r.validate_run_build(build_for("interpreter", s2_42r.PROFILE),
                RUN_REVISION, LOCAL_COMPILER, "build profile")

    def test_saved_local_only_evidence_and_native_evidence_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            corpus = Path(temporary) / "corpus"
            s2_42r.write_file_set(corpus, self.files)
            bad_saved = Path(temporary) / "results.json"
            bad_saved.write_bytes(s2_42r.json_bytes({
                "schema_version": 1, "status": "passed", "backend": "interpreter",
                "physical_native_acceptance": "not_run", "profile": s2_42r.PROFILE,
                "manifest_sha256": s2_42r.sha256((corpus / s2_42r.MANIFEST_NAME).read_bytes()),
                "case_count": 1056, "repeats_per_case": 2, "failures": [], "results": [],
                "runner_build": BUILD, "runner_machine": INTERPRETER_MACHINE,
            }))
            with self.assertRaises(s2_42r.CorpusError):
                s2_42r.verify_results(corpus, bad_saved)

            with tempfile.TemporaryDirectory() as temporary_run:
                output = Path(temporary_run) / "oracle"
                argv = ["generate_stage2_branch_corpus.py", "--run-oracle", "--corpus", str(corpus),
                        "--oracle-output-root", str(output)]
                with patch.object(s2_42r.sys, "argv", argv), patch.object(
                    s2_42r, "run_native_oracle", return_value=(255, None, "", "native unavailable")
                ):
                    self.assertEqual(s2_42r.main(), 1)
                summary = json.loads((output / "s2-42r-summary.json").read_bytes())
                self.assertEqual(summary["status"], "failed")
                self.assertEqual(summary["physical_mac_acceptance"], "not_run")
                self.assertEqual(summary["native_acceptance"], "failed")
                self.assertEqual(summary["oracle_stderr"], "native unavailable")

    def test_saved_results_require_execution_identity_bound_to_runner_provenance(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            corpus = Path(temporary) / "corpus"
            s2_42r.write_file_set(corpus, self.files)
            base = {
                "schema_version": 1, "status": "passed", "backend": "interpreter",
                "physical_native_acceptance": "not_run", "profile": s2_42r.PROFILE,
                "manifest_sha256": s2_42r.sha256((corpus / s2_42r.MANIFEST_NAME).read_bytes()),
                "case_count": 1056, "repeats_per_case": 2, "failures": [], "results": [],
                "runner_build": BUILD, "runner_machine": INTERPRETER_MACHINE,
                "execution_identity": {"executable_sha256": "a" * 64, "revision": BUILD["revision"]},
                "execution_machine": dict(INTERPRETER_MACHINE),
            }
            variants = [
                ("missing execution_identity", "execution_identity", None, "executable identity"),
                ("missing execution_machine", "execution_machine", None, "machine identity"),
            ]
            mismatch_revision = copy.deepcopy(base)
            mismatch_revision["execution_identity"]["revision"] = "other-revision"
            variants.append(("mismatched revision", None, mismatch_revision, "differs from runner result"))
            mismatch_machine = copy.deepcopy(base)
            mismatch_machine["execution_machine"]["host_arch"] = "arm64-other"
            variants.append(("mismatched machine", None, mismatch_machine, "differs from runner result"))
            for label, field, record, message in variants:
                with self.subTest(label=label):
                    candidate = copy.deepcopy(base) if record is None else record
                    if field is not None:
                        candidate.pop(field)
                    path = Path(temporary) / f"{label.replace(' ', '-')}.json"
                    path.write_bytes(s2_42r.json_bytes(candidate))
                    with self.assertRaisesRegex(s2_42r.CorpusError, message):
                        s2_42r.verify_results(corpus, path)

    @staticmethod
    def _native_run_manifest(corpus: Path, run: Path, *, include_frozen: bool,
                             include_records: bool, include_results: bool = False) -> dict:
        corpus_manifest = s2_42r.verify_corpus(corpus)
        frozen = run / "fixtures"
        if include_frozen:
            frozen.mkdir(parents=True)
        inputs = []
        for item in corpus_manifest["case_digests"]:
            identifier = item["case_id"]
            name = f"{identifier}.case.json"
            source = corpus / name
            target = frozen / name
            if include_frozen:
                shutil.copyfile(source, target)
            inputs.append({"case_id": identifier, "filename": name,
                           "fixture_path": str(target.resolve().relative_to(s2_42r.ROOT)),
                           "fixture_sha256": item["fixture_sha256"], "profile": s2_42r.PROFILE,
                           "schema_version": 2})
        records = []
        expected_by_id = {item["case_id"]: item for item in
                          json.loads((corpus / s2_42r.EXPECTED_NAME).read_bytes())["cases"]}
        if include_records:
            for repeat in (1, 2):
                for item in corpus_manifest["case_digests"]:
                    builds = {backend: build_for(backend) for backend in ("interpreter", "native_x86")}
                    machines = {"interpreter": INTERPRETER_MACHINE, "native_x86": NATIVE_MACHINE}
                    records.append({
                        "case_id": item["case_id"], "repeat": repeat,
                        "fixture_sha256": item["fixture_sha256"],
                        "differences": {"interpreter_vs_native": 0},
                        "stable_repeat": repeat > 1,
                        "build": builds, "machines": machines,
                    })
                    if include_results:
                        case_dir = run / f"repeat-{repeat:02d}" / item["case_id"]
                        case_dir.mkdir(parents=True)
                        raw = (corpus / f"{item['case_id']}.case.json").read_bytes()
                        for backend in ("interpreter", "native_x86"):
                            observed = result_for(raw, expected_by_id[item["case_id"]], backend)
                            observed["build"] = builds[backend]
                            observed["machine"] = machines[backend]
                            (case_dir / f"{backend}.json").write_bytes(s2_42r.json_bytes(observed))
                            (case_dir / f"{backend}.process.json").write_bytes(
                                s2_42r.json_bytes({"returncode": 0}))
        return {"status": "passed", "suite": "integer", "run_id": "test-run", "profile": s2_42r.PROFILE,
                "repeats": 2, "source_fingerprint": FINGERPRINT, "commands": [{"returncode": 0}],
                "machines": {"local": {"rustc": LOCAL_COMPILER, "host_os": "darwin", "host_arch": "arm64"},
                             "remote_rustc": REMOTE_COMPILER, "remote_cargo": "cargo remote-test-version",
                             "remote_uname": "Linux x86_64 test"},
                "inputs": inputs, "fixtures": records}

    def test_native_verifier_rejects_process_status_bad_commands_nonobject_manifest_and_missing_frozen_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary).resolve() / "repo"
            corpus, run = repo / "corpus", repo / "run"
            run.mkdir(parents=True)
            s2_42r.write_file_set(corpus, self.files)
            with patch.object(s2_42r, "ROOT", repo):
                manifest_path = run / "manifest.json"
                manifest_path.write_text("[]")
                with self.assertRaisesRegex(s2_42r.CorpusError, "must be an object"):
                    s2_42r.verify_native_run(corpus, run, 0)
                failed_command_manifest = {
                    "status": "passed", "suite": "integer", "run_id": "test-run",
                    "profile": s2_42r.PROFILE, "repeats": 2, "source_fingerprint": "f" * 64,
                    "inputs": [], "fixtures": [],
                }
                with self.assertRaisesRegex(s2_42r.CorpusError, "process did not exit successfully"):
                    manifest_path.write_bytes(s2_42r.json_bytes(failed_command_manifest))
                    s2_42r.verify_native_run(corpus, run, 255)
                for commands in ([{"returncode": 255}], [{"returncode": None, "error": "timed out"}]):
                    failed_command_manifest["commands"] = commands
                    manifest_path.write_bytes(s2_42r.json_bytes(failed_command_manifest))
                    with self.assertRaisesRegex(s2_42r.CorpusError, "command evidence"):
                        s2_42r.verify_native_run(corpus, run, 0)
                manifest_path.write_bytes(s2_42r.json_bytes(self._native_run_manifest(
                    corpus, run, include_frozen=False, include_records=False)))
                with self.assertRaisesRegex(s2_42r.CorpusError, "frozen fixture set"):
                    s2_42r.verify_native_run(corpus, run, 0)

    def test_native_verifier_rejects_missing_backend_process_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary).resolve() / "repo"
            corpus, run = repo / "corpus", repo / "run"
            run.mkdir(parents=True)
            s2_42r.write_file_set(corpus, self.files)
            with patch.object(s2_42r, "ROOT", repo):
                manifest_path = run / "manifest.json"
                manifest_path.write_bytes(s2_42r.json_bytes(self._native_run_manifest(
                    corpus, run, include_frozen=True, include_records=True)))
                with self.assertRaisesRegex(s2_42r.CorpusError, "process evidence is missing"):
                    s2_42r.verify_native_run(corpus, run, 0)
                first_id = s2_42r.verify_corpus(corpus)["case_digests"][0]["case_id"]
                case_dir = run / "repeat-01" / first_id
                case_dir.mkdir(parents=True)
                (case_dir / "interpreter.process.json").write_bytes(s2_42r.json_bytes({"returncode": 255}))
                with self.assertRaisesRegex(s2_42r.CorpusError, "did not exit successfully"):
                    s2_42r.verify_native_run(corpus, run, 0)

    def test_native_verifier_rejects_frozen_fixture_byte_or_digest_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary).resolve() / "repo"
            corpus, run = repo / "corpus", repo / "run"
            run.mkdir(parents=True)
            s2_42r.write_file_set(corpus, self.files)
            with patch.object(s2_42r, "ROOT", repo):
                run_manifest = self._native_run_manifest(corpus, run, include_frozen=True, include_records=False)
                (run / "manifest.json").write_bytes(s2_42r.json_bytes(run_manifest))
                first = run_manifest["inputs"][0]
                frozen = run / "fixtures" / first["filename"]
                raw = frozen.read_bytes()
                frozen.write_bytes(raw + b" ")
                with self.assertRaisesRegex(s2_42r.CorpusError, "frozen fixture differs"):
                    s2_42r.verify_native_run(corpus, run, 0)
                frozen.write_bytes(raw)
                first["fixture_sha256"] = "0" * 64
                (run / "manifest.json").write_bytes(s2_42r.json_bytes(run_manifest))
                with self.assertRaisesRegex(s2_42r.CorpusError, "frozen fixture differs"):
                    s2_42r.verify_native_run(corpus, run, 0)

    def test_native_evidence_roundtrip_and_run_level_provenance_mutations(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary).resolve() / "repo"
            corpus, run = repo / "corpus", repo / "run"
            run.mkdir(parents=True)
            s2_42r.write_file_set(corpus, self.files)
            with patch.object(s2_42r, "ROOT", repo):
                good = self._native_run_manifest(corpus, run, include_frozen=True,
                                                 include_records=True, include_results=True)
                manifest_path = run / "manifest.json"
                manifest_path.write_bytes(s2_42r.json_bytes(good))
                accepted = s2_42r.verify_native_run(corpus, run, 0)
                self.assertEqual(accepted["status"], "passed")
                self.assertEqual(accepted["native_acceptance"], "passed")
                self.assertEqual(accepted["results_per_backend"], 2112)

                first_id = good["inputs"][0]["case_id"]
                first_result = run / "repeat-01" / first_id / "interpreter.json"
                original_result = first_result.read_bytes()
                wrong_cpu_profile = json.loads(original_result)
                wrong_cpu_profile["profile"] = "debug"
                first_result.write_bytes(s2_42r.json_bytes(wrong_cpu_profile))
                with self.assertRaisesRegex(s2_42r.CorpusError, "profile differs"):
                    s2_42r.verify_native_run(corpus, run, 0)
                first_result.write_bytes(original_result)

                mutations = []
                missing_fingerprint = copy.deepcopy(good)
                missing_fingerprint.pop("source_fingerprint")
                mutations.append(("missing fingerprint", missing_fingerprint,
                                  "did not pass two repeats"))
                malformed_fingerprint = copy.deepcopy(good)
                malformed_fingerprint["source_fingerprint"] = "not-a-sha256"
                mutations.append(("malformed fingerprint", malformed_fingerprint,
                                  "did not pass two repeats"))
                wrong_fingerprint = copy.deepcopy(good)
                wrong_fingerprint["source_fingerprint"] = "0" * 64
                mutations.append(("wrong fingerprint", wrong_fingerprint,
                                  "build differs from run-level"))
                revision_mismatch = copy.deepcopy(good)
                revision_mismatch["fixtures"][0]["build"]["interpreter"]["revision"] = "sha256:" + "0" * 64
                mutations.append(("revision mismatch", revision_mismatch, "build differs from run-level"))
                missing_local_compiler = copy.deepcopy(good)
                missing_local_compiler["machines"]["local"].pop("rustc")
                mutations.append(("missing local compiler", missing_local_compiler,
                                  "run-level local/remote compiler identity"))
                missing_remote_compiler = copy.deepcopy(good)
                missing_remote_compiler["machines"].pop("remote_rustc")
                mutations.append(("missing remote compiler", missing_remote_compiler,
                                  "run-level local/remote compiler identity"))
                changed_remote_compiler = copy.deepcopy(good)
                changed_remote_compiler["machines"]["remote_rustc"] = "rustc changed-version"
                mutations.append(("changed remote compiler", changed_remote_compiler,
                                  "build differs from run-level"))
                for label, changed, message in mutations:
                    with self.subTest(label=label):
                        manifest_path.write_bytes(s2_42r.json_bytes(changed))
                        with self.assertRaisesRegex(s2_42r.CorpusError, message):
                            s2_42r.verify_native_run(corpus, run, 0)

    def test_native_main_writes_failed_summary_before_preflight(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            corpus = Path(temporary) / "corpus"
            s2_42r.write_file_set(corpus, self.files)
            (corpus / s2_42r.MANIFEST_NAME).unlink()
            output = Path(temporary) / "oracle"
            argv = ["generate_stage2_branch_corpus.py", "--run-oracle", "--corpus", str(corpus),
                    "--oracle-output-root", str(output)]
            with patch.object(s2_42r.sys, "argv", argv), patch.object(s2_42r, "run_native_oracle") as native:
                self.assertEqual(s2_42r.main(), 1)
            native.assert_not_called()
            summary = json.loads((output / "s2-42r-summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual(summary["native_acceptance"], "not_run")
            self.assertIn("missing or unexpected", summary["error"])

    def test_native_run_manifest_missing_cases_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary).resolve() / "repo"
            corpus = repo / "corpus"
            s2_42r.write_file_set(corpus, self.files)
            run = repo / "run"
            run.mkdir()
            with patch.object(s2_42r, "ROOT", repo):
                incomplete = self._native_run_manifest(corpus, run, include_frozen=False, include_records=False)
                incomplete["inputs"] = []
                (run / "manifest.json").write_bytes(s2_42r.json_bytes(incomplete))
                with self.assertRaisesRegex(s2_42r.CorpusError, "omitted or malformed frozen inputs"):
                    s2_42r.verify_native_run(corpus, run, 0)


if __name__ == "__main__":
    unittest.main()
