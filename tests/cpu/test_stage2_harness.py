#!/usr/bin/env python3
"""Orchestration infrastructure checks only; no SSH or synthetic CPU acceptance."""

import contextlib
import copy
import hashlib
import io
import json
import subprocess
import tempfile
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
        result = {"schema_version": 2, "case_id": fixture["case_id"], "input_sha256": "a" * 64,
                  "profile": oracle.PROFILE, "backend": "native_x86", "outcome": {"kind": "stop_address"},
                  "build": {"revision": "sha256:test", "compiler": "rustc test", "target": "x86_64-linux", "profile": "debug"},
                  "machine": {"host_os": "linux", "host_arch": "x86_64"},
                  "snapshot": {"status": "available", "value": {"state": {"mode": "long64", "gpr": [0] * 16},
                      "exception": {"status": "available"}, "memory": fixture["memory"]}}}
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
        result = {"schema_version": 2, "case_id": fixture["case_id"], "input_sha256": "a" * 64,
                  "profile": oracle.XORPD_PROFILE, "backend": "native_x86",
                  "outcome": {"kind": fixture["expected_exit"]},
                  "build": {"revision": "sha256:test", "compiler": "rustc test",
                            "target": "x86_64-linux", "profile": "debug"},
                  "machine": {"host_os": "linux", "host_arch": "x86_64"},
                  "snapshot": {"status": "available", "value": {"state": {"mode": "long64", "gpr": [0] * 16},
                      "exception": {"status": "available"}, "memory": fixture["memory"]}}}
        oracle.validate_result(result, fixture, "a" * 64, "native_x86", "sha256:test", "rustc test",
                               "native result", oracle.XORPD_PROFILE)
        with self.assertRaisesRegex(oracle.Stage2Error, "invalid profile"):
            oracle.validate_result(result, fixture, "a" * 64, "native_x86", "sha256:test", "rustc test",
                                   "native result")

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
