"""Integer oracle, strict inputs/evidence, and native denial for S2-42ab."""
from __future__ import annotations

import copy
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
from stage2_corpus_support import CorpusError, json_bytes, sha256, strict_object

SCRIPT = Path(__file__).with_name("generate_stage2_div_bl_fault_corpus.py")
SPEC = importlib.util.spec_from_file_location("s2_42ab", SCRIPT)
corpus = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(corpus)

BUILD = {"revision": "1" * 40, "compiler": "rustc test", "target": "aarch64-apple-darwin", "profile": "debug"}
MACHINE = {"host_os": "macos", "host_arch": "aarch64"}


def result_record(raw: bytes, expected: dict) -> dict:
    exception = copy.deepcopy(expected["exception"])
    return {"schema_version": 2, "case_id": expected["case_id"], "input_sha256": sha256(raw),
        "profile": corpus.PROFILE, "backend": "interpreter", "outcome": expected["outcome"],
        "instructions_executed": expected["instructions_executed"], "build": BUILD, "machine": MACHINE,
        "snapshot": {"status": "available", "value": {
            "state": {key: expected["state"][key] for key in ("mode", "gpr", "rip", "rflags")},
            "rflags_defined": expected["state"]["rflags_defined"], "memory": expected["memory"],
            "minss_memory_rax_controls": None, "addss_memory_rax_controls": None,
            "exception": exception}}}


class DivBlFaultCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = corpus.materialize()
        cls.manifest = json.loads(cls.files["manifest.json"], object_pairs_hook=strict_object)
        cls.expected = {row["case_id"]: row for row in json.loads(cls.files["expected.json"], object_pairs_hook=strict_object)["cases"]}

    def copy_bundle(self, target: Path):
        target.mkdir()
        for name, raw in self.files.items():
            (target / name).write_bytes(raw)

    def test_independent_integer_oracle_admits_only_exact_fault_predicate(self):
        self.assertTrue(corpus.is_div_fault(0xFFFF, 0))
        self.assertTrue(corpus.is_div_fault(0x0100, 1))
        for divisor in (1, 2, 3, 127, 255):
            dividend = divisor * 256 - 1
            self.assertEqual(dividend // divisor, 255)
            self.assertFalse(corpus.is_div_fault(dividend, divisor))
            with self.assertRaises(CorpusError):
                corpus.make_fixture("valid_control", dividend, divisor,
                    corpus.base_gprs(dividend, divisor), corpus.BASE_FLAGS, "valid boundary rejection probe")
            valid = corpus.make_fixture("temporary_fault", divisor * 256, divisor,
                corpus.base_gprs(divisor * 256, divisor), corpus.BASE_FLAGS, "oracle mutation probe")
            valid["initial_state"]["gpr"][0] = corpus.hx((int(valid["initial_state"]["gpr"][0], 16) & ~0xFFFF) | dividend)
            with self.assertRaisesRegex(CorpusError, "only BL=0 or AX//BL>255"):
                corpus.expected_snapshot(valid)

    def test_generated_inputs_are_deterministic_unique_and_evenly_split(self):
        first, second = corpus.generated_fixtures(), corpus.generated_fixtures()
        self.assertEqual(first, second)
        self.assertEqual(len(first), 1024)
        self.assertEqual(len({(tuple(row["initial_state"]["gpr"]), row["initial_state"]["rflags"]) for row in first}), 1024)
        self.assertEqual(sum((int(row["initial_state"]["gpr"][3], 16) & 0xFF) == 0 for row in first), 512)
        self.assertEqual(sum((int(row["initial_state"]["gpr"][3], 16) & 0xFF) != 0 for row in first), 512)
        for row in first:
            gprs = row["initial_state"]["gpr"]
            self.assertEqual(row["memory"][0]["initial_bytes_hex"], "f6f3")
            self.assertTrue(corpus.is_div_fault(int(gprs[0], 16) & 0xFFFF, int(gprs[3], 16) & 0xFF))
            self.assertEqual(int(row["initial_state"]["rflags"], 16) & ~corpus.RFLAGS_DEFINED, 0)
            self.assertEqual(corpus.expected_snapshot(row)["instructions_executed"], {"status": "available", "value": corpus.hx(0)})

    def test_pinned_controls_cover_fault_boundaries_and_full_unchanged_snapshots(self):
        controls, _ = corpus.load_controls()
        self.assertEqual(len(controls), 8)
        observed_pairs = []
        for identifier, raw, frozen in controls:
            item = json.loads(raw, object_pairs_hook=strict_object)
            ax = int(item["initial_state"]["gpr"][0], 16) & 0xFFFF
            bl = int(item["initial_state"]["gpr"][3], 16) & 0xFF
            observed_pairs.append((ax, bl))
            self.assertEqual(frozen, corpus.expected_snapshot(item))
            self.assertEqual(frozen["state"]["gpr"], item["initial_state"]["gpr"])
            self.assertEqual(frozen["state"]["rip"], item["initial_state"]["rip"])
            self.assertEqual(frozen["state"]["rflags"], item["initial_state"]["rflags"])
            self.assertEqual(frozen["exception"]["value"]["vector"], 0)
            self.assertEqual(frozen["exception"]["value"]["error_code"]["status"], "unavailable")
            self.assertEqual(frozen["exception"]["value"]["fault_address"]["status"], "unavailable")
            self.assertEqual(frozen["exception"]["value"]["faulting_rip"], corpus.hx(corpus.CODE_ADDRESS))
        self.assertEqual(observed_pairs, list(corpus.CONTROL_AX_BL))

    def test_manifest_strict_file_set_and_control_tampering(self):
        self.assertEqual(self.manifest["generated_case_count"], 1024)
        self.assertEqual(self.manifest["control_count"], 8)
        self.assertEqual(self.manifest["case_count"], 1032)
        self.assertEqual(self.manifest["control_manifest_sha256"], corpus.CONTROL_MANIFEST_SHA256)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            self.assertEqual(corpus.verify_corpus(bundle), self.manifest)
            (bundle / "extra").write_text("x")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            item = bundle / f"{corpus.control_id(0)}.case.json"
            item.write_bytes(item.read_bytes() + b" ")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            item = bundle / f"{corpus.control_id(0)}.case.json"
            item.unlink()
            item.symlink_to(SCRIPT)
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)

    def test_result_comparison_rejects_any_fault_metadata_or_state_drift(self):
        identifier = corpus.control_id(0)
        raw = corpus.control_inputs()[f"{identifier}.case.json"]
        expected = self.expected[identifier]
        valid = result_record(raw, expected)
        corpus.compare_result(valid, raw, expected, BUILD, MACHINE, "valid")
        mutations = (
            lambda item: item.__setitem__("unrecognized", True),
            lambda item: item["snapshot"]["value"].__setitem__("unrecognized", True),
            lambda item: item["snapshot"]["value"]["state"]["gpr"].__setitem__(0, corpus.hx(0)),
            lambda item: item["snapshot"]["value"]["state"]["gpr"].__setitem__(15, corpus.hx(0)),
            lambda item: item["snapshot"]["value"]["state"].__setitem__("rip", corpus.hx(2)),
            lambda item: item["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(2)),
            lambda item: item["snapshot"]["value"].__setitem__("rflags_defined", corpus.hx(0)),
            lambda item: item["snapshot"]["value"]["memory"][0].__setitem__("bytes_hex", "9090"),
            lambda item: item["snapshot"]["value"]["exception"]["value"].__setitem__("vector", 6),
            lambda item: item["snapshot"]["value"]["exception"]["value"].__setitem__("faulting_rip", corpus.hx(2)),
            lambda item: item["snapshot"]["value"]["exception"]["value"]["error_code"].__setitem__("status", "available"),
            lambda item: item["snapshot"]["value"]["exception"]["value"]["fault_address"].__setitem__("status", "available"),
            lambda item: item["snapshot"]["value"]["exception"]["value"].__setitem__("vector", False),
            lambda item: item["instructions_executed"].__setitem__("value", corpus.hx(1)),
            lambda item: item["outcome"].__setitem__("kind", "stop_address"),
        )
        for mutate in mutations:
            changed = copy.deepcopy(valid)
            mutate(changed)
            with self.assertRaises(CorpusError):
                corpus.compare_result(changed, raw, expected, BUILD, MACHINE, "tampered")

    def test_native_block_record_is_created_without_backend_invocation(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "native.json"
            with mock.patch.object(corpus.subprocess, "run", side_effect=AssertionError("backend must not start")):
                corpus.write_native_blocked(path)
            record = json.loads(path.read_bytes())
            self.assertEqual(record["native_x86"], "not_run_form_not_admitted")
            self.assertEqual(record["backend_execution"], "not_started")
            self.assertIn("no remote host is contacted", record["blocked_reason"])

    def test_timeout_and_interruption_preserve_partial_process_and_repeat_evidence(self):
        identifier = corpus.control_id(0)
        raw = corpus.control_inputs()[f"{identifier}.case.json"]
        expected = self.expected[identifier]
        observation = result_record(raw, expected)
        one_case_manifest = {"case_digests": [{"case_id": identifier}]}
        identity = {"executable_sha256": "a" * 64, "revision": BUILD["revision"]}

        for failure_kind in ("timeout", "interrupt"):
            with self.subTest(failure_kind=failure_kind), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                bundle = root / "corpus"
                bundle.mkdir()
                fixture_path = bundle / f"{identifier}.case.json"
                fixture_path.write_bytes(raw)
                (bundle / "manifest.json").write_bytes(json_bytes(one_case_manifest))
                (bundle / "expected.json").write_bytes(json_bytes({"cases": [expected]}))
                executable = root / "od-runner"
                executable.write_bytes(b"runner identity fixture")
                result_path = root / "results.json"
                calls = 0

                def invoke(command, **_kwargs):
                    nonlocal calls
                    calls += 1
                    if calls == 1:
                        return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(observation), stderr="")
                    if failure_kind == "timeout":
                        raise corpus.subprocess.TimeoutExpired(command, corpus.RUNNER_TIMEOUT_SECONDS,
                            output=b"partial stdout \\xff", stderr=b"partial stderr")
                    raise KeyboardInterrupt

                with mock.patch.object(corpus, "verify_corpus", return_value=one_case_manifest), \
                        mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                        mock.patch.object(corpus.subprocess, "run", side_effect=invoke):
                    with self.assertRaises(CorpusError):
                        corpus.run_interpreter(bundle, result_path, executable)
                saved = json.loads(result_path.read_bytes())
                self.assertEqual(len(saved["results"][0]["repeats"]), 2)
                self.assertEqual(saved["results"][0]["repeats"][0], observation)
                second_process = saved["results"][0]["processes"][1]
                if failure_kind == "timeout":
                    self.assertEqual(saved["status"], "failed")
                    self.assertEqual(second_process["timeout_diagnosis"]["kind"], "timeout_expired")
                    self.assertEqual(second_process["stdout"], "partial stdout \\xff")
                    self.assertEqual(saved["missing_attempts"], [{"case_id": identifier, "repeat": 2}])
                else:
                    self.assertEqual(saved["status"], "incomplete")
                    self.assertEqual(saved["results"][0]["repeats"][1]["failure"], "interrupted")
                    self.assertEqual(saved["missing_attempts"], [{"case_id": identifier, "repeat": 2}])

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = root / "corpus"
            bundle.mkdir()
            identifier = corpus.control_id(0)
            raw = corpus.control_inputs()[f"{identifier}.case.json"]
            expected = self.expected[identifier]
            observation = result_record(raw, expected)
            one_case_manifest = {"case_digests": [{"case_id": identifier}]}
            (bundle / f"{identifier}.case.json").write_bytes(raw)
            (bundle / "manifest.json").write_bytes(json_bytes(one_case_manifest))
            (bundle / "expected.json").write_bytes(json_bytes({"profile": corpus.PROFILE, "cases": [expected]}))
            executable = root / "od-runner"
            executable.write_bytes(b"non-object stdout runner")
            output = root / "results.json"
            identity = {"executable_sha256": "b" * 64, "revision": BUILD["revision"]}
            calls = 0

            def non_object_repeat(command, **_kwargs):
                nonlocal calls
                calls += 1
                stdout = json.dumps(observation) if calls == 1 else "[]"
                return corpus.subprocess.CompletedProcess(command, 0, stdout=stdout, stderr="")

            with mock.patch.object(corpus, "verify_corpus", return_value=one_case_manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=non_object_repeat):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, output, executable)
            saved = json.loads(output.read_bytes())
            self.assertEqual(saved["status"], "failed")
            self.assertEqual(saved["results"][0]["repeats"][0], observation)
            self.assertIn("one JSON object", saved["results"][0]["repeats"][1]["failure"])
            self.assertEqual(saved["missing_attempts"], [{"case_id": identifier, "repeat": 2}])

    def test_saved_result_replay_binds_stdout_command_environment_and_build_identity(self):
        identifier = corpus.control_id(0)
        raw = corpus.control_inputs()[f"{identifier}.case.json"]
        expected = self.expected[identifier]
        observation = result_record(raw, expected)
        one_case_manifest = {"case_digests": [{"case_id": identifier}]}
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = root / "corpus"
            bundle.mkdir()
            (bundle / f"{identifier}.case.json").write_bytes(raw)
            (bundle / "manifest.json").write_bytes(json_bytes(one_case_manifest))
            (bundle / "expected.json").write_bytes(json_bytes({"profile": corpus.PROFILE, "cases": [expected]}))
            executable = root / "od-runner"
            executable.write_bytes(b"strict replay fixture")
            result_path = root / "results.json"
            replay_build = {**BUILD, "revision": corpus.source_revision()}
            replay_observation = result_record(raw, expected)
            replay_observation["build"] = replay_build
            identity = {"executable_sha256": sha256(executable.read_bytes()), "revision": replay_build["revision"]}
            with mock.patch.object(corpus, "verify_corpus", return_value=one_case_manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=lambda command, **_kwargs:
                        corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(replay_observation), stderr="")):
                corpus.run_interpreter(bundle, result_path, executable)
            with mock.patch.object(corpus, "verify_corpus", return_value=one_case_manifest):
                corpus.verify_results(bundle, result_path, executable)
            baseline = json.loads(result_path.read_bytes())
            mutations = (
                lambda value: value["results"][0]["processes"][0].__setitem__("stdout", "{}"),
                lambda value: value["results"][0]["processes"][0]["command"].append("--native"),
                lambda value: value["results"][0]["processes"][0]["environment"].__setitem__("PATH", "changed"),
                lambda value: value["execution_identity"].__setitem__("revision", "2" * 40),
                lambda value: value.__setitem__("schema_version", 999),
                lambda value: value.__setitem__("unrecognized", True),
            )
            for mutate in mutations:
                evidence = copy.deepcopy(baseline)
                mutate(evidence)
                result_path.write_bytes(json_bytes(evidence))
                with mock.patch.object(corpus, "verify_corpus", return_value=one_case_manifest):
                    with self.assertRaises(CorpusError):
                        corpus.verify_results(bundle, result_path, executable)
            forged = copy.deepcopy(baseline)
            fake_revision = "f" * 40
            forged["execution_identity"]["revision"] = fake_revision
            forged["runner_build"]["revision"] = fake_revision
            for row in forged["results"]:
                for repeat_index, observation in enumerate(row["repeats"]):
                    observation["build"]["revision"] = fake_revision
                    row["processes"][repeat_index]["stdout"] = json.dumps(observation)
            result_path.write_bytes(json_bytes(forged))
            with mock.patch.object(corpus, "verify_corpus", return_value=one_case_manifest):
                with self.assertRaises(CorpusError):
                    corpus.verify_results(bundle, result_path, executable)


if __name__ == "__main__":
    unittest.main()
