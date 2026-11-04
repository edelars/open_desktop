"""Boundary, independent-oracle, manifest, replay, and interruption checks for S2-42aa."""
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

SCRIPT = Path(__file__).with_name("generate_stage2_bswap_corpus.py")
SPEC = importlib.util.spec_from_file_location("s2_42aa", SCRIPT)
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


class BswapCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.controls = corpus.control_inputs()
        cls.control_rows, cls.control_manifest = corpus.load_controls()
        cls.files = corpus.materialize()
        cls.manifest = json.loads(cls.files["manifest.json"])
        cls.expected = {x["case_id"]: x for x in json.loads(cls.files["expected.json"])["cases"]}

    def copy_bundle(self, target: Path):
        target.mkdir()
        for name, contents in self.files.items(): (target / name).write_bytes(contents)

    def test_exact_encodings_pinned_boundaries_and_independent_byte_oracle(self):
        self.assertEqual(len(self.controls), 32)
        self.assertEqual(len(self.control_rows), 32)
        self.assertEqual(corpus.CONTROL_MANIFEST_SHA256, sha256(self.control_manifest))
        for width, code, values in ((32, "0fc8", corpus.CONTROL_VALUES_32),
                                    (64, "480fc8", corpus.CONTROL_VALUES_64)):
            for i, value in enumerate(values):
                ident = corpus.control_id(width, i)
                item = json.loads(self.controls[f"{ident}.case.json"])
                before = [int(x, 16) for x in item["initial_state"]["gpr"]]
                expected = corpus.expected_snapshot(item)
                after = [int(x, 16) for x in expected["state"]["gpr"]]
                reversed_value = int.from_bytes((value & ((1 << width) - 1)).to_bytes(width // 8, "little")[::-1], "little")
                wanted = reversed_value if width == 64 else reversed_value & corpus.MASK32
                self.assertEqual(item["memory"][0]["initial_bytes_hex"], code)
                self.assertEqual(item["memory"][0]["length"], len(bytes.fromhex(code)))
                self.assertEqual(item["stop_rip"], corpus.hx(corpus.RIP + len(bytes.fromhex(code))))
                self.assertEqual(item["instruction_budget"], 1)
                self.assertEqual(before[0], value if width == 64 else ((before[0] >> 32) << 32) | value)
                self.assertEqual(after[0], wanted)
                self.assertEqual(after[1:], before[1:])
                self.assertEqual(expected["state"]["rflags"], item["initial_state"]["rflags"])
                self.assertEqual(expected["memory"][0]["bytes_hex"], code)
                self.assertEqual(expected["outcome"], {"kind": "stop_address"})
                self.assertEqual(expected["instructions_executed"]["value"], corpus.hx(1))
                self.assertIsNone(expected["exception"])

    def test_eax_zero_extends_high_half_and_rax_changes_all_64_bits(self):
        eax = json.loads(self.controls[f"{corpus.control_id(32, 0)}.case.json"])
        rax = json.loads(self.controls[f"{corpus.control_id(64, 2)}.case.json"])
        eax_before = int(eax["initial_state"]["gpr"][0], 16)
        eax_after = int(corpus.expected_snapshot(eax)["state"]["gpr"][0], 16)
        rax_before = int(rax["initial_state"]["gpr"][0], 16)
        rax_after = int(corpus.expected_snapshot(rax)["state"]["gpr"][0], 16)
        self.assertEqual(eax_before >> 32, 0xFFFFFFFF)
        self.assertEqual(eax_after >> 32, 0)
        self.assertNotEqual(rax_before >> 32, rax_after >> 32)
        self.assertNotEqual(rax_before & corpus.MASK32, rax_after & corpus.MASK32)

    def test_1024_unique_generated_inputs_per_width_are_stable(self):
        first, second = corpus.generated_fixtures(), corpus.generated_fixtures()
        self.assertEqual(first, second)
        for width, code in ((32, "0fc8"), (64, "480fc8")):
            cases = [x for x in first if x["case_id"].startswith(f"s2_42aa_bswap_{width}_")]
            self.assertEqual(len(cases), 1024)
            states = {(tuple(x["initial_state"]["gpr"]), x["initial_state"]["rflags"], x["memory"][0]["initial_bytes_hex"]) for x in cases}
            self.assertEqual(len(states), 1024)
            destination_values = {x["initial_state"]["gpr"][0] for x in cases}
            self.assertEqual(len(destination_values), 1024)
            self.assertEqual({x["memory"][0]["initial_bytes_hex"] for x in cases}, {code})
            for item in cases:
                initial = [int(x, 16) for x in item["initial_state"]["gpr"]]
                after = [int(x, 16) for x in corpus.expected_snapshot(item)["state"]["gpr"]]
                self.assertEqual(after[1:], initial[1:])
                self.assertEqual(corpus.expected_snapshot(item)["state"]["rflags"], item["initial_state"]["rflags"])

    def test_manifest_is_strict_and_rejects_mutation_and_duplicate_keys(self):
        self.assertEqual(self.manifest["case_count"], 2080)
        self.assertEqual(self.manifest["generated_per_width"], 1024)
        self.assertEqual(self.manifest["controls_per_width"], 16)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"; self.copy_bundle(bundle)
            self.assertEqual(corpus.verify_corpus(bundle), self.manifest)
            (bundle / "unexpected").write_text("x")
            with self.assertRaises(CorpusError): corpus.verify_corpus(bundle)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"; self.copy_bundle(bundle)
            target = bundle / f"{corpus.control_id(32, 0)}.case.json"
            target.write_bytes(target.read_bytes() + b" ")
            with self.assertRaises(CorpusError): corpus.verify_corpus(bundle)
        with self.assertRaises(CorpusError): json.loads('{"a":1,"a":2}', object_pairs_hook=strict_object)

    def test_complete_observation_rejects_any_state_exit_or_provenance_change(self):
        ident = corpus.control_id(32, 0); raw = self.controls[f"{ident}.case.json"]
        expected = self.expected[ident]; valid = observation(raw, expected)
        corpus.compare_result(valid, raw, expected, BUILD, MACHINE, "valid")
        mutations = (
            lambda x: x.__setitem__("unexpected", True),
            lambda x: x["snapshot"]["value"]["state"]["gpr"].__setitem__(1, corpus.hx(9)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(3)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rip", corpus.hx(3)),
            lambda x: x["snapshot"]["value"]["memory"][0].__setitem__("bytes_hex", "00"),
            lambda x: x["instructions_executed"].__setitem__("value", corpus.hx(0)),
            lambda x: x["build"].__setitem__("unexpected", True),
            lambda x: x["machine"].__setitem__("page_size", True),
            lambda x: x["outcome"].__setitem__("kind", "fault"),
        )
        for mutate in mutations:
            changed = copy.deepcopy(valid); mutate(changed)
            with self.assertRaises(CorpusError): corpus.compare_result(changed, raw, expected, BUILD, MACHINE, "mutated")

    def test_interruption_saves_completed_attempt_and_lists_missing_repeat(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"runner identity")
            output = root / "results.json"; ident = corpus.control_id(32, 0)
            raw = self.files[f"{ident}.case.json"]; seen = observation(raw, self.expected[ident]); calls = 0
            def interrupt_after_one(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1: return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen), stderr="")
                raise KeyboardInterrupt
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), mock.patch.object(corpus.subprocess, "run", side_effect=interrupt_after_one):
                with self.assertRaises(CorpusError): corpus.run_interpreter(bundle, output, runner)
            saved = json.loads(output.read_bytes())
            self.assertEqual(saved["status"], "incomplete")
            self.assertEqual(saved["results"][0]["repeats"][0], seen)
            self.assertEqual(saved["missing_attempts"][0], {"case_id": ident, "repeat": 2})

    def test_saved_replay_rejects_duplicate_keys_and_incomplete_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"runner identity")
            result = root / "results.json"
            result.write_text('{"schema_version":1,"schema_version":1}')
            with self.assertRaises(CorpusError): corpus.verify_results(bundle, result, runner)
            result.write_text(json.dumps({"schema_version": 1, "status": "incomplete"}))
            with self.assertRaises(CorpusError): corpus.verify_results(bundle, result, runner)

    def test_saved_source_revision_must_match_the_checked_out_commit(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"runner identity")
            result_path = root / "results.json"
            result = {"schema_version": 1, "status": "passed", "backend": "interpreter",
                "native_x86": "not_run_form_not_admitted", "profile": corpus.PROFILE,
                "manifest_sha256": sha256((bundle / "manifest.json").read_bytes()),
                "case_count": corpus.TOTAL_COUNT, "repeats_per_case": corpus.REPEATS,
                "execution_identity": {"executable_sha256": sha256(runner.read_bytes()), "revision": "0" * 40},
                "execution_machine": {"host_os": MACHINE["host_os"], "host_arch": MACHINE["host_arch"]},
                "runner_build": {**BUILD, "revision": "unavailable"}, "runner_machine": MACHINE,
                "results": [], "failures": [], "missing_attempts": []}
            result_path.write_bytes(json_bytes(result))
            with self.assertRaisesRegex(CorpusError, "saved runner/source provenance differs"):
                corpus.verify_results(bundle, result_path, runner)

    def test_saved_case_and_repeat_counts_require_exact_integer_types(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"runner identity")
            result_path = root / "results.json"
            result = {"schema_version": 1, "status": "passed", "backend": "interpreter",
                "native_x86": "not_run_form_not_admitted", "profile": corpus.PROFILE,
                "manifest_sha256": sha256((bundle / "manifest.json").read_bytes()),
                "case_count": corpus.TOTAL_COUNT, "repeats_per_case": corpus.REPEATS,
                "execution_identity": {"executable_sha256": sha256(runner.read_bytes()), "revision": "0" * 40},
                "execution_machine": {"host_os": MACHINE["host_os"], "host_arch": MACHINE["host_arch"]},
                "runner_build": {**BUILD, "revision": "unavailable"}, "runner_machine": MACHINE,
                "results": [], "failures": [], "missing_attempts": []}
            for field, value in (("case_count", float(corpus.TOTAL_COUNT)), ("repeats_per_case", float(corpus.REPEATS))):
                changed = copy.deepcopy(result); changed[field] = value; result_path.write_bytes(json_bytes(changed))
                with self.assertRaisesRegex(CorpusError, "saved evidence is not a complete success"):
                    corpus.verify_results(bundle, result_path, runner)

    def test_non_object_runner_json_preserves_first_repeat_in_checkpoint(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"runner identity")
            output = root / "results.json"; ident = corpus.control_id(32, 0)
            raw = self.files[f"{ident}.case.json"]; seen = observation(raw, self.expected[ident]); calls = 0
            def malformed_then_interrupt(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1: return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen), stderr="")
                if calls == 2: return corpus.subprocess.CompletedProcess(command, 0, stdout="[]", stderr="")
                raise KeyboardInterrupt
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), mock.patch.object(corpus.subprocess, "run", side_effect=malformed_then_interrupt):
                with self.assertRaises(CorpusError): corpus.run_interpreter(bundle, output, runner)
            saved = json.loads(output.read_bytes()); first = saved["results"][0]
            self.assertEqual(first["repeats"][0], seen)
            self.assertIn("failure", first["repeats"][1])
            self.assertEqual(len(first["processes"]), 2)
            self.assertEqual(saved["missing_attempts"][0], {"case_id": ident, "repeat": 2})

    def test_native_is_blocked_without_starting_a_backend(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "native.json"
            with mock.patch.object(corpus.subprocess, "run", side_effect=AssertionError("native backend must not start")):
                corpus.write_native_blocked(path)
            record = json.loads(path.read_bytes())
            self.assertEqual(record["status"], "blocked")
            self.assertEqual(record["native_x86"], "not_run_form_not_admitted")
            self.assertEqual(record["backend_execution"], "not_started")


if __name__ == "__main__": unittest.main()
