#!/usr/bin/env python3
"""Regression checks for orchestration; these tests do not claim physical execution."""

import contextlib
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import stage1_harness as harness
import stage1_oracle as oracle


class OracleInputTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.addCleanup(patch.stopall)
        patch.object(oracle, "ROOT", self.root).start()
        self.output = self.root / "output"
        self.output.mkdir()
        self.raw = (harness.ROOT / oracle.DEFAULT_CASES[0]).read_bytes()

    def case(self, directory="input", name="one.case.json", case_id=None):
        path = self.root / directory / name
        path.parent.mkdir(exist_ok=True)
        value = json.loads(self.raw)
        if case_id is not None:
            value["case_id"] = case_id
        path.write_text(json.dumps(value))
        return path

    def test_freezes_exact_input_bytes_before_execution(self):
        path = self.case()
        original = path.read_bytes()
        cases = oracle.prepare_cases([str(path)], self.output)
        path.write_text("changed after preparation")
        self.assertEqual(cases[0][0].read_bytes(), original)
        self.assertEqual(cases[0][2], oracle.hashlib.sha256(original).hexdigest())

    def test_rejects_path_traversal_case_id_before_creating_result_directory(self):
        path = self.case(case_id="../../escaped")
        with self.assertRaisesRegex(oracle.Stage1Error, "invalid fixture case_id"):
            oracle.prepare_cases([str(path)], self.output)
        self.assertFalse((self.root / "escaped").exists())

    def test_rejects_duplicate_remote_names_and_case_ids(self):
        for duplicate_name in (True, False):
            with self.subTest(duplicate_name=duplicate_name):
                output = self.output / str(duplicate_name)
                output.mkdir()
                first = self.case("first", "one.case.json", "case-one")
                second = self.case("second", "one.case.json" if duplicate_name else "two.case.json",
                                   "case-two" if duplicate_name else "case-one")
                with self.assertRaisesRegex(oracle.Stage1Error, "duplicate fixture"):
                    oracle.prepare_cases([str(first), str(second)], output)

    def test_malformed_program_has_a_reportable_harness_error(self):
        path = self.case()
        value = json.loads(self.raw)
        value["program"] = []
        path.write_text(json.dumps(value))
        with self.assertRaisesRegex(oracle.Stage1Error, "fixture program"):
            oracle.prepare_cases([str(path)], self.output)

    def test_native_failure_preserves_all_process_output(self):
        paths = [self.case(name="one.case.json", case_id="one"),
                 self.case(name="two.case.json", case_id="two")]
        cases = oracle.prepare_cases([str(path) for path in paths], self.output)
        records = {
            "one.case.json": {"returncode": 5, "stdout": '{"status":"timed_out"}', "stderr": "watchdog"},
            "two.case.json": {"returncode": 0, "stdout": '{"status":"completed"}', "stderr": ""},
        }
        process = subprocess.CompletedProcess([], 0, json.dumps(records), "ssh diagnostic")
        with patch.object(oracle, "run", return_value=process):
            with self.assertRaisesRegex(oracle.Stage1Error, "one.case.json: exit 5"):
                oracle.run_remote_batch("host", "/repo", "runner", "fixtures", cases, self.output)
        for name, record in records.items():
            case_id = name.removesuffix(".case.json")
            self.assertEqual((self.output / case_id / "native_x86.json").read_text(), record["stdout"])
            self.assertEqual((self.output / case_id / "native_x86.stderr.log").read_text(), record["stderr"])
        self.assertEqual(json.loads((self.output / "native-batch.stdout.json").read_text()), records)


class HarnessSelectionTests(unittest.TestCase):
    def test_generated_corpus_keeps_manual_boundary_cases(self):
        commands = []
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def step(name, command, run_dir, timeout):
                commands.append(command)
                return {"status": "passed", "output_tail": ""}

            with patch.object(harness, "ROOT", root), patch.object(harness, "run_step", side_effect=step), \
                 patch("sys.argv", ["stage1_harness.py", "--suite", "oracle", "--generated-count", "3"]), \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(harness.main(), 0)
        oracle_command = commands[-1]
        for fixture in oracle.DEFAULT_CASES:
            self.assertIn(fixture, oracle_command)
        self.assertTrue(oracle_command[-1].startswith("work/stage1-harness-generated-"))


if __name__ == "__main__":
    unittest.main()
