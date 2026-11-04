"""Controls, integer oracle, reproducibility, evidence replay and native denial for S2-42y."""
from __future__ import annotations

import contextlib
import copy
import importlib.util
import json
import sys
import tempfile
import unittest
from collections import Counter
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
from stage2_corpus_support import CorpusError, json_bytes, sha256

SCRIPT = Path(__file__).with_name("generate_stage2_xadd_qword_memory_corpus.py")
SPEC = importlib.util.spec_from_file_location("s2_42y", SCRIPT)
corpus = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(corpus)

BUILD = {"revision": "1" * 40, "compiler": "rustc test", "target": "aarch64-apple-darwin", "profile": "debug"}
MACHINE = {"host_os": "macos", "host_arch": "aarch64", "cpu_model": None, "kernel": None, "page_size": None}


def observation(raw: bytes, expected: dict) -> dict:
    return {"schema_version": 2, "case_id": expected["case_id"], "input_sha256": sha256(raw),
            "profile": corpus.PROFILE, "backend": "interpreter", "outcome": expected["outcome"],
            "instructions_executed": expected["instructions_executed"], "build": BUILD, "machine": MACHINE,
            "snapshot": {"status": "available", "value": {"state": {k: expected["state"][k] for k in ("mode", "gpr", "rip", "rflags")},
                "rflags_defined": expected["state"]["rflags_defined"], "memory": expected["memory"],
                "minss_memory_rax_controls": None, "addss_memory_rax_controls": None,
                "exception": {"status": "available", "value": None}}}}


class XaddMemoryCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = corpus.materialize()
        cls.manifest = json.loads(cls.files["manifest.json"])
        cls.expected = {x["case_id"]: x for x in json.loads(cls.files["expected.json"])["cases"]}

    def copy_bundle(self, target: Path):
        target.mkdir()
        for name, contents in self.files.items(): (target / name).write_bytes(contents)

    def test_exact_bare_encoding_geometry_and_eight_frozen_controls(self):
        controls = corpus.control_inputs()
        self.assertEqual(len(controls), 8)
        cases, _ = corpus.load_controls()
        self.assertEqual([row[0] for row in cases], [corpus.control_id(i) for i in range(8)])
        for index, raw in enumerate(controls.values()):
            item = json.loads(raw)
            self.assertEqual(item["memory"][0]["initial_bytes_hex"], "480fc100")
            self.assertEqual(item["memory"][0]["length"], 4)
            self.assertEqual(item["initial_state"]["rip"], corpus.hx(0x100000))
            self.assertEqual(item["stop_rip"], corpus.hx(0x100004))
            self.assertEqual(item["instruction_budget"], 1)
            self.assertEqual(int(item["initial_state"]["gpr"][0], 16), corpus.OPERAND_ADDRESS)
            self.assertEqual([int(x["guest_address"], 16) for x in item["memory"][1:]], [0x3000F8, 0x300100])
            self.assertEqual([x["length"] for x in item["memory"][1:]], [8, 8])
            self.assertEqual(item["memory"][1]["permissions"], {"read": True, "write": True, "execute": False})
            self.assertEqual(item["memory"][2]["permissions"], {"read": True, "write": True, "execute": False})
            initial_data = bytes.fromhex(item["memory"][1]["initial_bytes_hex"] + item["memory"][2]["initial_bytes_hex"])
            memory = int.from_bytes(initial_data[3:11], "little")
            self.assertEqual(memory, corpus.CONTROL_MEMORY_VALUES[index])
            expected = corpus.expected_snapshot(item)
            self.assertEqual(expected["state"]["gpr"][0], corpus.hx(memory))
            self.assertEqual(expected["memory"][0]["bytes_hex"], "480fc100")
            self.assertEqual(expected["outcome"], {"kind": "stop_address"})
            self.assertEqual(expected["instructions_executed"]["value"], corpus.hx(1))
            self.assertIsNone(expected["exception"])
            self.assertEqual(expected, cases[index][2])
        for index in range(8):
            before = json.loads(controls[f"{corpus.control_id(index)}.case.json"])
            after = corpus.expected_snapshot(before)
            self.assertEqual(after["state"]["gpr"][1:], before["initial_state"]["gpr"][1:])
            self.assertEqual(int(after["state"]["rflags"], 16) & ~corpus.ARITHMETIC_FLAGS,
                             int(before["initial_state"]["rflags"], 16) & ~corpus.ARITHMETIC_FLAGS)

    def test_oracle_covers_wrap_zero_auxiliary_carry_and_signed_overflow(self):
        observed = []
        for raw in corpus.control_inputs().values():
            item = json.loads(raw)
            old = int.from_bytes(bytes.fromhex(item["memory"][1]["initial_bytes_hex"] + item["memory"][2]["initial_bytes_hex"])[3:11], "little")
            result = (old + corpus.OPERAND_ADDRESS) & corpus.MASK64
            flags = int(corpus.expected_snapshot(item)["state"]["rflags"], 16)
            observed.append((old, result, flags))
        self.assertIn(((-corpus.OPERAND_ADDRESS) & corpus.MASK64, 0,
                       next(flags for old, result, flags in observed if old == ((-corpus.OPERAND_ADDRESS) & corpus.MASK64))), observed)
        self.assertTrue(any(old == 5 and flags & (1 << 4) for old, _, flags in observed), "AF carry control missing")
        self.assertTrue(any(old == 0x7FFFFFFFFFFFFFFF and flags & (1 << 11) for old, _, flags in observed), "signed overflow control missing")
        self.assertTrue(any(flags & 1 for _, _, flags in observed), "unsigned carry control missing")
        self.assertTrue(any(flags & 64 for _, _, flags in observed), "zero result control missing")
        self.assertEqual([x[0] for x in observed], list(corpus.CONTROL_MEMORY_VALUES))

    def test_generated_inputs_are_deterministic_unique_and_semantically_diverse(self):
        first, second = corpus.generated_fixtures(), corpus.generated_fixtures()
        self.assertEqual(first, second)
        self.assertEqual(len(first), 1024)
        self.assertEqual(len({(tuple(x["initial_state"]["gpr"]), x["initial_state"]["rflags"],
                               x["memory"][1]["initial_bytes_hex"], x["memory"][2]["initial_bytes_hex"]) for x in first}), 1024)
        self.assertEqual({x["memory"][0]["initial_bytes_hex"] for x in first}, {"480fc100"})
        counts = Counter()
        flag_bits = Counter()
        for item in first:
            state = item["initial_state"]
            self.assertEqual(int(state["gpr"][0], 16), corpus.OPERAND_ADDRESS)
            self.assertEqual(int(state["rflags"], 16) & ~corpus.RFLAGS_DEFINED, 0)
            data = bytes.fromhex(item["memory"][1]["initial_bytes_hex"] + item["memory"][2]["initial_bytes_hex"])
            old_memory = int.from_bytes(data[3:11], "little")
            counts[old_memory] += 1
            flags = int(corpus.expected_snapshot(item)["state"]["rflags"], 16)
            for bit in (0, 2, 4, 6, 7, 11): flag_bits[bit] += bool(flags & (1 << bit))
        self.assertEqual(set(counts), {value for _, value in corpus.MEMORY_CLASSES})
        self.assertEqual(set(counts.values()), {64})
        for bit in (0, 2, 4, 6, 7, 11):
            self.assertGreater(flag_bits[bit], 0, f"arithmetic flag bit {bit} never set")

    def test_manifest_and_strict_bundle_validation(self):
        self.assertEqual(self.manifest["case_count"], 1032)
        self.assertEqual(self.manifest["generated_case_count"], 1024)
        self.assertEqual(self.manifest["control_count"], 8)
        self.assertEqual(self.manifest["control_manifest_sha256"], corpus.CONTROL_MANIFEST_SHA256)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"; self.copy_bundle(bundle)
            self.assertEqual(corpus.verify_corpus(bundle), self.manifest)
            extra = bundle / "unexpected"; extra.write_text("x")
            with self.assertRaises(CorpusError): corpus.verify_corpus(bundle)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"; self.copy_bundle(bundle)
            target = bundle / "s2_42y_xadd_qword_memory_c00.case.json"
            target.write_bytes(target.read_bytes() + b" ")
            with self.assertRaises(CorpusError): corpus.verify_corpus(bundle)

    def test_complete_state_comparison_rejects_gpr_flag_rip_and_code_mutations(self):
        identifier = corpus.control_id(0)
        raw = corpus.control_inputs()[f"{identifier}.case.json"]
        expected = self.expected[identifier]
        valid = observation(raw, expected)
        corpus.compare_result(valid, raw, expected, BUILD, MACHINE, "valid")
        for mutate in (
            lambda x: x.__setitem__("schema_version", 2.0),
            lambda x: x.__setitem__("unexpected", True),
            lambda x: x["snapshot"].__setitem__("unexpected", True),
            lambda x: x["snapshot"]["value"].__setitem__("unexpected", True),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("unexpected", True),
            lambda x: x["snapshot"]["value"]["state"]["gpr"].__setitem__(0, corpus.hx(77)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(2)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rip", corpus.hx(3)),
            lambda x: x["snapshot"]["value"]["memory"][0].__setitem__("bytes_hex", "00"),
            lambda x: x["build"].__setitem__("unexpected", True),
            lambda x: x["machine"].__setitem__("page_size", True),
            lambda x: x["outcome"].__setitem__("unexpected", True),
            lambda x: x["instructions_executed"].__setitem__("value", 1),
            lambda x: x["snapshot"]["value"]["exception"].__setitem__("unexpected", True),
        ):
            changed = copy.deepcopy(valid); mutate(changed)
            with self.assertRaises(CorpusError): corpus.compare_result(changed, raw, expected, BUILD, MACHINE, "mutated")

    def test_interrupted_checkpoint_records_unfinished_attempts_and_native_is_blocked(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            result_path = root / "results.json"
            manifest = self.manifest
            row = {"case_id": corpus.control_id(0), "repeats": [{"ok": True}], "processes": [{"returncode": 0}]}
            corpus.write_checkpoint(result_path, bundle, manifest, [row], ["interrupted"], {"x": "x"}, MACHINE, BUILD, MACHINE)
            saved = json.loads(result_path.read_bytes())
            self.assertEqual(saved["status"], "incomplete")
            self.assertEqual(saved["missing_attempts"][0], {"case_id": corpus.control_id(0), "repeat": 2})
            native = root / "native.json"
            with mock.patch.object(corpus.subprocess, "run", side_effect=AssertionError("backend must not start")):
                corpus.write_native_blocked(native)
            record = json.loads(native.read_bytes())
            self.assertEqual(record["status"], "blocked")
            self.assertEqual(record["native_x86"], "not_run_form_not_admitted")
            self.assertEqual(record["backend_execution"], "not_started")

    def test_runner_interrupt_persists_completed_repeat_and_exact_missing_attempt(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"interruption test runner")
            output = root / "results.json"
            ident = corpus.control_id(0)
            raw = self.files[f"{ident}.case.json"]
            seen = observation(raw, self.expected[ident])
            stdout = json.dumps(seen, indent=2) + "\n"
            calls = 0

            def interrupt_after_one(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1:
                    return corpus.subprocess.CompletedProcess(command, 0, stdout=stdout, stderr="")
                raise KeyboardInterrupt

            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=interrupt_after_one):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, output, runner)
            checkpoint = json.loads(output.read_bytes())
            self.assertEqual(checkpoint["status"], "incomplete")
            first = checkpoint["results"][0]
            self.assertEqual(first["case_id"], ident)
            self.assertEqual(first["repeats"][0], seen)
            self.assertIn("interrupted", first["repeats"][1]["failure"])
            self.assertEqual(checkpoint["missing_attempts"][0], {"case_id": ident, "repeat": 2})

    def test_timeout_preserves_partial_streams_and_diagnosis(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"timeout test runner")
            output = root / "results.json"
            ident = corpus.control_id(0)
            raw = self.files[f"{ident}.case.json"]
            seen = observation(raw, self.expected[ident])
            calls = 0

            def timeout_then_success(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1:
                    raise corpus.subprocess.TimeoutExpired(command, corpus.RUNNER_TIMEOUT_SECONDS,
                        output=b"partial stdout \xff", stderr=b"partial stderr")
                return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen), stderr="")

            manifest = {"case_digests": [self.manifest["case_digests"][0]]}
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=timeout_then_success):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, output, runner)
            saved = json.loads(output.read_bytes())
            self.assertEqual(saved["status"], "failed")
            timed_out = saved["results"][0]["processes"][0]
            self.assertEqual(timed_out["stdout"], "partial stdout �")
            self.assertEqual(timed_out["stderr"], "partial stderr")
            self.assertEqual(timed_out["timeout_diagnosis"]["kind"], "timeout_expired")
            self.assertEqual(timed_out["timeout_diagnosis"]["timeout_seconds"], corpus.RUNNER_TIMEOUT_SECONDS)
            self.assertIn("timed out", saved["failures"][0])

    def test_runtime_repeat_results_must_be_identical(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"repeat stability runner")
            output = root / "results.json"
            ident = corpus.control_id(0)
            raw = self.files[f"{ident}.case.json"]
            seen = observation(raw, self.expected[ident])
            calls = 0

            def varying_runner(command, **_kwargs):
                nonlocal calls
                calls += 1
                result = copy.deepcopy(seen)
                if calls == 2:
                    result["run_nonce"] = "changed"
                return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(result), stderr="")

            manifest = {"case_digests": [self.manifest["case_digests"][0]]}
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus, "compare_result"), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=varying_runner):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, output, runner)
            saved = json.loads(output.read_bytes())
            self.assertEqual(saved["status"], "failed")
            self.assertEqual(len(saved["results"][0]["repeats"]), 2)
            self.assertTrue(any("repeat results differ" in failure for failure in saved["failures"]))

    def test_runner_rejects_non_object_json_and_overwrites_stale_success_before_preflight(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"malformed output runner")
            output = root / "results.json"
            manifest = {"case_digests": [self.manifest["case_digests"][0]]}
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            fake = lambda command, **_kwargs: corpus.subprocess.CompletedProcess(command, 0, stdout="[]", stderr="")
            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=fake):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, output, runner)
            malformed = json.loads(output.read_bytes())
            self.assertEqual(malformed["status"], "failed")
            self.assertTrue(any("one JSON object" in failure for failure in malformed["failures"]))

            typed_path = root / "typed.json"
            identifier = corpus.control_id(0)
            raw = self.files[f"{identifier}.case.json"]
            wrong_type = observation(raw, self.expected[identifier])
            wrong_type["snapshot"]["value"]["state"]["gpr"] = "not-an-array"
            fake_typed = lambda command, **_kwargs: corpus.subprocess.CompletedProcess(
                command, 0, stdout=json.dumps(wrong_type), stderr="")
            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=fake_typed):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, typed_path, runner)
            typed = json.loads(typed_path.read_bytes())
            self.assertEqual(typed["status"], "failed")
            self.assertTrue(any("complete GPR/RIP/RFLAGS differs" in failure for failure in typed["failures"]))

            stale_path = root / "stale.json"
            stale_path.write_bytes(json_bytes({"status": "passed", "failures": []}))
            with contextlib.redirect_stderr(__import__("io").StringIO()):
                code = corpus.main(["--run-interpreter", "--corpus", str(root / "missing"),
                    "--results", str(stale_path), "--runner", str(runner)])
            replaced = json.loads(stale_path.read_bytes())
            self.assertEqual(code, 1)
            self.assertEqual(replaced["status"], "failed")
            self.assertTrue(replaced["failures"])

    def test_saved_replay_binds_full_repeated_observation_and_provenance(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"review test runner identity")
            rows = []
            for item in self.manifest["case_digests"]:
                ident = item["case_id"]; raw = self.files[f"{ident}.case.json"]; expected = self.expected[ident]
                seen = observation(raw, expected); out = json.dumps(seen, indent=2) + "\n"
                fixture_path = str((bundle / f"{ident}.case.json").resolve())
                proc = {"returncode": 0, "stdout": out, "stderr": "", "command": [str(runner.resolve()), "--fixture", fixture_path, "--engine", "interpreter", "--json"], "cwd": str(corpus.ROOT.resolve()), "environment": corpus.process_environment(), "timeout_diagnosis": None}
                rows.append({"case_id": ident, "fixture_sha256": sha256(raw), "repeats": [seen, copy.deepcopy(seen)], "processes": [proc, copy.deepcopy(proc)]})
            result = {"schema_version": 1, "status": "passed", "backend": "interpreter", "native_x86": "not_run_form_not_admitted", "profile": corpus.PROFILE,
                "manifest_sha256": sha256(self.files["manifest.json"]), "case_count": corpus.TOTAL_COUNT, "repeats_per_case": 2,
                "execution_identity": {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]},
                "execution_machine": {key: MACHINE[key] for key in ("host_os", "host_arch")},
                "runner_build": BUILD, "runner_machine": MACHINE, "results": rows, "failures": [], "missing_attempts": []}
            path = root / "results.json"; path.write_bytes(json_bytes(result))
            corpus.verify_results(bundle, path, runner)
            altered = copy.deepcopy(result); altered["results"][0]["processes"][0]["command"] = []
            path.write_bytes(json_bytes(altered))
            with self.assertRaises(CorpusError): corpus.verify_results(bundle, path, runner)

            false_returncode = copy.deepcopy(result)
            false_returncode["results"][0]["processes"][0]["returncode"] = False
            path.write_bytes(json_bytes(false_returncode))
            with self.assertRaises(CorpusError): corpus.verify_results(bundle, path, runner)

            malformed_stderr = copy.deepcopy(result)
            malformed_stderr["results"][0]["processes"][0]["stderr"] = 7
            path.write_bytes(json_bytes(malformed_stderr))
            with self.assertRaises(CorpusError): corpus.verify_results(bundle, path, runner)

            for mutate in (
                lambda seen: seen.__setitem__("schema_version", 2.0),
                lambda seen: seen.__setitem__("unexpected", True),
                lambda seen: seen["snapshot"]["value"].__setitem__("unexpected", True),
            ):
                malformed_result = copy.deepcopy(result)
                row = malformed_result["results"][0]
                for repeat in range(2):
                    observation_copy = copy.deepcopy(row["repeats"][repeat])
                    mutate(observation_copy)
                    row["repeats"][repeat] = observation_copy
                    row["processes"][repeat]["stdout"] = json.dumps(observation_copy, indent=2) + "\n"
                path.write_bytes(json_bytes(malformed_result))
                with self.assertRaises(CorpusError): corpus.verify_results(bundle, path, runner)

            stdout_float_version = copy.deepcopy(result)
            first_proc = stdout_float_version["results"][0]["processes"][0]
            stdout_observation = json.loads(first_proc["stdout"])
            stdout_observation["schema_version"] = 2.0
            first_proc["stdout"] = json.dumps(stdout_observation, indent=2) + "\n"
            path.write_bytes(json_bytes(stdout_float_version))
            with self.assertRaises(CorpusError): corpus.verify_results(bundle, path, runner)

            malformed_summary = copy.deepcopy(result)
            malformed_summary["schema_version"] = 1.0
            path.write_bytes(json_bytes(malformed_summary))
            with self.assertRaises(CorpusError): corpus.verify_results(bundle, path, runner)


if __name__ == "__main__": unittest.main()
