"""Focused contract and evidence tests for S2-42ae."""
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

SCRIPT = Path(__file__).with_name("generate_stage2_bsf_bsr_qword_corpus.py")
SPEC = importlib.util.spec_from_file_location("s2_42ae", SCRIPT)
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


class BitScanQwordCorpusTests(unittest.TestCase):
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

    def test_exact_forms_controls_and_independent_bit_scan_oracle(self):
        self.assertEqual(len(corpus.FORMS), 4)
        self.assertEqual(len(self.controls), 4 * corpus.CONTROL_PER_FORM)
        expected_codes = {"bsf_rcx": "480fbcc1", "bsf_rax": "480fbcc0",
                          "bsr_rcx": "480fbdc1", "bsr_rax": "480fbdc0"}
        for form, (_, source_reg, _) in corpus.FORMS.items():
            self.assertEqual(corpus.FORMS[form][2].hex(), expected_codes[form])
            for i, source in enumerate(corpus.CONTROL_VALUES):
                ident = corpus.control_id(form, i)
                item = json.loads(self.controls[f"{ident}.case.json"])
                initial = item["initial_state"]
                gprs = [int(x, 16) for x in initial["gpr"]]
                expected = corpus.expected_snapshot(item)
                actual_gprs = [int(x, 16) for x in expected["state"]["gpr"]]
                actual_source = gprs[1] if source_reg == "rcx" else gprs[0]
                self.assertEqual(actual_source, source)
                if source_reg == "rax":
                    self.assertEqual(actual_gprs[0], gprs[0] if source == 0 else (source & -source).bit_length() - 1 if form.startswith("bsf") else source.bit_length() - 1)
                elif source == 0:
                    self.assertEqual(actual_gprs[0], gprs[0])
                else:
                    self.assertEqual(actual_gprs[0], (source & -source).bit_length() - 1 if form.startswith("bsf") else source.bit_length() - 1)
                old_flags = int(initial["rflags"], 16)
                expected_arithmetic = (0x40 if source == 0 else 0) | (0x4 if source.bit_count() % 2 == 0 else 0)
                self.assertEqual(int(expected["state"]["rflags"], 16) & corpus.RFLAGS_MASK, expected_arithmetic)
                self.assertEqual(int(expected["state"]["rflags"], 16) & ~corpus.RFLAGS_MASK, old_flags & ~corpus.RFLAGS_MASK)
                self.assertEqual(expected["state"]["rflags_defined"], corpus.hx(corpus.RFLAGS_DEFINED))
                self.assertEqual(expected["memory"][0]["bytes_hex"], expected_codes[form])
                self.assertEqual(expected["instructions_executed"]["value"], corpus.hx(1))
                self.assertIsNone(expected["exception"])

    def test_generated_inputs_are_deterministic_and_unique_for_each_exact_form(self):
        first, second = corpus.generated_fixtures(), corpus.generated_fixtures()
        self.assertEqual(first, second)
        for form in corpus.FORMS:
            cases = [item for item in first if item["case_id"].startswith(f"s2_42ae_{form}_")]
            self.assertEqual(len(cases), corpus.GENERATED_PER_FORM)
            states = {(tuple(item["initial_state"]["gpr"]), item["initial_state"]["rflags"],
                       item["memory"][0]["initial_bytes_hex"]) for item in cases}
            self.assertEqual(len(states), corpus.GENERATED_PER_FORM)
            self.assertEqual(len({item["seed"] for item in cases}), len(corpus.SEEDS))

    def test_pinned_control_manifest_and_expected_states_are_strict(self):
        controls, raw = corpus.load_controls()
        self.assertEqual(len(controls), len(corpus.FORMS) * corpus.CONTROL_PER_FORM)
        self.assertEqual(corpus.CONTROL_MANIFEST_SHA256, sha256(raw))
        changed = copy.deepcopy(json.loads(controls[0][1]))
        changed["memory"][0]["initial_bytes_hex"] = "480fbdc0"
        with self.assertRaises(CorpusError):
            corpus.expected_snapshot(changed)

    def test_bundle_digest_file_set_and_duplicate_json_keys_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            manifest = corpus.verify_corpus(bundle)
            self.assertEqual(manifest["case_count"], corpus.TOTAL_COUNT)
            (bundle / "extra.json").write_text("{}")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)
        with self.assertRaises(CorpusError):
            json.loads('{"case_count":1,"case_count":2}', object_pairs_hook=strict_object)

    def test_full_snapshot_oracle_rejects_state_exit_and_provenance_changes(self):
        ident = corpus.control_id("bsf_rcx", 0)
        raw = self.files[f"{ident}.case.json"]
        expected = self.expected[ident]
        seen = observation(raw, expected)
        corpus.compare_result(seen, raw, expected, BUILD, MACHINE, "control")
        for mutate in (
            lambda x: x["snapshot"]["value"]["state"]["gpr"].__setitem__(0, corpus.hx(7)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(0)),
            lambda x: x["outcome"].__setitem__("kind", "timeout"),
            lambda x: x["snapshot"]["value"].__setitem__("rflags_defined", corpus.hx(0)),
        ):
            bad = copy.deepcopy(seen)
            mutate(bad)
            with self.assertRaises(CorpusError):
                corpus.compare_result(bad, raw, expected, BUILD, MACHINE, "mutated observation")

    def test_interruption_checkpoint_keeps_first_repeat_and_marks_missing_second(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = root / "corpus"
            self.copy_bundle(bundle)
            runner = root / "od-runner"
            runner.write_bytes(b"runner identity")
            output = root / "results.json"
            ident = corpus.control_id("bsf_rcx", 0)
            raw = self.files[f"{ident}.case.json"]
            seen = observation(raw, self.expected[ident])
            calls = 0

            def run_then_interrupt(command, **_kwargs):
                nonlocal calls
                calls += 1
                if calls == 1:
                    return corpus.subprocess.CompletedProcess(command, 0, stdout=json.dumps(seen), stderr="")
                raise KeyboardInterrupt

            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=run_then_interrupt):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, output, runner)
            saved = json.loads(output.read_bytes())
            self.assertEqual(saved["results"][0]["repeats"][0], seen)
            self.assertEqual(saved["missing_attempts"][0], {"case_id": ident, "repeat": 2})
            self.assertEqual(saved["native_x86"], "not_run/not_selected")


if __name__ == "__main__":
    unittest.main()
