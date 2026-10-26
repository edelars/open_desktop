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
    def test_remote_test_routing_keeps_legacy_suite_and_skips_known_isa_failures_for_block(self):
        legacy = oracle.remote_test_command("/remote/repo", "jit")
        block = oracle.remote_test_command("/remote/repo", "jit-block")
        self.assertIn("cargo test --offline -p od-isa -p od-testkit -p od-runner -p od-x86-runner", legacy)
        self.assertIn("cargo test --offline -p od-testkit -p od-runner -p od-x86-runner", block)
        self.assertNotIn("-p od-isa ", block)
        self.assertIn("cd /remote/repo", block)

    def test_remote_preflight_persists_full_process_output_and_exit_code(self):
        with tempfile.TemporaryDirectory() as directory:
            output_dir = Path(directory)
            output_dir.mkdir(exist_ok=True)

            def failed_process(argv, *, timeout):
                return subprocess.CompletedProcess(argv, 101, "stdout: full\n", "stderr: full\n")

            record = oracle.run_remote_test_preflight(
                "host", "cargo test", output_dir, process_runner=failed_process
            )
            self.assertEqual(record["exit_code"], 101)
            self.assertEqual((output_dir / record["stdout_path"]).read_text(), "stdout: full\n")
            self.assertEqual((output_dir / record["stderr_path"]).read_text(), "stderr: full\n")
            saved = json.loads((output_dir / "remote-test-preflight.json").read_text())
            self.assertEqual(saved, record)

    def test_remote_preflight_timeout_persists_partial_output_and_timeout_record(self):
        with tempfile.TemporaryDirectory() as directory:
            output_dir = Path(directory)

            def timeout_process(argv, *, timeout):
                raise subprocess.TimeoutExpired(
                    argv, timeout, output=b"partial stdout\n", stderr=b"partial stderr\n"
                )

            record = oracle.run_remote_test_preflight(
                "host", "cargo test", output_dir, process_runner=timeout_process
            )
            self.assertIsNone(record["exit_code"])
            self.assertTrue(record["timed_out"])
            self.assertEqual(record["timeout_seconds"], 1800.0)
            self.assertEqual((output_dir / record["stdout_path"]).read_text(), "partial stdout\n")
            self.assertEqual((output_dir / record["stderr_path"]).read_text(), "partial stderr\n")
            self.assertEqual(
                json.loads((output_dir / "remote-test-preflight.json").read_text()), record
            )

    def test_s3_14_manifest_selects_eight_historical_and_five_boundary_cases(self):
        path = oracle.ROOT / oracle.JIT_BLOCK_MANIFEST
        manifest = json.loads(path.read_text())
        self.assertEqual(manifest["stage"], "S3-14")
        self.assertEqual(manifest["suite"], "register-only one-block triple comparison")
        self.assertEqual(manifest["engine"], "jit-block")
        self.assertEqual(sum(case["kind"] == "historical" for case in manifest["cases"]), 8)
        self.assertEqual(sum(case["kind"] == "boundary" for case in manifest["cases"]), 5)
        self.assertEqual(len(manifest["excluded"]), 2)
        self.assertTrue(all(item["reason"] for item in manifest["excluded"]))
        self.assertEqual(len(manifest["local_only"]), 1)
        self.assertIn("budget", manifest["local_only"][0]["one_block_reason"])
        for case in manifest["cases"]:
            raw = (oracle.ROOT / case["path"]).read_bytes()
            fixture = json.loads(raw)
            self.assertEqual(fixture["expected_exit"], "reached_code_end", case["path"])
            self.assertEqual(oracle.hashlib.sha256(raw).hexdigest(), case["fixture_sha256"])
            self.assertEqual(fixture["program"]["sha256"], case["program_sha256"])
        local_fixture = json.loads((oracle.ROOT / manifest["local_only"][0]["path"]).read_text())
        self.assertEqual(local_fixture["expected_exit"], "instruction_budget_exhausted")
        self.assertEqual(
            {Path(item["path"]).name for item in manifest["excluded"]},
            {"s0_09_sub_jnz_loop.case.json", "s1_06_jz_not_taken.case.json"},
        )
        self.assertEqual(oracle.load_case_manifest(oracle.JIT_BLOCK_MANIFEST), [
            case["path"] for case in manifest["cases"]
        ])
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            cases = oracle.prepare_cases(oracle.load_case_manifest(oracle.JIT_BLOCK_MANIFEST), output)
        self.assertEqual(len(cases), 13)

    def test_jit_block_requires_explicit_manifest_or_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(oracle, "ROOT", Path(directory)), \
                 patch("sys.argv", ["stage1_oracle.py", "--engine", "jit-block"]), \
                 contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    oracle.main()
        self.assertEqual(error.exception.code, 2)

    def test_manifest_rejects_local_only_or_unknown_selected_case_kind(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            manifest_path = root / "manifest.json"
            for kind in ("local_only", "future_kind"):
                manifest_path.write_text(json.dumps({
                    "schema_version": 1,
                    "engine": "jit-block",
                    "cases": [{"kind": kind}],
                    "excluded": [],
                }))
                with self.subTest(kind=kind), patch.object(oracle, "ROOT", root):
                    with self.assertRaisesRegex(oracle.Stage1Error, "historical or boundary"):
                        oracle.load_case_manifest(manifest_path)

    def _run_oracle_main(self, *, engine="jit-block", selection="manifest", result_backend=None,
                         preflight_exit_code=0, preflight_timeout=False):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name).resolve()
        fixture_rel = "tests/fixtures/x86/s0_09_mov_imm64.case.json"
        fixture_path = root / fixture_rel
        fixture_path.parent.mkdir(parents=True)
        raw_fixture = (oracle.ROOT / fixture_rel).read_bytes()
        fixture_path.write_bytes(raw_fixture)
        manifest_rel = "tests/cpu/manifest.json"
        manifest_path = root / manifest_rel
        manifest_path.parent.mkdir(parents=True)
        fixture = json.loads(raw_fixture)
        manifest_path.write_text(json.dumps({
            "schema_version": 1,
            "stage": "S3-14",
            "suite": "test one-block selection",
            "engine": "jit-block",
            "cases": [{
                "path": fixture_rel,
                "fixture_sha256": oracle.hashlib.sha256(raw_fixture).hexdigest(),
                "program_sha256": fixture["program"]["sha256"],
                "required": True,
                "kind": "historical",
                "category": "test_mov",
                "terminal_boundary": "code_end",
                "one_block_reason": "One MOV reaches the code end.",
            }],
            "excluded": [],
        }))

        engines = []
        native_batches = []
        comparisons = []
        fingerprint = "a" * 64
        machine = {
            "host_os": "test-os", "host_arch": "test-arch", "cpu_model": None,
            "kernel": "test-kernel", "page_size": 4096,
            "rustc": "rustc test", "cargo": "cargo test",
        }

        def result_value(fixture, input_digest, backend):
            return {
                "schema_version": 1,
                "case_id": fixture["case_id"],
                "input_sha256": input_digest,
                "profile": fixture["profile"],
                "backend": backend,
                "status": "completed",
                "expected_exit": fixture["expected_exit"],
                "build": {
                    "revision": f"sha256:{fingerprint}",
                    "target": "test-target",
                    "profile": "debug",
                    "compiler": "rustc test",
                },
                "machine": {"host_os": "test-os", "host_arch": "test-arch"},
                "snapshot": {
                    field: {"status": "available", "value": {}}
                    for field in ("gpr", "rip", "rflags")
                },
            }

        def fake_run(argv, *, cwd=oracle.ROOT, env=None, timeout=60.0):
            if argv[0] == "ssh":
                return subprocess.CompletedProcess(
                    argv, preflight_exit_code, "preflight stdout: all lines\n",
                    "preflight stderr: all lines\n" if preflight_exit_code else "",
                )
            if argv[0] in {"cargo", "scp"}:
                return subprocess.CompletedProcess(argv, 0, "", "")
            engine = argv[argv.index("--engine") + 1]
            engines.append(engine)
            selected_path = Path(argv[argv.index("--fixture") + 1])
            fixture = json.loads(selected_path.read_text())
            input_digest = oracle.hashlib.sha256(selected_path.read_bytes()).hexdigest()
            if engine == "interpreter":
                backend = "interpreter"
            else:
                backend = result_backend or oracle.expected_backend(engine)
            return subprocess.CompletedProcess(
                argv, 0, json.dumps(result_value(fixture, input_digest, backend)), ""
            )

        def fake_ssh_run(target, command, *, timeout=60.0):
            if "python3 -c" in command:
                return fingerprint + "\n"
            if "rustc --version" in command:
                return "rustc remote\n"
            if "cargo --version" in command:
                return "cargo remote\n"
            return "test result: ok\n"

        def fake_preflight_process(argv, *, timeout):
            if preflight_timeout:
                raise subprocess.TimeoutExpired(
                    argv, timeout, output="preflight partial stdout\n",
                    stderr="preflight partial stderr\n",
                )
            return subprocess.CompletedProcess(
                argv, preflight_exit_code, "preflight stdout: all lines\n",
                "preflight stderr: all lines\n" if preflight_exit_code else "",
            )

        def fake_native_batch(target, remote_root, runner, fixture_dir, cases, output_dir, *, process_runner=None):
            native_batches.append(output_dir.name)
            for _, fixture, _ in cases:
                (output_dir / fixture["case_id"]).mkdir()
            return {
                path.name: result_value(fixture, digest, "native_x86")
                for path, fixture, digest in cases
            }

        def fake_compare(runner, left, right, fixture_path, output_path, env, *, process_runner=None):
            fixture = json.loads(fixture_path.read_text())
            comparisons.append((fixture["case_id"], left.name, right.name))
            return {"case_id": fixture["case_id"], "differences": []}

        argv = ["stage1_oracle.py"]
        if selection == "manifest":
            argv.extend(["--engine", engine, "--manifest", manifest_rel])
        elif selection == "cases":
            argv.extend([fixture_rel, "--engine", engine])
        elif selection == "default":
            argv.append(fixture_rel)
        else:
            raise AssertionError(selection)
        argv.extend(["--repeats", "2", "--output-root", "work/test-oracle"])
        output = io.StringIO()
        with patch.object(oracle, "ROOT", root), \
             patch("sys.argv", argv), \
             patch.object(oracle, "source_fingerprint", return_value=fingerprint), \
             patch.object(oracle, "local_machine", return_value=machine), \
             patch.object(oracle, "ssh_run", side_effect=fake_ssh_run), \
             patch.object(oracle, "run", side_effect=fake_run), \
             patch.object(oracle, "run_preflight_process", side_effect=fake_preflight_process), \
             patch.object(oracle, "run_remote_batch", side_effect=fake_native_batch), \
             patch.object(oracle, "compare", side_effect=fake_compare), \
             contextlib.redirect_stdout(output), contextlib.redirect_stderr(io.StringIO()):
            status = oracle.main()
        manifest_paths = list((root / "work/test-oracle").glob("*/manifest.json"))
        saved = json.loads(manifest_paths[0].read_text())
        if "remote_test_preflight" in saved:
            preflight = saved["remote_test_preflight"]
            saved["_preflight_stdout"] = (manifest_paths[0].parent / preflight["stdout_path"]).read_text()
            saved["_preflight_stderr"] = (manifest_paths[0].parent / preflight["stderr_path"]).read_text()
        return status, engines, native_batches, comparisons, saved

    def test_jit_block_orchestration_checks_backend_and_repeats_all_three_engines(self):
        status, engines, native_batches, comparisons, saved = self._run_oracle_main()
        self.assertEqual(status, 0, saved)
        self.assertEqual(engines, ["interpreter", "jit-block"] * 2)
        self.assertEqual(len(native_batches), 2)
        self.assertEqual(len(comparisons), 9)  # cross-backend, native, and all three repeat checks
        self.assertEqual(saved["status"], "passed")
        self.assertEqual(saved["stage"], "S3-14")
        self.assertEqual(saved["suite"], "register-only one-block triple comparison")
        self.assertEqual(saved["engine"], "jit-block")
        self.assertEqual(saved["jit_backend"], "jit_ir_block")
        self.assertEqual(saved["remote_test_preflight"]["exit_code"], 0)
        self.assertEqual(saved["_preflight_stdout"], "preflight stdout: all lines\n")
        self.assertEqual(saved["repeats"], 2)
        entry = saved["fixtures"][1]
        self.assertIn("local_jit_block", entry["build"])
        self.assertEqual(entry["differences"]["interpreter_vs_jit_block"], 0)
        self.assertEqual(entry["differences"]["jit_block_vs_native"], 0)
        self.assertEqual(entry["repeat_differences"], {
            "interpreter": 0, "jit_block": 0, "native_x86": 0,
        })

    def test_jit_block_orchestration_rejects_legacy_backend_tag_and_records_failure(self):
        status, _, _, _, saved = self._run_oracle_main(result_backend="jit")
        self.assertEqual(status, 1)
        self.assertEqual(saved["status"], "failed")
        self.assertIn("expected 'jit_ir_block'", saved["error"])

    def test_legacy_run_manifest_does_not_claim_s3_14_one_block_suite(self):
        status, _, _, _, saved = self._run_oracle_main(engine="jit", selection="default")
        self.assertEqual(status, 0)
        self.assertEqual(saved["stage"], "Stage1")
        self.assertEqual(saved["suite"], "legacy fixture oracle")
        self.assertNotIn("one-block", saved["suite"])

    def test_remote_preflight_failure_is_logged_and_aborts_before_build_or_execution(self):
        status, engines, native_batches, _, saved = self._run_oracle_main(preflight_exit_code=101)
        self.assertEqual(status, 1)
        self.assertEqual(saved["status"], "failed")
        self.assertEqual(saved["remote_test_preflight"]["exit_code"], 101)
        self.assertEqual(saved["_preflight_stdout"], "preflight stdout: all lines\n")
        self.assertEqual(saved["_preflight_stderr"], "preflight stderr: all lines\n")
        self.assertEqual(engines, [])
        self.assertEqual(native_batches, [])
        self.assertIn("full stdout/stderr are saved", saved["error"])

    def test_remote_preflight_timeout_is_logged_before_aborting(self):
        status, engines, native_batches, _, saved = self._run_oracle_main(preflight_timeout=True)
        self.assertEqual(status, 1)
        self.assertEqual(saved["status"], "failed")
        self.assertIsNone(saved["remote_test_preflight"]["exit_code"])
        self.assertTrue(saved["remote_test_preflight"]["timed_out"])
        self.assertEqual(saved["_preflight_stdout"], "preflight partial stdout\n")
        self.assertEqual(saved["_preflight_stderr"], "preflight partial stderr\n")
        self.assertEqual(engines, [])
        self.assertEqual(native_batches, [])
        self.assertIn("timed out after 1800.0 seconds", saved["error"])

    def test_explicit_jit_block_cases_do_not_claim_curated_manifest_suite(self):
        status, _, _, _, saved = self._run_oracle_main(engine="jit-block", selection="cases")
        self.assertEqual(status, 0)
        self.assertEqual(saved["stage"], "S3-14")
        self.assertEqual(saved["suite"], "explicit jit-block case comparison")
        self.assertNotIn("one-block triple", saved["suite"])

    def test_explicit_jit_block_case_selection_and_legacy_default_engine(self):
        block_status, block_engines, _, _, block_saved = self._run_oracle_main(selection="cases")
        self.assertEqual(block_status, 0)
        self.assertEqual(block_engines, ["interpreter", "jit-block"] * 2)
        self.assertEqual(block_saved["engine"], "jit-block")

        jit_status, jit_engines, _, _, jit_saved = self._run_oracle_main(engine="jit", selection="default")
        self.assertEqual(jit_status, 0)
        self.assertEqual(jit_engines, ["interpreter", "jit"] * 2)
        self.assertEqual(jit_saved["engine"], "jit")
        self.assertIn("local_jit", jit_saved["fixtures"][0]["build"])

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
