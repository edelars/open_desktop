"""Focused integrity and fail-closed tests for the S2-42m corpus generator."""

from __future__ import annotations

import copy
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
import sys
from unittest import mock


MODULE_PATH = Path(__file__).with_name("generate_stage2_cvtsi2sd_corpus.py")
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("s2_42m", MODULE_PATH)
assert SPEC and SPEC.loader
s2_42m = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(s2_42m)


class Cvtsi2sdCorpusTests(unittest.TestCase):
    def test_integer_oracle_exact_signed_boundaries(self) -> None:
        cases = {
            0: 0x0000000000000000,
            1: 0x3FF0000000000000,
            -1: 0xBFF0000000000000,
            1 << 24: 0x4170000000000000,
            (1 << 24) + 1: 0x4170000010000000,
            -(1 << 24) - 1: 0xC170000010000000,
            -(1 << 31): 0xC1E0000000000000,
            (1 << 31) - 1: 0x41DFFFFFFFC00000,
        }
        for value, expected in cases.items():
            with self.subTest(value=value):
                self.assertEqual(s2_42m.binary64_bits(value), expected)

    def test_generated_matrix_is_unique_complete_and_finite_control_mask(self) -> None:
        files = s2_42m.corpus_files()
        fixture_names = [name for name in files if name.endswith(".case.json")]
        self.assertEqual(len(fixture_names), 1033)
        self.assertEqual(len(set(fixture_names)), len(fixture_names))
        ids = [name.removesuffix(".case.json") for name in fixture_names]
        self.assertEqual(len(set(ids)), len(ids))
        seen_rc: set[int] = set()
        seen_osxmmexcpt: set[bool] = set()
        seen_sticky: set[int] = set()
        seen_masks: set[int] = set()
        seen_ftz: set[int] = set()
        handwritten: set[int] = set()
        for name in fixture_names:
            fixture = json.loads(files[name])
            mxcsr = int(fixture["initial_state"]["sse"]["mxcsr"], 16)
            self.assertEqual(mxcsr & ~s2_42m.MXCSR_MASK, 0)
            self.assertEqual(mxcsr & (1 << 6), 0)
            seen_rc.add((mxcsr >> 13) & 3)
            seen_sticky.add(mxcsr & 0x3f)
            seen_masks.add(mxcsr & 0x1f80)
            seen_ftz.add(mxcsr & 0x8000)
            seen_osxmmexcpt.add(fixture["cvtsi2sd_r32_controls"]["cr4_osxmmexcpt"])
            if fixture["provenance"]["kind"] == "handwritten_control":
                expected = json.loads(files[name.replace(".case.json", ".expected.json")])
                handwritten.add(int(expected["binary64_bits"], 16))
        self.assertEqual(seen_rc, {0, 1, 2, 3})
        self.assertEqual(seen_osxmmexcpt, {False, True})
        self.assertGreater(len(seen_sticky), 1)
        self.assertGreater(len(seen_masks), 1)
        self.assertEqual(seen_ftz, {0, 0x8000})
        self.assertGreaterEqual(len(handwritten), 8)

    def test_exact_file_set_and_tampering_are_rejected(self) -> None:
        files = s2_42m.corpus_files()
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp) / "corpus"
            s2_42m.write_file_set(directory, files)
            s2_42m.verify_corpus(directory)
            path = directory / "manifest.json"
            path.write_text(path.read_text().replace('"repeat_count": 2', '"repeat_count": 1'))
            with self.assertRaises(s2_42m.CorpusError):
                s2_42m.verify_corpus(directory)

    def test_state_comparator_checks_high_xmm_and_controls(self) -> None:
        files = s2_42m.corpus_files()
        case = "s2_42m_cvtsi2sd_hand_00"
        raw = files[f"{case}.case.json"]
        expected = json.loads(files[f"{case}.expected.json"])
        result = {
            "schema_version": 2, "case_id": case, "input_sha256": s2_42m.sha256(raw),
            "profile": s2_42m.PROFILE, "backend": "interpreter",
            "outcome": expected["outcome"],
            "instructions_executed": {"status": "available", "value": expected["instructions_executed"]},
            "snapshot": {"status": "available", "value": {
                "state": copy.deepcopy(expected["state"]), "rflags_defined": expected["rflags_defined"],
                "memory": expected["memory"], "cvtsi2sd_r32_controls": expected["cvtsi2sd_r32_controls"],
                "exception": {"status": "available", "value": None},
            }},
            "build": {"id": "build"}, "machine": {"id": "machine"},
        }
        s2_42m.compare_result(result, raw, expected, result["build"], result["machine"], case)
        altered = copy.deepcopy(result)
        altered["snapshot"]["value"]["state"]["sse"]["xmm"][0] = "00" * 8 + "ff" * 8
        with self.assertRaises(s2_42m.CorpusError):
            s2_42m.compare_result(altered, raw, expected, result["build"], result["machine"], case)

    def test_native_backend_is_explicitly_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            summary = Path(temp) / "native.json"
            with mock.patch.object(s2_42m.sys, "argv", ["generator", "--native", "--results", str(summary)]):
                self.assertEqual(s2_42m.main(), 1)
            record = json.loads(summary.read_text())
            self.assertEqual(record["status"], "failed")
            self.assertEqual(record["backend"], "native_x86")

    def test_saved_summary_rejects_bad_schema_and_incomplete_provenance(self) -> None:
        files = s2_42m.corpus_files()
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp) / "corpus"
            s2_42m.write_file_set(directory, files)
            result_path = Path(temp) / "results.json"
            record = {
                "schema_version": 999, "status": "passed", "backend": "interpreter",
                "native_x86": "not_run_profile_closed", "profile": s2_42m.PROFILE, "failures": [],
                "manifest_sha256": s2_42m.sha256((directory / "manifest.json").read_bytes()),
                "case_count": 1033, "repeats_per_case": 2, "results": [],
                "execution_identity": {"executable_sha256": "", "revision": ""},
                "execution_machine": {}, "runner_build": {}, "runner_machine": {},
            }
            result_path.write_bytes(s2_42m.json_bytes(record))
            with self.assertRaises(s2_42m.CorpusError):
                s2_42m.verify_saved(directory, result_path)
            record["schema_version"] = 1
            record["failures"] = ["suppressed failure"]
            result_path.write_bytes(s2_42m.json_bytes(record))
            with self.assertRaises(s2_42m.CorpusError):
                s2_42m.verify_saved(directory, result_path)
            record["failures"] = []
            result_path.write_bytes(s2_42m.json_bytes(record))
            with self.assertRaises(s2_42m.CorpusError):
                s2_42m.verify_saved(directory, result_path)


if __name__ == "__main__":
    unittest.main()
