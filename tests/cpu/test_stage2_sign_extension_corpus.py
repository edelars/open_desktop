"""Pinned controls, independent oracle, strict replay, and denial tests for S2-42ac."""
from __future__ import annotations

import contextlib
import copy
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent))
from stage2_corpus_support import CorpusError, json_bytes, sha256

SCRIPT = Path(__file__).with_name("generate_stage2_sign_extension_corpus.py")
SPEC = importlib.util.spec_from_file_location("s2_42ac", SCRIPT)
corpus = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(corpus)

BUILD = {"revision": "1" * 40, "compiler": "rustc test", "target": "aarch64-apple-darwin", "profile": "debug"}
MACHINE = {"host_os": "macos", "host_arch": "aarch64"}


def observation(raw: bytes, expected: dict) -> dict:
    return {"schema_version": 2, "case_id": expected["case_id"], "input_sha256": sha256(raw),
            "profile": corpus.PROFILE, "backend": "interpreter", "outcome": expected["outcome"],
            "instructions_executed": expected["instructions_executed"], "build": BUILD, "machine": MACHINE,
            "snapshot": {"status": "available", "value": {
                "state": {k: expected["state"][k] for k in ("mode", "gpr", "rip", "rflags")},
                "rflags_defined": expected["state"]["rflags_defined"], "memory": expected["memory"],
                "minss_memory_rax_controls": None, "addss_memory_rax_controls": None,
                "exception": {"status": "available", "value": None}}}}


class SignExtensionCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = corpus.materialize()
        cls.manifest = json.loads(cls.files["manifest.json"])
        cls.expected = {x["case_id"]: x for x in json.loads(cls.files["expected.json"])["cases"]}

    def copy_bundle(self, target: Path):
        target.mkdir()
        for name, content in self.files.items():
            (target / name).write_bytes(content)

    def test_pinned_controls_and_exact_architectural_oracle(self):
        controls, raw_manifest = corpus.load_controls()
        self.assertEqual(sha256(raw_manifest), corpus.CONTROL_MANIFEST_SHA256)
        self.assertEqual(len(controls), 32)
        for opname, encoding in corpus.ENCODINGS.items():
            rows = [x for x in controls if f"_{opname}_" in x[0]]
            self.assertEqual(len(rows), 16)
            self.assertEqual({json.loads(x[1])["memory"][0]["initial_bytes_hex"] for x in rows}, {encoding.hex()})
        for ident, raw, expected in controls:
            item = json.loads(raw)
            self.assertEqual(expected, corpus.expected_snapshot(item))
            before = item["initial_state"]
            after = expected["state"]
            self.assertEqual(after["rip"], corpus.hx(corpus.RIP + 2))
            self.assertEqual(after["rflags"], before["rflags"])
            if item["memory"][0]["initial_bytes_hex"] == "4898":
                eax = int(before["gpr"][0], 16) & corpus.MASK32
                want = eax | (0xFFFFFFFF00000000 if eax & 0x80000000 else 0)
                self.assertEqual(after["gpr"][0], corpus.hx(want))
            else:
                rax = int(before["gpr"][0], 16)
                self.assertEqual(after["gpr"][0], before["gpr"][0])
                self.assertEqual(after["gpr"][2], corpus.hx(corpus.MASK64 if rax >> 63 else 0))

    def test_generated_inputs_are_deterministic_unique_and_balanced(self):
        first, second = corpus.generated_fixtures(), corpus.generated_fixtures()
        self.assertEqual(first, second)
        self.assertEqual(len(first), 2048)
        for opname, encoding in corpus.ENCODINGS.items():
            selected = [x for x in first if x["memory"][0]["initial_bytes_hex"] == encoding.hex()]
            self.assertEqual(len(selected), 1024)
            self.assertEqual(len({(tuple(x["initial_state"]["gpr"]), x["initial_state"]["rflags"]) for x in selected}), 1024)
            if opname == "cdqe":
                outcomes = {int(x["initial_state"]["gpr"][0], 16) >> 31 & 1 for x in selected}
                self.assertEqual(outcomes, {0, 1})
            else:
                outcomes = {int(x["initial_state"]["gpr"][0], 16) >> 63 for x in selected}
                self.assertEqual(outcomes, {0, 1})

    def test_manifest_and_file_set_are_exact_and_tamper_evident(self):
        self.assertEqual(self.manifest["case_count"], 2080)
        self.assertEqual(self.manifest["control_count"], 32)
        self.assertEqual(self.manifest["generated_case_count"], 2048)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            self.assertEqual(corpus.verify_corpus(bundle), self.manifest)
            extra = bundle / "extra"
            extra.write_text("unexpected")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            target = bundle / "s2_42ac_cdqe_c00.case.json"
            target.write_bytes(target.read_bytes() + b" ")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)

    def test_complete_comparison_rejects_state_memory_outcome_and_exception_tampering(self):
        ident = "s2_42ac_cdqe_c00"
        raw = self.files[f"{ident}.case.json"]
        expected = self.expected[ident]
        valid = observation(raw, expected)
        corpus.compare_result(valid, raw, expected, BUILD, MACHINE, "valid")
        mutations = (
            lambda x: x.__setitem__("unexpected", "extra result field"),
            lambda x: x["snapshot"]["value"]["state"]["gpr"].__setitem__(0, corpus.hx(1)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(2)),
            lambda x: x["snapshot"]["value"].__setitem__("rflags_defined", corpus.hx(0)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rip", corpus.hx(0)),
            lambda x: x["snapshot"]["value"]["memory"][0].__setitem__("bytes_hex", "00"),
            lambda x: x["outcome"].__setitem__("kind", "exception"),
            lambda x: x["instructions_executed"].__setitem__("value", corpus.hx(0)),
            lambda x: x["snapshot"]["value"]["exception"].__setitem__("value", {"kind": "fault"}),
        )
        for mutate in mutations:
            changed = copy.deepcopy(valid)
            mutate(changed)
            with self.assertRaises(CorpusError):
                corpus.compare_result(changed, raw, expected, BUILD, MACHINE, "tampered")

    def test_native_is_denied_without_invoking_a_backend(self):
        with tempfile.TemporaryDirectory() as temp, mock.patch.object(corpus.subprocess, "run", side_effect=AssertionError("backend invoked")):
            path = Path(temp) / "native.json"
            corpus.write_native_blocked(path)
            saved = json.loads(path.read_bytes())
            self.assertEqual(saved["native_x86"], "not_run_form_not_admitted")
            self.assertEqual(saved["backend_execution"], "not_started")

    def test_interrupt_checkpoint_lists_unfinished_repeat(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            manifest = self.manifest
            ident = "s2_42ac_cdqe_c00"
            row = {"case_id": ident, "repeats": [{"ok": True}], "processes": [{"returncode": 0}]}
            path = root / "results.json"
            corpus.write_checkpoint(path, bundle, manifest, [row], ["interrupted"], {"x": "x"}, MACHINE, BUILD, MACHINE)
            saved = json.loads(path.read_bytes())
            self.assertEqual(saved["status"], "incomplete")
            self.assertIn({"case_id": ident, "repeat": 2}, saved["missing_attempts"])

    def test_repeat_stability_and_saved_result_tampering(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp); bundle = root / "corpus"; self.copy_bundle(bundle)
            runner = root / "od-runner"; runner.write_bytes(b"test runner")
            ident = "s2_42ac_cdqe_c00"; raw = self.files[f"{ident}.case.json"]
            obs = observation(raw, self.expected[ident]); fixture_path = str((bundle / f"{ident}.case.json").resolve())
            proc = {"returncode": 0, "stdout": json.dumps(obs), "stderr": "",
                    "command": [str(runner.resolve()), "--fixture", fixture_path, "--engine", "interpreter", "--json"],
                    "cwd": str(corpus.ROOT.resolve()), "environment": corpus.process_environment(), "timeout_diagnosis": None}
            manifest = {"case_digests": [{"case_id": ident}]}
            identity = {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]}
            varying = copy.deepcopy(obs); varying["unexpected"] = "different"
            runs = iter((obs, varying))
            def fake_run(*args, **kwargs):
                return corpus.subprocess.CompletedProcess(args[0], 0, stdout=json.dumps(next(runs)), stderr="")
            output = root / "unstable.json"
            with mock.patch.object(corpus, "verify_corpus", return_value=manifest), \
                    mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)), \
                    mock.patch.object(corpus, "compare_result"), \
                    mock.patch.object(corpus.subprocess, "run", side_effect=fake_run):
                with self.assertRaises(CorpusError):
                    corpus.run_interpreter(bundle, output, runner)
            failed = json.loads(output.read_bytes())
            self.assertEqual(failed["status"], "failed")
            self.assertTrue(any("repeat results differ" in x for x in failed["failures"]))

            rows = []
            for entry in self.manifest["case_digests"]:
                case_id = entry["case_id"]
                case_raw = self.files[f"{case_id}.case.json"]
                case_obs = observation(case_raw, self.expected[case_id])
                case_path = str((bundle / f"{case_id}.case.json").resolve())
                case_proc = {"returncode": 0, "stdout": json.dumps(case_obs), "stderr": "",
                    "command": [str(runner.resolve()), "--fixture", case_path, "--engine", "interpreter", "--json"],
                    "cwd": str(corpus.ROOT.resolve()), "environment": corpus.process_environment(), "timeout_diagnosis": None}
                rows.append({"case_id": case_id, "fixture_sha256": sha256(case_raw),
                    "repeats": [case_obs, copy.deepcopy(case_obs)], "processes": [case_proc, copy.deepcopy(case_proc)]})
            saved = {"schema_version": 1, "status": "passed", "backend": "interpreter",
                "native_x86": "not_run_form_not_admitted", "profile": corpus.PROFILE,
                "manifest_sha256": sha256(self.files["manifest.json"]), "case_count": corpus.TOTAL_COUNT, "repeats_per_case": 2,
                "execution_identity": identity, "execution_machine": MACHINE, "runner_build": BUILD,
                "runner_machine": MACHINE, "results": rows, "failures": [], "missing_attempts": []}
            output.write_bytes(json_bytes(saved))
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)):
                corpus.verify_results(bundle, output, runner)
            changed = copy.deepcopy(saved)
            changed["results"][0]["repeats"][0]["snapshot"]["value"]["state"]["gpr"][0] = corpus.hx(3)
            output.write_bytes(json_bytes(changed))
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)):
                with self.assertRaises(CorpusError):
                    corpus.verify_results(bundle, output, runner)
            forged = copy.deepcopy(saved)
            forged_revision = "f" * 40
            forged["execution_identity"]["revision"] = forged_revision
            forged["runner_build"]["revision"] = forged_revision
            for row in forged["results"]:
                for repeat, process in zip(row["repeats"], row["processes"]):
                    repeat["build"]["revision"] = forged_revision
                    process["stdout"] = json.dumps(repeat)
            output.write_bytes(json_bytes(forged))
            with mock.patch.object(corpus, "command_identity", return_value=(identity, MACHINE)):
                with self.assertRaises(CorpusError):
                    corpus.verify_results(bundle, output, runner)


if __name__ == "__main__":
    unittest.main()
