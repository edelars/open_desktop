#!/usr/bin/env python3
"""Orchestration infrastructure checks only; no SSH or synthetic CPU acceptance."""

import contextlib
import copy
import hashlib
import io
import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import stage1_oracle as previous
import stage2_oracle as oracle


class Stage2HarnessTests(unittest.TestCase):
    def setUp(self):
        self.real_root = oracle.ROOT
        self.raw = (self.real_root / oracle.DEFAULT_CASES[0]).read_bytes()
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        for name in ("stage1_oracle.py", "stage2_oracle.py"):
            target = self.root / "tests/cpu" / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes((self.real_root / "tests/cpu" / name).read_bytes())
        self.addCleanup(patch.stopall)
        patch.object(oracle, "ROOT", self.root).start()
        patch.object(previous, "ROOT", self.root).start()
        self.output = self.root / "output"
        self.output.mkdir()

    def case(self, directory="input", name="one.case.json", case_id=None):
        path = self.root / directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        value = json.loads(self.raw)
        if case_id is not None:
            value["case_id"] = case_id
        path.write_text(json.dumps(value))
        return path

    def result(self, fixture, digest="a" * 64, backend="native_x86", revision="sha256:test",
               compiler="rustc test", profile=None):
        profile = profile or fixture["profile"]
        state = {"mode": "long64", "gpr": ["0x0"] * 16,
                 "rip": "0x100003", "rflags": "0x2"}
        if profile == oracle.XORPD_PROFILE:
            state["sse"] = {"xmm": ["00" * 16 for _ in range(16)], "mxcsr": "0x1f80"}
        return {"schema_version": 2, "case_id": fixture["case_id"], "input_sha256": digest,
                "profile": profile, "backend": backend,
                "outcome": {"kind": fixture["expected_exit"]},
                "instructions_executed": {"status": "available", "value": "0x1"},
                "build": {"revision": revision, "compiler": compiler,
                          "target": "x86_64-linux", "profile": "debug"},
                "machine": {"host_os": "linux", "host_arch": "x86_64"},
                "snapshot": {"status": "available", "value": {"state": state,
                    "rflags_defined": "0x0", "exception": {"status": "available", "value": None},
                    "memory": fixture["memory"]}}}

    def test_exact_json_is_frozen_before_execution_and_remains_bound_to_digest(self):
        path = self.case()
        raw = path.read_bytes() + b"\n\n"
        path.write_bytes(raw)
        frozen, _, digest = oracle.prepare_cases([str(path)], self.output)[0]
        path.write_text("changed original")
        self.assertEqual(frozen.read_bytes(), raw)
        self.assertEqual(digest, hashlib.sha256(raw).hexdigest())

    def test_rejects_duplicate_case_ids_or_filenames_without_partial_freeze(self):
        for same_name in (True, False):
            output = self.output / str(same_name)
            output.mkdir()
            first = self.case("first", "first.case.json", "first")
            second = self.case("second", "first.case.json" if same_name else "second.case.json",
                               "second" if same_name else "first")
            with self.assertRaisesRegex(oracle.Stage2Error, "duplicate fixture"):
                oracle.prepare_cases([str(first), str(second)], output)
            self.assertFalse((output / "fixtures").exists())

    def test_rejects_unsafe_names_ids_root_escape_and_symlink_escape(self):
        for name, case_id in [("bad name.case.json", "valid"), ("safe.case.json", "../escape")]:
            path = self.case(name=name, case_id=case_id)
            with self.assertRaisesRegex(oracle.Stage2Error, "unsafe fixture"):
                oracle.prepare_cases([str(path)], self.output)
        with tempfile.TemporaryDirectory() as external:
            outside = Path(external) / "outside.case.json"
            outside.write_bytes(self.raw)
            link = self.root / "link.case.json"
            link.symlink_to(outside)
            for path in (outside, link):
                with self.assertRaisesRegex(oracle.Stage2Error, "inside the repository"):
                    oracle.prepare_cases([str(path)], self.output)

    def test_schema_profile_size_and_duplicate_fields_reject_before_build(self):
        path = self.case()
        original = json.loads(self.raw)
        for field, value in [("schema_version", 1), ("schema_version", True),
                             ("profile", "s0_09_register_subset_v1"), ("wall_timeout_ms", 0)]:
            malformed = copy.deepcopy(original)
            malformed[field] = value
            path.write_text(json.dumps(malformed))
            with self.assertRaises(oracle.Stage2Error):
                oracle.prepare_cases([str(path)], self.output)
        path.write_bytes(b" " * (oracle.MAX_FIXTURE_BYTES + 1))
        with self.assertRaisesRegex(oracle.Stage2Error, "JSON limit"):
            oracle.prepare_cases([str(path)], self.output)
        path.write_bytes(self.raw.replace(b'"schema_version": 2', b'"schema_version": 2, "schema_version": 2', 1))
        with self.assertRaisesRegex(oracle.Stage2Error, "duplicate JSON field"):
            oracle.prepare_cases([str(path)], self.output)

    def write_xorpd_corpus(self, root):
        corpus = root / "corpus"
        corpus.mkdir(parents=True)
        digests = []
        for seed in oracle.XORPD_SEEDS:
            for sequence in range(oracle.XORPD_CASES_PER_SEED):
                case_id = f"s2_42a_xorpd_{seed:016x}_{sequence:04x}"
                value = {"schema_version": 2, "case_id": case_id, "profile": oracle.XORPD_PROFILE,
                         "seed": f"0x{seed:016x}",
                         "initial_state": {"sse": {"xmm": [hashlib.sha256(f"{case_id}:{index}".encode()).hexdigest()[:32]
                                                               for index in range(16)]}}}
                raw = json.dumps(value, separators=(",", ":")).encode()
                (corpus / f"{case_id}.case.json").write_bytes(raw)
                digests.append(hashlib.sha256(raw).hexdigest())
        manifest = {"manifest_version": 1, "generator_version": oracle.XORPD_GENERATOR_VERSION,
                    "profile": oracle.XORPD_PROFILE,
                    "seeds": [f"0x{seed:016x}" for seed in oracle.XORPD_SEEDS],
                    "sequence": "0..256 (exclusive)", "case_digests": digests,
                    "corpus_digest": hashlib.sha256("\n".join(digests).encode()).hexdigest()}
        (root / "manifest.json").write_text(json.dumps(manifest))
        return root

    def test_recorded_xorpd_corpus_is_frozen_in_manifest_order(self):
        corpus_root = self.write_xorpd_corpus(self.root / "recorded")
        cases, manifest, digest = oracle.prepare_xorpd_corpus(corpus_root, self.output)
        self.assertEqual(len(cases), 1024)
        self.assertEqual(cases[0][1]["case_id"], f"s2_42a_xorpd_{oracle.XORPD_SEEDS[0]:016x}_0000")
        self.assertEqual(cases[-1][1]["case_id"], f"s2_42a_xorpd_{oracle.XORPD_SEEDS[-1]:016x}_00ff")
        self.assertEqual(manifest["profile"], oracle.XORPD_PROFILE)
        self.assertEqual(digest, hashlib.sha256((corpus_root / "manifest.json").read_bytes()).hexdigest())
        self.assertEqual((self.output / "fixtures" / cases[0][0].name).read_bytes(), cases[0][0].read_bytes())

    def test_xorpd_corpus_rejects_manifest_hash_case_set_profile_and_duplicate_input_changes(self):
        for change in ("manifest_hash", "missing", "extra", "wrong_profile", "duplicate_input"):
            with self.subTest(change=change):
                root = self.write_xorpd_corpus(self.root / change)
                if change == "manifest_hash":
                    manifest = json.loads((root / "manifest.json").read_text())
                    manifest["case_digests"][0] = "0" * 64
                    manifest["corpus_digest"] = hashlib.sha256("\n".join(manifest["case_digests"]).encode()).hexdigest()
                    (root / "manifest.json").write_text(json.dumps(manifest))
                    expected = "case hash mismatch"
                elif change == "missing":
                    next((root / "corpus").iterdir()).unlink()
                    expected = "missing or extra"
                elif change == "extra":
                    (root / "corpus" / "extra.case.json").write_text("{}")
                    expected = "missing or extra"
                elif change == "wrong_profile":
                    path = sorted((root / "corpus").glob("*.case.json"))[0]
                    value = json.loads(path.read_text())
                    value["profile"] = "s2-long-integer-v1"
                    path.write_text(json.dumps(value))
                    expected = "case hash mismatch"
                else:
                    first_path = root / "corpus" / f"s2_42a_xorpd_{oracle.XORPD_SEEDS[0]:016x}_0000.case.json"
                    second_path = root / "corpus" / f"s2_42a_xorpd_{oracle.XORPD_SEEDS[0]:016x}_0001.case.json"
                    second = json.loads(second_path.read_text())
                    first = json.loads(first_path.read_text())
                    second["initial_state"]["sse"]["xmm"] = first["initial_state"]["sse"]["xmm"]
                    second_path.write_text(json.dumps(second))
                    # Keep manifest hashes consistent so duplicate input validation is reached.
                    manifest = json.loads((root / "manifest.json").read_text())
                    manifest["case_digests"][1] = hashlib.sha256(second_path.read_bytes()).hexdigest()
                    manifest["corpus_digest"] = hashlib.sha256("\n".join(manifest["case_digests"]).encode()).hexdigest()
                    (root / "manifest.json").write_text(json.dumps(manifest))
                    expected = "duplicate XMM inputs"
                (self.output / change).mkdir()
                with self.assertRaisesRegex(oracle.Stage2Error, expected):
                    oracle.prepare_xorpd_corpus(root, self.output / change)

    def test_xorpd_manifest_rejects_extra_fields_duplicate_json_and_wrong_profile(self):
        root = self.write_xorpd_corpus(self.root / "strict-manifest")
        path = root / "manifest.json"
        base = json.loads(path.read_text())
        for malformed in ({**base, "unrecognized": True},
                          {**base, "profile": "s2-long-integer-v1"}):
            path.write_text(json.dumps(malformed))
            with self.assertRaises(oracle.Stage2Error):
                oracle._xorpd_manifest(path)
        path.write_text(path.read_text().replace('"manifest_version": 1',
                                                  '"manifest_version": 1, "manifest_version": 1', 1))
        with self.assertRaisesRegex(oracle.Stage2Error, "duplicate JSON field"):
            oracle._xorpd_manifest(path)

    def test_xorpd_suite_requires_two_repeats_and_recorded_corpus_only(self):
        with self.assertRaises(SystemExit):
            oracle.main(["--suite", "xorpd-native-corpus", "--repeats", "3"])
        with self.assertRaises(SystemExit):
            oracle.main(["--suite", "xorpd-native-corpus", "some.case.json"])

    def _run_mock_xorpd_suite(self, malformed_native_snapshot=False):
        fingerprint = "f" * 64
        interpreter_calls, batch_calls = [], []

        def prepare(_corpus_root, output):
            fixtures_dir = output / "fixtures"
            fixtures_dir.mkdir()
            cases = []
            for sequence in range(2):
                fixture = json.loads(self.raw)
                fixture["case_id"] = f"s2_42a_xorpd_simulated_{sequence}"
                fixture["profile"] = oracle.XORPD_PROFILE
                fixture["seed"] = f"0x{oracle.XORPD_SEEDS[0]:016x}"
                fixture["initial_state"]["sse"] = {"xmm": [f"{sequence:02x}" * 16 for _ in range(16)],
                                                    "mxcsr": "0x1f80"}
                path = fixtures_dir / f"{fixture['case_id']}.case.json"
                raw = json.dumps(fixture).encode()
                path.write_bytes(raw)
                cases.append((path, fixture, hashlib.sha256(raw).hexdigest()))
            return cases, {"corpus_digest": "a" * 64}, "b" * 64

        def native_batch(_target, _remote_root, _runner, _fixture_dir, cases, output_dir, *, process_runner):
            batch_calls.append(output_dir.name)
            results = {}
            for index, (path, fixture, digest) in enumerate(cases):
                result = self.result(fixture, digest, "native_x86", f"sha256:{fingerprint}",
                                    "rustc remote", oracle.XORPD_PROFILE)
                if malformed_native_snapshot and not batch_calls[:-1] and index == 0:
                    result["snapshot"]["value"] = None
                case_dir = output_dir / fixture["case_id"]
                case_dir.mkdir()
                (case_dir / "native_x86.json").write_text(json.dumps(result))
                (case_dir / "native_x86.stderr.log").write_text("")
                (case_dir / "native_x86.process.json").write_text(json.dumps({"returncode": 0}))
                results[path.name] = result
            return results

        def compare_results(_runner, _left, _right, fixture_path, _report_path, _env, **_kwargs):
            fixture = json.loads(Path(fixture_path).read_text())
            return {"schema_version": 2, "case_id": fixture["case_id"],
                    "input_sha256": hashlib.sha256(Path(fixture_path).read_bytes()).hexdigest(),
                    "matches": True, "differences": []}

        def run(argv, **_kwargs):
            if argv[0] == "ssh":
                command = argv[-1]
                if "python3 -c" in command:
                    return subprocess.CompletedProcess(argv, 0, fingerprint + "\n", "")
                if "rustc --version" in command:
                    return subprocess.CompletedProcess(argv, 0, "rustc remote\n", "")
                if "cargo --version" in command:
                    return subprocess.CompletedProcess(argv, 0, "cargo remote\n", "")
                if command == "uname -a":
                    return subprocess.CompletedProcess(argv, 0, "Linux test\n", "")
                return subprocess.CompletedProcess(argv, 0, "", "")
            if argv[0] == "scp" or argv[0] == "cargo":
                return subprocess.CompletedProcess(argv, 0, "", "")
            if "--verify-native-xorpd-corpus" in argv:
                return subprocess.CompletedProcess(argv, 0, "verified 1024 cases\n", "")
            if "--fixture" in argv:
                fixture_path = Path(argv[argv.index("--fixture") + 1])
                fixture = json.loads(fixture_path.read_text())
                digest = hashlib.sha256(fixture_path.read_bytes()).hexdigest()
                interpreter_calls.append(fixture["case_id"])
                result = self.result(fixture, digest, "interpreter", f"sha256:{fingerprint}",
                                     "rustc local", oracle.XORPD_PROFILE)
                return subprocess.CompletedProcess(argv, 0, json.dumps(result), "")
            raise AssertionError(f"unexpected command: {argv}")

        with patch.object(oracle, "prepare_xorpd_corpus", side_effect=prepare), \
             patch.object(oracle, "run_remote_xorpd_batch", side_effect=native_batch), \
             patch.object(previous, "compare", side_effect=compare_results), \
             patch.object(previous, "source_fingerprint", return_value=fingerprint), \
             patch.object(previous, "local_machine", return_value={"rustc": "rustc local", "cargo": "cargo local"}), \
             patch.object(previous, "run", side_effect=run), \
             contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            status = oracle.main(["--suite", "xorpd-native-corpus", "--host", "test-host",
                                  "--output-root", "work/stage2-oracle"])
        manifests = list((self.root / "work/stage2-oracle").glob("*/manifest.json"))
        self.assertEqual(len(manifests), 1)
        return status, json.loads(manifests[0].read_text()), interpreter_calls, batch_calls

    def test_xorpd_suite_orchestrates_two_interpreter_and_native_repeats(self):
        status, manifest, interpreter_calls, batch_calls = self._run_mock_xorpd_suite()
        self.assertEqual(status, 0, manifest)
        self.assertEqual(manifest["status"], "passed")
        self.assertEqual(manifest["corpus_manifest"]["physical_native"], "executed_and_matched")
        self.assertEqual(batch_calls, ["repeat-01", "repeat-02"])
        self.assertEqual(len(interpreter_calls), 4)
        self.assertEqual(len(manifest["fixtures"]), 4)
        self.assertEqual([row["repeat"] for row in manifest["fixtures"]], [1, 1, 2, 2])
        self.assertEqual([row["stable_repeat"] for row in manifest["fixtures"]], [False, False, True, True])

    def test_xorpd_malformed_nested_native_result_fails_run_manifest_cleanly(self):
        status, manifest, _, _ = self._run_mock_xorpd_suite(malformed_native_snapshot=True)
        self.assertEqual(status, 1)
        self.assertEqual(manifest["status"], "failed")
        self.assertEqual(manifest["corpus_manifest"]["physical_native"], "failed")
        self.assertIn("snapshot value must be an object", manifest["error"])

    def test_failed_process_evidence_preserves_output_returncode_and_only_build_environment(self):
        manifest = {"commands": []}
        evidence = oracle.Evidence(self.output, manifest)
        process = subprocess.CompletedProcess(["cargo"], 42, "build stdout", "build stderr")
        with patch.object(previous, "run", return_value=process):
            result = evidence.run(["cargo", "build"], env={"RUSTC_VERSION": "rustc test", "SECRET": "must not persist"})
        self.assertEqual(result.returncode, 42)
        saved = json.loads((self.output / "manifest.json").read_text())
        record = saved["commands"][0]
        self.assertEqual(record["returncode"], 42)
        self.assertEqual(record["build_environment"], {"RUSTC_VERSION": "rustc test"})
        self.assertEqual((self.output / record["stdout"]).read_text(), "build stdout")
        self.assertEqual((self.output / record["stderr"]).read_text(), "build stderr")

    def test_timeout_preserves_partial_process_output_in_failure_evidence(self):
        expired = subprocess.TimeoutExpired(["ssh"], 1, output=b"partial out", stderr=b"partial err")
        failure = previous.Stage1Error("timeout")
        failure.__cause__ = expired
        evidence = oracle.Evidence(self.output, {"commands": []})
        with patch.object(previous, "run", side_effect=failure):
            with self.assertRaisesRegex(oracle.Stage2Error, "timeout"):
                evidence.run(["ssh", "host"])
        manifest = json.loads((self.output / "manifest.json").read_text())
        record = manifest["commands"][0]
        self.assertIsNone(record["returncode"])
        self.assertEqual((self.output / record["stdout"]).read_text(), "partial out")
        self.assertEqual((self.output / record["stderr"]).read_text(), "partial err")

    def test_xorpd_native_timeout_keeps_each_completed_case_result_and_status(self):
        first, second = self.case("native-input", "first.case.json", "first"), self.case(
            "native-input", "second.case.json", "second")
        prepared = self.output / "prepared"
        prepared.mkdir()
        cases = oracle.prepare_cases([str(first), str(second)], prepared)
        first_record = json.dumps({"filename": cases[0][0].name, "returncode": 0,
            "stdout": '{"case_id":"first"}', "stderr": "first stderr"}, separators=(",", ":"))
        evidence = oracle.Evidence(self.output, {"commands": []})
        repeat = self.output / "repeat-01"
        repeat.mkdir()
        script = ("import json,sys,time; "
                  f"print({json.dumps(first_record)}, flush=True); "
                  "print('timed out on second case', file=sys.stderr, flush=True); time.sleep(2)")
        with patch.object(previous, "ssh_argv", return_value=[sys.executable, "-c", script]):
            with self.assertRaisesRegex(oracle.Stage2Error, "timed out"):
                oracle.run_remote_xorpd_batch("host", "/repo", "native-runner", "fixtures", cases,
                                              repeat, process_runner=evidence.run, timeout=0.1)
        first_dir = repeat / "first"
        self.assertEqual((first_dir / "native_x86.json").read_text(), '{"case_id":"first"}')
        self.assertEqual((first_dir / "native_x86.stderr.log").read_text(), "first stderr")
        self.assertEqual(json.loads((first_dir / "native_x86.process.json").read_text()), {"returncode": 0})
        self.assertFalse((repeat / "second" / "native_x86.json").exists())
        command = json.loads((self.output / "manifest.json").read_text())["commands"][0]
        self.assertIn(first_record, (self.output / command["stdout"]).read_text())

    def test_stream_timeout_applies_after_child_closes_both_pipes(self):
        evidence = oracle.Evidence(self.output, {"commands": []})
        script = "import os,time; os.close(1); os.close(2); time.sleep(2)"
        started = time.monotonic()
        with self.assertRaisesRegex(oracle.Stage2Error, "timed out"):
            evidence.run([sys.executable, "-c", script], timeout=0.05,
                         stdout_consumer=lambda _line, _complete: None)
        self.assertLess(time.monotonic() - started, 1.0)
        manifest = json.loads((self.output / "manifest.json").read_text())
        self.assertIsNone(manifest["commands"][0]["returncode"])

    def test_missing_or_failed_native_output_is_not_skipped_and_raw_batch_is_preserved(self):
        path = self.case()
        cases = oracle.prepare_cases([str(path)], self.output)
        batch_dir = self.output / "batch"
        batch_dir.mkdir()
        evidence = oracle.Evidence(self.output, {"commands": []})
        records = {path.name: {"returncode": 4, "stdout": '{"outcome":{"kind":"unsupported"}}',
                               "stderr": "unsupported mapping"}}
        process = subprocess.CompletedProcess([], 0, json.dumps(records), "ssh diagnostic")
        with patch.object(previous, "run", return_value=process):
            with self.assertRaisesRegex(oracle.Stage2Error, "exit 4"):
                previous.run_remote_batch("host", "/repo", "runner", "fixtures", cases,
                                          batch_dir, process_runner=evidence.run)
        self.assertEqual(json.loads((batch_dir / "native-batch.stdout.json").read_text()), records)
        case_dir = batch_dir / cases[0][1]["case_id"]
        self.assertEqual((case_dir / "native_x86.stderr.log").read_text(), "unsupported mapping")
        self.assertEqual(json.loads((case_dir / "native_x86.process.json").read_text())["returncode"], 4)
        with patch.object(previous, "run", return_value=subprocess.CompletedProcess([], 0, "{}", "")):
            with self.assertRaisesRegex(oracle.Stage2Error, "different number"):
                previous.run_remote_batch("host", "/repo", "runner", "fixtures", cases,
                                          batch_dir, process_runner=evidence.run)

    def test_result_identity_and_build_fingerprint_are_required_before_comparison(self):
        fixture = json.loads(self.raw)
        # Deliberately partial dictionaries test admission only. They are never
        # presented as CPU results or successful acceptance evidence.
        result = self.result(fixture)
        for field, value in [("schema_version", 1), ("case_id", "different"), ("input_sha256", "b" * 64),
                             ("profile", "wrong"), ("backend", "interpreter"),
                             ("outcome", {"kind": "unsupported"}), ("build", {"revision": "old"}),
                             ("snapshot", {"status": "unavailable"})]:
            malformed = copy.deepcopy(result)
            malformed[field] = value
            with self.subTest(field=field), self.assertRaises(oracle.Stage2Error):
                oracle.validate_result(malformed, fixture, "a" * 64, "native_x86", "sha256:test", "rustc test", "test")

    def test_xorpd_result_requires_native_profile_and_complete_snapshot_admission(self):
        fixture = json.loads(self.raw)
        fixture["profile"] = oracle.XORPD_PROFILE
        result = self.result(fixture, profile=oracle.XORPD_PROFILE)
        oracle.validate_result(result, fixture, "a" * 64, "native_x86", "sha256:test", "rustc test",
                               "native result", oracle.XORPD_PROFILE)
        with self.assertRaisesRegex(oracle.Stage2Error, "invalid profile"):
            oracle.validate_result(result, fixture, "a" * 64, "native_x86", "sha256:test", "rustc test",
                                   "native result")

    def test_nested_null_snapshot_is_a_harness_error_not_an_attribute_error(self):
        fixture = json.loads(self.raw)
        fixture["profile"] = oracle.XORPD_PROFILE
        result = self.result(fixture, profile=oracle.XORPD_PROFILE)
        result["snapshot"]["value"] = None
        with self.assertRaisesRegex(oracle.Stage2Error, "snapshot value must be an object"):
            oracle.validate_result(result, fixture, "a" * 64, "native_x86", "sha256:test", "rustc test",
                                   "native result", oracle.XORPD_PROFILE)

    def test_comparison_requires_positive_match_and_exact_case_binding(self):
        fixture = json.loads(self.raw)
        report = {"schema_version": 2, "case_id": fixture["case_id"], "input_sha256": "a" * 64,
                  "matches": True, "differences": []}
        for field, value in [("matches", False), ("input_sha256", "b" * 64), ("schema_version", 1),
                             ("differences", ["gpr[15]"])]:
            changed = {**report, field: value}
            with self.assertRaisesRegex(oracle.Stage2Error, "comparison report"):
                oracle.validate_comparison(changed, fixture, "a" * 64)

    def test_main_failure_manifest_keeps_frozen_inputs_and_failed_build_output(self):
        path = self.case()
        responses = [subprocess.CompletedProcess([], 0, "f" * 64 + "\n", ""),
                     subprocess.CompletedProcess([], 0, "rustc remote", ""),
                     subprocess.CompletedProcess([], 0, "cargo remote", ""),
                     subprocess.CompletedProcess([], 0, "Linux remote", ""),
                     subprocess.CompletedProcess([], 101, "compile stdout", "compile failure")]
        with patch.object(previous, "source_fingerprint", return_value="f" * 64), \
             patch.object(previous, "local_machine", return_value={"rustc": "rustc local"}), \
             patch.object(previous, "run", side_effect=responses), \
             contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(oracle.main([str(path), "--host", "test-host"]), 1)
        manifests = list((self.root / "work/stage2-oracle").glob("*/manifest.json"))
        self.assertEqual(len(manifests), 1)
        manifest = json.loads(manifests[0].read_text())
        self.assertEqual(manifest["suite"], "integer")
        self.assertEqual(manifest["profile"], "s2-long-integer-v1")
        self.assertNotIn("corpus_manifest", manifest)
        self.assertEqual(manifest["status"], "failed")
        self.assertIn("local runner build failed", manifest["error"])
        frozen = self.root / manifest["inputs"][0]["fixture_path"]
        self.assertEqual(frozen.read_bytes(), path.read_bytes())
        failed = manifest["commands"][-1]
        self.assertEqual(failed["returncode"], 101)
        self.assertEqual((manifests[0].parent / failed["stderr"]).read_text(), "compile failure")

    def test_legacy_link_rejects_other_fingerprint_missing_repeats_or_mismatch(self):
        path = self.output / "legacy.json"
        base = {"status": "passed", "source_fingerprint": "f" * 64, "repeats": 2,
                "inputs": [{"case_id": f"case-{i}"} for i in range(1010)], "fixtures": []}
        for item in base["inputs"]:
            for repeat in (1, 2):
                base["fixtures"].append({"case_id": item["case_id"], "repeat": repeat,
                    "differences": {"interpreter_vs_native": 0, "jit_vs_native": 0}})
        for change in ("fingerprint", "missing", "duplicate", "mismatch"):
            value = copy.deepcopy(base)
            if change == "fingerprint":
                value["source_fingerprint"] = "e" * 64
            elif change == "missing":
                value["fixtures"].pop()
            elif change == "duplicate":
                value["fixtures"][-1] = value["fixtures"][0]
            else:
                value["fixtures"][0]["differences"]["interpreter_vs_native"] = 1
            path.write_text(json.dumps(value))
            with self.subTest(change=change), self.assertRaises(oracle.Stage2Error):
                oracle.legacy_evidence(path, "f" * 64)


if __name__ == "__main__":
    unittest.main()
