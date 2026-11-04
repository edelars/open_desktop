"""Pinned controls, independent rel32 oracle, replay, tamper rejection and native denial for S2-42z."""
from __future__ import annotations

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

SCRIPT = Path(__file__).with_name("generate_stage2_jcc_rel32_corpus.py")
SPEC = importlib.util.spec_from_file_location("s2_42z", SCRIPT)
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
                "exception": {"status": "available", "value": None},
                "addss_memory_rax_controls": None, "minss_memory_rax_controls": None}}}


class JccRel32CorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = corpus.materialize()
        cls.manifest = json.loads(cls.files["manifest.json"])
        cls.expected = {x["case_id"]: x for x in json.loads(cls.files["expected.json"])["cases"]}

    def copy_bundle(self, target: Path):
        target.mkdir()
        for name, contents in self.files.items():
            (target / name).write_bytes(contents)

    def test_pinned_controls_cover_opcode_zf_and_signed_boundaries(self):
        controls = corpus.control_inputs()
        self.assertEqual(len(controls), 32)
        self.assertEqual(len(corpus.load_controls()[0]), 32)
        seen = set()
        for raw in controls.values():
            item = json.loads(raw)
            code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
            self.assertEqual(len(code), 6)
            self.assertEqual(code[:2], bytes((0x0f, code[1])))
            self.assertIn(code[1], corpus.OPCODES)
            unsigned = int.from_bytes(code[2:], "little")
            displacement = unsigned - (1 << 32) if unsigned & (1 << 31) else unsigned
            zf = (int(item["initial_state"]["rflags"], 16) >> 6) & 1
            seen.add((code[1], zf, displacement))
            expected = corpus.expected_snapshot(item)
            taken = (code[1] == 0x84 and zf == 1) or (code[1] == 0x85 and zf == 0)
            self.assertEqual(expected["state"]["rip"], item["stop_rip"])
            self.assertEqual(expected["state"]["rip"], corpus.hx(corpus.RIP + 6 + displacement if taken else corpus.RIP + 6))
            self.assertEqual(expected["state"]["gpr"], item["initial_state"]["gpr"])
            self.assertEqual(expected["state"]["rflags"], item["initial_state"]["rflags"])
            self.assertLess(int(expected["state"]["rip"], 16), 1 << 47)
        self.assertEqual(seen, {(op, zf, disp) for op in corpus.OPCODES for zf in (0, 1) for disp in corpus.DISPLACEMENTS})
        self.assertIn(-(1 << 31), corpus.DISPLACEMENTS)
        self.assertIn((1 << 31) - 1, corpus.DISPLACEMENTS)
        self.assertIn(-129, corpus.DISPLACEMENTS)
        self.assertIn(128, corpus.DISPLACEMENTS)
        self.assertNotIn(-6, corpus.DISPLACEMENTS)

    def test_generated_inputs_are_unique_deterministic_and_matrix_balanced(self):
        first, second = corpus.generated_fixtures(), corpus.generated_fixtures()
        self.assertEqual(first, second)
        self.assertEqual(len(first), 1024)
        unique = {(x["memory"][0]["initial_bytes_hex"], tuple(x["initial_state"]["gpr"]),
                   x["initial_state"]["rip"], x["initial_state"]["rflags"]) for x in first}
        self.assertEqual(len(unique), 1024)
        counts = Counter()
        by_seed = {seed: Counter() for seed in corpus.SEEDS}
        columns = [set() for _ in range(16)]
        flags_seen = {bit: set() for bit in (0, 2, 4, 6, 7, 10, 11)}
        for item in first:
            code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
            unsigned = int.from_bytes(code[2:], "little")
            displacement = unsigned - (1 << 32) if unsigned & (1 << 31) else unsigned
            zf = (int(item["initial_state"]["rflags"], 16) >> 6) & 1
            cell = (code[1], zf, displacement)
            counts[cell] += 1
            by_seed[int(item["seed"], 16)][cell] += 1
            self.assertEqual(len(item["initial_state"]["gpr"]), 16)
            gprs = item["initial_state"]["gpr"]
            for index, value in enumerate(gprs):
                columns[index].add(value)
            rflags = int(item["initial_state"]["rflags"], 16)
            self.assertEqual(rflags & ~corpus.RFLAGS_DEFINED, 0)
            for bit in flags_seen:
                flags_seen[bit].add(bool(rflags & (1 << bit)))
        self.assertEqual(set(counts.values()), {32})
        self.assertEqual(set(counts), {(op, zf, disp) for op in corpus.OPCODES for zf in (0, 1) for disp in corpus.DISPLACEMENTS})
        for seed_counts in by_seed.values():
            self.assertEqual(set(seed_counts.values()), {8})
            self.assertEqual(set(seed_counts), set(counts))
        self.assertTrue(all(len(column) > 1 for column in columns))
        self.assertTrue(all(values == {False, True} for values in flags_seen.values()))

    def test_manifest_and_strict_corpus_reject_tampering(self):
        self.assertEqual(self.manifest["case_count"], 1056)
        self.assertEqual(self.manifest["generated_case_count"], 1024)
        self.assertEqual(self.manifest["control_count"], 32)
        self.assertEqual(self.manifest["code_address"], corpus.hx(corpus.CODE_ADDRESS))
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            self.assertEqual(corpus.verify_corpus(bundle), self.manifest)
            (bundle / "unexpected").write_text("x")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)
        with tempfile.TemporaryDirectory() as temp:
            bundle = Path(temp) / "corpus"
            self.copy_bundle(bundle)
            target = bundle / f"{corpus.control_ids()[0]}.case.json"
            target.write_bytes(target.read_bytes() + b" ")
            with self.assertRaises(CorpusError):
                corpus.verify_corpus(bundle)

    def test_complete_snapshot_comparison_rejects_state_and_code_mutations(self):
        ident = corpus.control_ids()[0]
        raw = corpus.control_inputs()[f"{ident}.case.json"]
        expected = self.expected[ident]
        valid = observation(raw, expected)
        corpus.compare_result(valid, raw, expected, BUILD, MACHINE, "valid")
        mutations = (
            lambda x: x.__setitem__("unexpected", True),
            lambda x: x["snapshot"].__setitem__("unexpected", None),
            lambda x: x["snapshot"]["value"].__setitem__("minss_memory_rax_controls", {"case_id": "unrelated"}),
            lambda x: x["snapshot"]["value"]["state"]["gpr"].__setitem__(0, corpus.hx(77)),
            lambda x: x["snapshot"]["value"]["state"]["gpr"].__setitem__(15, corpus.hx(77)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rflags", corpus.hx(2)),
            lambda x: x["snapshot"]["value"].__setitem__("rflags_defined", corpus.hx(0)),
            lambda x: x["snapshot"]["value"]["state"].__setitem__("rip", corpus.hx(3)),
            lambda x: x["snapshot"]["value"]["memory"][0].__setitem__("bytes_hex", "00"),
            lambda x: x["outcome"].__setitem__("kind", "exception"),
            lambda x: x["instructions_executed"].__setitem__("value", corpus.hx(0)),
            lambda x: x["snapshot"]["value"]["exception"].__setitem__("value", {"kind": "fault"}),
        )
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                changed = copy.deepcopy(valid)
                mutate(changed)
                with self.assertRaises(CorpusError):
                    corpus.compare_result(changed, raw, expected, BUILD, MACHINE, "mutated")

    def test_saved_replay_binds_both_observations_processes_and_provenance(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle = root / "corpus"
            self.copy_bundle(bundle)
            runner = root / "od-runner"
            runner.write_bytes(b"test runner identity")
            selected = runner.resolve()
            env = corpus.process_environment()
            rows = []
            for item in self.manifest["case_digests"]:
                ident = item["case_id"]
                raw = self.files[f"{ident}.case.json"]
                seen = observation(raw, self.expected[ident])
                stdout = json.dumps(seen, indent=2) + "\n"
                fixture_path = str((bundle / f"{ident}.case.json").resolve())
                proc = {"returncode": 0, "stdout": stdout, "stderr": "",
                        "command": [str(selected), "--fixture", fixture_path, "--engine", "interpreter", "--json"],
                        "cwd": str(corpus.ROOT.resolve()), "environment": env, "timeout_diagnosis": None}
                rows.append({"case_id": ident, "fixture_sha256": sha256(raw),
                             "repeats": [seen, copy.deepcopy(seen)], "processes": [proc, copy.deepcopy(proc)]})
            result = {"schema_version": 1, "status": "passed", "backend": "interpreter",
                "native_x86": "not_run_form_not_admitted", "profile": corpus.PROFILE,
                "manifest_sha256": sha256(self.files["manifest.json"]), "case_count": corpus.TOTAL_COUNT,
                "repeats_per_case": 2, "execution_identity": {"executable_sha256": sha256(runner.read_bytes()), "revision": BUILD["revision"]},
                "execution_machine": {"host_os": MACHINE["host_os"], "host_arch": MACHINE["host_arch"]},
                "runner_build": BUILD, "runner_machine": MACHINE,
                "results": rows, "failures": [], "missing_attempts": []}
            path = root / "results.json"
            path.write_bytes(json_bytes(result))
            corpus.verify_results(bundle, path, runner)
            for mutate in (
                lambda x: x.__setitem__("unexpected", True),
                lambda x: x["results"][0]["repeats"][0]["snapshot"]["value"]["state"].__setitem__("rip", corpus.hx(5)),
                lambda x: x["results"][0]["repeats"][0]["snapshot"]["value"].__setitem__("minss_memory_rax_controls", {"case_id": "unrelated"}),
                lambda x: x["results"][0]["processes"][0].__setitem__("command", []),
                lambda x: x["results"][0]["processes"][0].__setitem__("returncode", False),
                lambda x: x.__setitem__("manifest_sha256", "0" * 64),
                lambda x: x["results"][0]["repeats"].__setitem__(1, {"failure": "missing"}),
            ):
                changed = copy.deepcopy(result)
                mutate(changed)
                path.write_bytes(json_bytes(changed))
                with self.assertRaises(CorpusError):
                    corpus.verify_results(bundle, path, runner)

    def test_native_request_is_denied_without_backend_start(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp) / "native.json"
            with mock.patch.object(corpus.subprocess, "run", side_effect=AssertionError("backend must not start")) as backend:
                self.assertEqual(corpus.main(["--native", "--results", str(output)]), 1)
            backend.assert_not_called()
            saved = json.loads(output.read_bytes())
            self.assertEqual(saved["status"], "blocked")
            self.assertEqual(saved["native_x86"], "not_run_form_not_admitted")
            self.assertEqual(saved["backend_execution"], "not_started")


if __name__ == "__main__":
    unittest.main()
