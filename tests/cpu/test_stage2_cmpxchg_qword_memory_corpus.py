"""Strict controls, integer oracle, local evidence, and native-denial checks for S2-42u."""

from __future__ import annotations

import contextlib
import copy
from collections import Counter
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
from stage2_corpus_support import CorpusError, json_bytes, sha256, strict_object

SCRIPT = Path(__file__).with_name("generate_stage2_cmpxchg_qword_memory_corpus.py")
SPEC = importlib.util.spec_from_file_location("generate_stage2_cmpxchg_qword_memory_corpus", SCRIPT)
corpus = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(corpus)


BUILD = {"revision": "1" * 40, "compiler": "rustc test", "target": "aarch64-apple-darwin", "profile": "debug"}
MACHINE = {"host_os": "macos", "host_arch": "aarch64"}


def result_record(fixture_raw: bytes, expected: dict, backend: str = "interpreter") -> dict:
    return {
        "schema_version": 2,
        "case_id": expected["case_id"],
        "input_sha256": sha256(fixture_raw),
        "profile": corpus.PROFILE,
        "backend": backend,
        "outcome": expected["outcome"],
        "instructions_executed": expected["instructions_executed"],
        "build": BUILD,
        "machine": MACHINE,
        "snapshot": {"status": "available", "value": {
            "state": {key: expected["state"][key] for key in ("mode", "gpr", "rip", "rflags")},
            "rflags_defined": expected["state"]["rflags_defined"],
            "memory": expected["memory"],
            "exception": {"status": "available", "value": None},
        }},
    }


class CmpxchgMemoryCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = corpus.materialize()
        cls.manifest = json.loads(cls.files[corpus.MANIFEST_NAME], object_pairs_hook=strict_object)
        cls.expected = json.loads(cls.files[corpus.EXPECTED_NAME], object_pairs_hook=strict_object)["cases"]
        cls.expected_by_id = {row["case_id"]: row for row in cls.expected}

    def _copy_bundle(self, target: Path) -> None:
        target.mkdir()
        for name, raw in self.files.items():
            (target / name).write_bytes(raw)

    def test_static_controls_pin_equal_and_mismatch_full_snapshots(self) -> None:
        controls, expected_raw, _ = corpus.load_controls()
        self.assertEqual(len(controls), 2)
        self.assertEqual(len(json.loads(expected_raw)["cases"]), 2)
        self.assertEqual([row[0] for row in controls],
                         [corpus.control_id(pair, 0) for pair in range(2)])
        for pair, memory_value in enumerate((corpus.OPERAND_ADDRESS, 0x1122334455667788)):
            identifier = corpus.control_id(pair, 0)
            fixture = json.loads(controls[pair][1])
            expected = self.expected_by_id[identifier]
            self.assertEqual(bytes.fromhex(fixture["memory"][0]["initial_bytes_hex"]), bytes.fromhex("480fb118"))
            self.assertEqual(fixture["memory"][1]["region_id"], "data_left")
            self.assertEqual(fixture["memory"][2]["region_id"], "data_right")
            self.assertEqual(int(fixture["initial_state"]["rflags"], 16), 0x0CD7)
            self.assertEqual(int(fixture["initial_state"]["gpr"][0], 16), corpus.OPERAND_ADDRESS)
            initial = bytes.fromhex(fixture["memory"][1]["initial_bytes_hex"] + fixture["memory"][2]["initial_bytes_hex"])
            self.assertEqual(int.from_bytes(initial[corpus.OPERAND_OFFSET:corpus.OPERAND_OFFSET + 8], "little"), memory_value)
            self.assertEqual(expected["state"]["gpr"][0], corpus.hx(corpus.OPERAND_ADDRESS if pair == 0 else memory_value))
            self.assertEqual(len(bytes.fromhex(expected["memory"][1]["bytes_hex"])), corpus.DATA_LEFT_LENGTH)
            self.assertEqual(len(bytes.fromhex(expected["memory"][2]["bytes_hex"])), corpus.DATA_RIGHT_LENGTH)
            self.assertEqual(expected["memory"][0]["bytes_hex"], "480fb118")

    def test_generated_inputs_are_deterministic_and_unique(self) -> None:
        first = corpus.generated_fixtures()
        second = corpus.generated_fixtures()
        self.assertEqual(first, second)
        self.assertEqual(len(first), 1024)
        self.assertEqual([item["case_id"] for item in first[:2]], [corpus.generated_id(0, 0), corpus.generated_id(0, 1)])
        tags = {item["initial_state"]["gpr"][15] for item in first}
        self.assertEqual(len(tags), 1024)
        self.assertEqual(sum(int(item["initial_state"]["gpr"][15], 16) >> 32 & 0xFF == 1 for item in first), 256)
        self.assertEqual({int(item["initial_state"]["gpr"][0], 16) for item in first}, {corpus.OPERAND_ADDRESS})
        memory_counts = Counter()
        flag_classes = Counter()
        for fixture in first:
            self.assertEqual(fixture["memory"][0]["initial_bytes_hex"], "480fb118")
            self.assertEqual(fixture["memory"][0]["permissions"], {"read": False, "write": False, "execute": True})
            self.assertEqual(fixture["memory"][1]["permissions"], {"read": True, "write": True, "execute": False})
            self.assertEqual(fixture["memory"][2]["permissions"], {"read": True, "write": True, "execute": False})
            self.assertEqual(int(fixture["initial_state"]["rflags"], 16) & ~corpus.RFLAGS_DEFINED, 0)
            memory = bytes.fromhex(fixture["memory"][1]["initial_bytes_hex"] + fixture["memory"][2]["initial_bytes_hex"])
            old_memory = int.from_bytes(memory[corpus.OPERAND_OFFSET:corpus.OPERAND_OFFSET + 8], "little")
            memory_counts[old_memory] += 1
            flags = int(corpus.expected_snapshot(fixture)["state"]["rflags"], 16)
            flag_classes[(bool(flags & 1), bool(flags & 64), bool(flags & 0x800))] += 1
        self.assertEqual(set(memory_counts), {value for _, value in corpus.GENERATED_MEMORY_CLASSES})
        self.assertEqual(set(memory_counts.values()), {128})
        self.assertEqual(flag_classes, Counter({
            (False, True, False): 128,
            (False, False, False): 384,
            (True, False, False): 384,
            (True, False, True): 128,
        }))

    def test_order_manifest_and_digest_bind_controls_before_generated(self) -> None:
        ids = [item["case_id"] for item in self.manifest["case_digests"]]
        expected_controls = [corpus.control_id(pair, 0) for pair in range(2)]
        expected_generated = [corpus.generated_id(seed_index, sequence)
                              for seed_index in range(4) for sequence in range(256)]
        self.assertEqual(ids, expected_controls + expected_generated)
        self.assertEqual(self.manifest["case_count"], 1026)
        self.assertEqual(self.manifest["control_count"], 2)
        self.assertEqual(self.manifest["generated_case_count"], 1024)
        self.assertEqual(self.manifest["control_manifest_sha256"], corpus.CONTROL_MANIFEST_SHA256)
        self.assertEqual(self.manifest["corpus_digest"], sha256("\n".join(row["fixture_sha256"] for row in self.manifest["case_digests"]).encode()))

    def test_strict_bundle_rejects_missing_extra_symlink_tamper_and_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp) / "bundle"
            self._copy_bundle(base)
            corpus.verify_corpus(base)
            (base / "s2_42u_cmpxchg_qword_memory_equal.case.json").unlink()
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(base)
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp) / "bundle"
            self._copy_bundle(base)
            (base / "unexpected.txt").write_text("extra")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(base)
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp) / "bundle"
            self._copy_bundle(base)
            target = base / "s2_42u_cmpxchg_qword_memory_equal.case.json"
            target.unlink()
            target.symlink_to(Path(__file__))
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(base)
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp) / "bundle"
            self._copy_bundle(base)
            target = base / "s2_42u_cmpxchg_qword_memory_equal.case.json"
            target.write_bytes(target.read_bytes() + b" ")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(base)
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp) / "bundle"
            self._copy_bundle(base)
            manifest = json.loads((base / corpus.MANIFEST_NAME).read_bytes())
            manifest["case_digests"][0]["case_id"] = "../escape"
            (base / corpus.MANIFEST_NAME).write_bytes(json_bytes(manifest))
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(base)

    def test_duplicate_json_keys_are_rejected(self) -> None:
        with self.assertRaises(CorpusError):
            json.loads('{"case_id":"a","case_id":"b"}', object_pairs_hook=strict_object)

    def test_result_comparison_rejects_wrong_flags_rax_canary_code_and_retirement(self) -> None:
        identifier = corpus.control_id(3, 1)
        fixture_raw = corpus.control_inputs()[f"{identifier}.case.json"]
        expected = self.expected_by_id[identifier]
        valid = result_record(fixture_raw, expected)
        corpus.compare_result(valid, fixture_raw, expected, "interpreter", BUILD, MACHINE, "valid")
        mutations = (
            ("flags", lambda item: item["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(2))),
            ("old-rax", lambda item: item["snapshot"]["value"]["state"]["gpr"].__setitem__(0, corpus.hx(0x777))),
            ("canary", lambda item: item["snapshot"]["value"]["memory"][1].__setitem__("bytes_hex", "00" + item["snapshot"]["value"]["memory"][1]["bytes_hex"][2:])),
            ("code", lambda item: item["snapshot"]["value"]["memory"][0].__setitem__("bytes_hex", "00" + item["snapshot"]["value"]["memory"][0]["bytes_hex"][2:])),
            ("retirement", lambda item: item.__setitem__("instructions_executed", {"status": "available", "value": corpus.hx(2)})),
        )
        for label, mutate in mutations:
            with self.subTest(label=label):
                changed = copy.deepcopy(valid)
                mutate(changed)
                with self.assertRaises(CorpusError):
                    corpus.compare_result(changed, fixture_raw, expected, "interpreter", BUILD, MACHINE, label)

    def _valid_summary(self, executable: Path, directory: Path) -> dict:
        selected_executable = str(executable.resolve())
        rows = []
        for item in self.manifest["case_digests"]:
            identifier = item["case_id"]
            raw = self.files[f"{identifier}.case.json"]
            expected = self.expected_by_id[identifier]
            observed = result_record(raw, expected)
            stdout = json.dumps(observed, indent=2, ensure_ascii=False) + "\n"
            fixture_path = str((directory / f"{identifier}.case.json").resolve())
            command = [selected_executable, "--fixture", fixture_path, "--engine", "interpreter", "--json"]
            process = {"returncode": 0, "stdout": stdout, "stderr": "", "command": command,
                       "cwd": str(corpus.ROOT.resolve()), "environment": corpus.process_environment(),
                       "timeout_diagnosis": None}
            rows.append({"case_id": identifier, "fixture_sha256": sha256(raw),
                         "repeats": [observed, copy.deepcopy(observed)], "processes": [process, copy.deepcopy(process)]})
        return {
            "schema_version": 1, "status": "passed", "backend": "interpreter",
            "native_x86": "not_run_form_not_admitted", "profile": corpus.PROFILE,
            "manifest_sha256": sha256(self.files[corpus.MANIFEST_NAME]), "case_count": 1026,
            "repeats_per_case": 2, "execution_identity": {"executable_sha256": sha256(executable.read_bytes()), "revision": BUILD["revision"]},
            "execution_machine": MACHINE, "runner_build": BUILD, "runner_machine": MACHINE,
            "results": rows, "failures": [], "missing_attempts": [],
        }

    def test_saved_evidence_is_frozen_and_unstable_repeats_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp) / "corpus"
            self._copy_bundle(directory)
            executable = Path(temp) / "od-runner"
            executable.write_bytes(b"mock runner binary")
            summary = self._valid_summary(executable, directory)
            result_path = Path(temp) / "results.json"
            result_path.write_bytes(json_bytes(summary))
            corpus.verify_results(directory, result_path, executable)

            bad_sha = copy.deepcopy(summary)
            bad_sha["execution_identity"]["executable_sha256"] = "x" * 64
            result_path.write_bytes(json_bytes(bad_sha))
            with self.assertRaises(CorpusError):
                corpus.verify_results(directory, result_path, executable)

            empty_argv = copy.deepcopy(summary)
            empty_argv["results"][0]["processes"][0]["command"] = []
            result_path.write_bytes(json_bytes(empty_argv))
            with self.assertRaises(CorpusError):
                corpus.verify_results(directory, result_path, executable)

            missing_cwd = copy.deepcopy(summary)
            missing_cwd["results"][0]["processes"][0].pop("cwd")
            result_path.write_bytes(json_bytes(missing_cwd))
            with self.assertRaises(CorpusError):
                corpus.verify_results(directory, result_path, executable)

            changed_env = copy.deepcopy(summary)
            changed_env["results"][0]["processes"][0]["environment"]["PATH"] = "/tampered"
            result_path.write_bytes(json_bytes(changed_env))
            with self.assertRaises(CorpusError):
                corpus.verify_results(directory, result_path, executable)

            unstable = copy.deepcopy(summary)
            unstable["results"][0]["repeats"][1]["extra_unstable_field"] = True
            unstable["results"][0]["processes"][1]["stdout"] = json.dumps(unstable["results"][0]["repeats"][1])
            result_path.write_bytes(json_bytes(unstable))
            with self.assertRaises(CorpusError):
                corpus.verify_results(directory, result_path, executable)

    def test_missing_corpus_preflight_replaces_stale_passed_summary(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            result_path = Path(temp) / "results.json"
            result_path.write_bytes(json_bytes({"status": "passed", "failures": []}))
            output = io.StringIO()
            with contextlib.redirect_stderr(output):
                status = corpus.main(["--run-interpreter", "--corpus", str(Path(temp) / "missing"),
                                      "--results", str(result_path), "--runner", str(Path(temp) / "runner")])
            self.assertEqual(status, 1)
            record = json.loads(result_path.read_bytes())
            self.assertEqual(record["status"], "failed")
            self.assertNotEqual(record.get("status"), "passed")
            self.assertTrue(record["failures"])

    def test_malformed_runner_json_writes_failed_summary(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            directory = root / "corpus"
            directory.mkdir()
            identifier = corpus.control_id(0, 1)
            fixture_raw = corpus.control_inputs()[f"{identifier}.case.json"]
            (directory / f"{identifier}.case.json").write_bytes(fixture_raw)
            expected = self.expected_by_id[identifier]
            (directory / corpus.EXPECTED_NAME).write_bytes(json_bytes({"profile": corpus.PROFILE, "cases": [expected]}))
            (directory / corpus.MANIFEST_NAME).write_bytes(self.files[corpus.MANIFEST_NAME])
            executable = root / "od-runner"
            executable.write_bytes(b"mock runner binary")
            partial_manifest = {"case_digests": [self.manifest["case_digests"][0]]}

            def fake_run(command, **_kwargs):
                if command[0] == "git":
                    return corpus.subprocess.CompletedProcess(command, 0, stdout=BUILD["revision"] + "\n", stderr="")
                return corpus.subprocess.CompletedProcess(command, 0, stdout="[]", stderr="")

            result_path = root / "results.json"
            with mock.patch.object(corpus, "verify_corpus", return_value=partial_manifest), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=fake_run):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(directory, result_path, executable)
            record = json.loads(result_path.read_bytes())
            self.assertEqual(record["status"], "failed")
            self.assertIn("one JSON object", record["failures"][0])

    def test_timeout_preserves_partial_streams_and_timeout_diagnosis(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            directory = root / "corpus"
            directory.mkdir()
            identifier = corpus.control_id(0, 0)
            fixture_raw = self.files[f"{identifier}.case.json"]
            (directory / f"{identifier}.case.json").write_bytes(fixture_raw)
            expected = self.expected_by_id[identifier]
            (directory / corpus.EXPECTED_NAME).write_bytes(
                json_bytes({"profile": corpus.PROFILE, "cases": [expected]}))
            manifest = {"case_digests": [self.manifest["case_digests"][0]]}
            (directory / corpus.MANIFEST_NAME).write_bytes(json_bytes(manifest))
            executable = root / "od-runner"
            executable.write_bytes(b"mock runner binary")
            result_path = root / "results.json"
            valid = result_record(fixture_raw, expected)
            calls = 0

            def fake_run(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1:
                    raise corpus.subprocess.TimeoutExpired(
                        command, corpus.RUNNER_TIMEOUT_SECONDS,
                        output=b"partial stdout \xff", stderr=b"partial stderr")
                return corpus.subprocess.CompletedProcess(
                    command, 0, stdout=json.dumps(valid, indent=2) + "\n", stderr="")

            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(
                        {"executable_sha256": sha256(executable.read_bytes()), "revision": BUILD["revision"]}, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=fake_run), \
                    contextlib.redirect_stderr(io.StringIO()):
                status = corpus.main(["--run-interpreter", "--corpus", str(directory),
                                      "--results", str(result_path), "--runner", str(executable)])

            self.assertEqual(status, 1)
            saved = json.loads(result_path.read_bytes())
            self.assertEqual(saved["status"], "failed")
            timed_out = saved["results"][0]["processes"][0]
            self.assertEqual(timed_out["returncode"], None)
            self.assertEqual(timed_out["stdout"], "partial stdout �")
            self.assertEqual(timed_out["stderr"], "partial stderr")
            self.assertEqual(timed_out["timeout_diagnosis"]["kind"], "timeout_expired")
            self.assertEqual(timed_out["timeout_diagnosis"]["timeout_seconds"], corpus.RUNNER_TIMEOUT_SECONDS)
            self.assertTrue(timed_out["timeout_diagnosis"]["message"])
            self.assertIn("timed out", saved["failures"][0])

    def test_interruption_preserves_completed_attempt_and_lists_missing_repeats(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            directory = root / "corpus"
            self._copy_bundle(directory)
            executable = root / "od-runner"
            executable.write_bytes(b"mock runner binary")
            result_path = root / "results.json"
            identifier = corpus.control_id(0, 0)
            fixture_raw = self.files[f"{identifier}.case.json"]
            expected = self.expected_by_id[identifier]
            valid = result_record(fixture_raw, expected)
            stdout = json.dumps(valid, indent=2, ensure_ascii=False) + "\n"
            calls = 0

            def fake_run(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1:
                    return corpus.subprocess.CompletedProcess(command, 0, stdout=stdout, stderr="")
                raise KeyboardInterrupt

            with mock.patch.object(corpus, "command_identity", return_value=(
                    {"executable_sha256": sha256(executable.read_bytes()), "revision": BUILD["revision"]}, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=fake_run), contextlib.redirect_stderr(io.StringIO()):
                status = corpus.main(["--run-interpreter", "--corpus", str(directory),
                                      "--results", str(result_path), "--runner", str(executable)])
            self.assertEqual(status, 1)
            saved = json.loads(result_path.read_bytes())
            self.assertEqual(saved["status"], "failed")
            first = saved["results"][0]
            self.assertEqual(first["case_id"], identifier)
            self.assertEqual(len(first["repeats"]), 2)
            self.assertEqual(first["repeats"][0], valid)
            self.assertIn("interrupted", first["repeats"][1]["failure"])
            self.assertEqual(len(saved["missing_attempts"]), (corpus.TOTAL_COUNT - 1) * corpus.REPEATS + 1)
            self.assertEqual(saved["missing_attempts"][0], {"case_id": identifier, "repeat": 2})
            self.assertEqual(first["processes"][0]["cwd"], str(corpus.ROOT.resolve()))
            self.assertEqual(set(first["processes"][0]["environment"]), set(corpus.PROCESS_ENV_KEYS))

    def test_explicit_native_request_fails_before_backend_and_records_reason(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            result_path = Path(temp) / "native-blocked.json"
            output = io.StringIO()
            with mock.patch.object(corpus.subprocess, "run", side_effect=AssertionError("backend called")), contextlib.redirect_stdout(output):
                status = corpus.main(["--native", "--results", str(result_path)])
            self.assertEqual(status, 1)
            record = json.loads(result_path.read_bytes())
            self.assertEqual(record["status"], "blocked")
            self.assertEqual(record["native_x86"], "not_run_form_not_admitted")
            self.assertEqual(record["backend_execution"], "not_started")
            self.assertIn("not admitted", record["blocked_reason"])


if __name__ == "__main__":
    unittest.main()
