"""Focused contract, uniqueness, and evidence tests for S2-42af."""
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
from stage2_corpus_support import CorpusError, sha256, strict_object

SCRIPT = Path(__file__).with_name("generate_stage2_bt_family_qword_register_corpus.py")
SPEC = importlib.util.spec_from_file_location("s2_42af", SCRIPT)
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


class BitFamilyQwordCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.controls = corpus.control_inputs()
        cls.files = corpus.materialize()
        cls.manifest = json.loads(cls.files["manifest.json"])
        cls.expected = {row["case_id"]: row for row in json.loads(cls.files["expected.json"])["cases"]}

    def copy_bundle(self, target: Path):
        target.mkdir()
        for name, contents in self.files.items():
            (target / name).write_bytes(contents)

    def test_exact_encodings_pinned_controls_and_integer_oracle(self):
        expected_codes = {"bt": "480fa3c8", "bts": "480fabc8", "btr": "480fb3c8", "btc": "480fbbc8"}
        self.assertEqual(set(corpus.FORMS), set(expected_codes))
        self.assertEqual(len(self.controls), 4 * corpus.CONTROL_PER_FORM)
        for form, (operation, code) in corpus.FORMS.items():
            self.assertEqual(code.hex(), expected_codes[form])
            old_bits = set()
            for i, (destination, offset) in enumerate(corpus.CONTROL_INPUTS):
                ident = corpus.control_id(form, i)
                item = json.loads(self.controls[f"{ident}.case.json"])
                initial = item["initial_state"]
                before = [int(x, 16) for x in initial["gpr"]]
                expected = corpus.expected_snapshot(item)
                after = [int(x, 16) for x in expected["state"]["gpr"]]
                self.assertEqual(before[:2], [destination, offset])
                index = offset & 63
                old_bit = (destination >> index) & 1
                old_bits.add(old_bit)
                wanted = destination
                if operation == "bts": wanted |= 1 << index
                elif operation == "btr": wanted &= ~(1 << index) & corpus.MASK64
                elif operation == "btc": wanted ^= 1 << index
                self.assertEqual(after[0], wanted)
                self.assertEqual(after[1:], before[1:])
                old_flags = int(initial["rflags"], 16)
                flags = int(expected["state"]["rflags"], 16)
                self.assertEqual(flags, (old_flags & ~1) | old_bit)
                self.assertEqual(flags & corpus.RFLAGS_MASK, (old_flags & (corpus.RFLAGS_MASK & ~1)) | old_bit)
                self.assertEqual(expected["state"]["rflags_defined"], corpus.hx(corpus.RFLAGS_MASK))
                self.assertEqual(expected["memory"][0]["bytes_hex"], expected_codes[form])
                self.assertEqual(expected["instructions_executed"]["value"], corpus.hx(1))
                self.assertIsNone(expected["exception"])
            self.assertEqual(old_bits, {0, 1}, f"{form}: controls must exercise CF=0 and CF=1")

    def test_generated_states_are_deterministic_unique_and_cover_modulo_indices(self):
        first, second = corpus.generated_fixtures(), corpus.generated_fixtures()
        self.assertEqual(first, second)
        for form in corpus.FORMS:
            cases = [row for row in first if row["case_id"].startswith(f"s2_42af_{form}_")]
            self.assertEqual(len(cases), corpus.GENERATED_PER_FORM)
            states = {(tuple(row["initial_state"]["gpr"]), row["initial_state"]["rflags"],
                       row["memory"][0]["initial_bytes_hex"]) for row in cases}
            self.assertEqual(len(states), corpus.GENERATED_PER_FORM)
            offsets = [int(row["initial_state"]["gpr"][1], 16) for row in cases]
            self.assertEqual({value & 63 for value in offsets}, set(range(64)))
            self.assertIn(64, offsets)
            self.assertIn(127, offsets)
            self.assertTrue(any(value > 0xFFFFFFFF for value in offsets))
            selected = [((int(row["initial_state"]["gpr"][0], 16) >> (int(row["initial_state"]["gpr"][1], 16) & 63)) & 1) for row in cases]
            self.assertEqual(selected.count(0), selected.count(1))

    def test_pinned_controls_and_oracle_reject_encoding_or_manifest_drift(self):
        controls, raw = corpus.load_controls()
        self.assertEqual(len(controls), 4 * corpus.CONTROL_PER_FORM)
        self.assertEqual(corpus.CONTROL_MANIFEST_SHA256, sha256(raw))
        changed = copy.deepcopy(json.loads(controls[0][1]))
        changed["memory"][0]["initial_bytes_hex"] = "480fbbc0"
        with self.assertRaises(CorpusError): corpus.expected_snapshot(changed)

    def test_bundle_digest_file_set_and_duplicate_json_keys_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            manifest = corpus.verify_corpus(bundle)
            self.assertEqual(manifest["case_count"], corpus.TOTAL_COUNT)
            (bundle / "extra.json").write_text("{}")
            with self.assertRaises(CorpusError): corpus.verify_corpus(bundle)
        with self.assertRaises(CorpusError):
            json.loads('{"case_count":1,"case_count":2}', object_pairs_hook=strict_object)

    def test_full_snapshot_comparison_rejects_state_exit_and_definedness_changes(self):
        ident = corpus.control_id("bt", 0)
        raw, expected = self.files[f"{ident}.case.json"], self.expected[ident]
        seen = observation(raw, expected)
        corpus.compare_result(seen, raw, expected, BUILD, MACHINE, "control")
        undefined_changed = copy.deepcopy(seen)
        flags = int(undefined_changed["snapshot"]["value"]["state"]["rflags"], 16) ^ 0x4
        undefined_changed["snapshot"]["value"]["state"]["rflags"] = corpus.hx(flags)
        corpus.compare_result(undefined_changed, raw, expected, BUILD, MACHINE, "undefined PF may differ")
        for mutate in (
            lambda x: x["snapshot"]["value"]["state"]["gpr"].__setitem__(0, corpus.hx(7)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(0)),
            lambda x: x["outcome"].__setitem__("kind", "timeout"),
            lambda x: x["snapshot"]["value"].__setitem__("rflags_defined", corpus.hx(0)),
        ):
            bad = copy.deepcopy(seen)
            mutate(bad)
            with self.assertRaises(CorpusError): corpus.compare_result(bad, raw, expected, BUILD, MACHINE, "mutated")

    def test_interruption_checkpoint_retains_completed_attempt_and_missing_repeat(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"runner identity")
            output = root / "results.json"
            ident = corpus.control_id("bt", 0); raw = self.files[f"{ident}.case.json"]
            seen = observation(raw, self.expected[ident]); calls = 0
            def run_then_interrupt(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1: return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen), stderr="")
                raise KeyboardInterrupt
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=run_then_interrupt):
                with self.assertRaises(CorpusError): corpus.run_interpreter(bundle, output, runner)
            saved = json.loads(output.read_bytes())
            self.assertEqual(saved["results"][0]["repeats"][0], seen)
            self.assertEqual(saved["missing_attempts"][0], {"case_id": ident, "repeat": 2})
            self.assertEqual(saved["native_x86"], "not_run/not_selected")

    def test_invalid_utf8_on_second_repeat_preserves_first_result_and_process_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"runner identity")
            output = root / "results.json"
            ident = corpus.control_id("bt", 0); raw = self.files[f"{ident}.case.json"]
            seen = observation(raw, self.expected[ident]); calls = 0
            def success_then_invalid_utf8(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1:
                    return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen).encode(), stderr=b"")
                return corpus.subprocess.CompletedProcess(command, 0, stdout=b"\xff", stderr=b"\xfe")
            manifest = {"case_digests": [self.manifest["case_digests"][0]]}
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=success_then_invalid_utf8):
                with self.assertRaises(CorpusError): corpus.run_interpreter(bundle, output, runner)
            saved = json.loads(output.read_bytes())
            result = saved["results"][0]
            self.assertEqual(saved["status"], "failed")
            self.assertEqual(result["repeats"][0], seen)
            self.assertIn("failure", result["repeats"][1])
            self.assertEqual([row["returncode"] for row in result["processes"]], [0, 0])
            self.assertIn("stdout_hex=ff", result["processes"][1]["stderr"])
            self.assertIn("stderr_hex=fe", result["processes"][1]["stderr"])
            self.assertEqual(saved["missing_attempts"], [{"case_id": ident, "repeat": 2}])

            calls = 0
            def success_then_utf16_json(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1:
                    return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen).encode(), stderr=b"")
                return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen).encode("utf-16-le"), stderr=b"")
            utf16_output = root / "utf16-results.json"
            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=success_then_utf16_json):
                with self.assertRaises(CorpusError): corpus.run_interpreter(bundle, utf16_output, runner)
            utf16_saved = json.loads(utf16_output.read_bytes())
            self.assertEqual(utf16_saved["results"][0]["repeats"][0], seen)
            self.assertIn("failure", utf16_saved["results"][0]["repeats"][1])
            self.assertEqual(utf16_saved["missing_attempts"], [{"case_id": ident, "repeat": 2}])


if __name__ == "__main__":
    unittest.main()
