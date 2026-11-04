from __future__ import annotations

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import contextlib

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests/cpu"))
SPEC = importlib.util.spec_from_file_location(
    "mov_memory_corpus", ROOT / "tests/cpu/generate_stage2_mov_memory_corpus.py")
assert SPEC is not None and SPEC.loader is not None
corpus = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(corpus)
import s2_42q_mov_controls as controls
import stage2_oracle


class MovMemoryHandControlTests(unittest.TestCase):
    def test_exact_ordered_matrix_static_bytes_and_architectural_boundaries(self) -> None:
        rows, fixtures, expected, manifest, _ = controls.verify_controls()
        self.assertEqual(len(rows), 56)
        self.assertEqual(tuple(row["case_id"] for row in rows), controls.expected_ids())
        self.assertEqual(len({row["case_id"] for row in rows}), 56)
        self.assertEqual(len(fixtures), len(expected))
        self.assertEqual(manifest["control_set_digest"], controls.CONTROL_SET_DIGEST)
        for row in rows:
            case_id, fixture, wanted = row["case_id"], fixtures[row["case_id"]], expected[row["case_id"]]
            self.assertEqual(hashlib.sha256((ROOT / row["fixture_path"]).read_bytes()).hexdigest(), row["fixture_sha256"])
            self.assertEqual(hashlib.sha256((ROOT / row["expected_path"]).read_bytes()).hexdigest(), row["expected_sha256"])
            self.assertEqual(fixture["memory"][0]["permissions"], {"read": False, "write": False, "execute": True})
            self.assertEqual(fixture["memory"][1]["permissions"], {"read": True, "write": True, "execute": False})
            self.assertEqual(fixture["memory"][1]["comparison_mask_hex"], "ff" * 128)
            self.assertEqual(wanted["instructions_executed"], {"status": "available", "value": "0x0000000000000001"})
            self.assertEqual(wanted["state"]["rflags"], "0x00000000000008d7")
            self.assertEqual(wanted["state"]["rflags_defined"], "0x0000000000000cd7")
            self.assertEqual(wanted["memory"][1]["bytes_hex"], fixture["memory"][1]["initial_bytes_hex"] if row["direction"] == "load" else wanted["memory"][1]["bytes_hex"])
            self.assertEqual(len(wanted["memory"][1]["bytes_hex"]), 256)
            if row["direction"] == "load":
                old = int(fixture["initial_state"]["gpr"][0], 16)
                target = int(row["effective_address"], 16) - int(fixture["memory"][1]["guest_address"], 16)
                size = row["width"] // 8
                value = int.from_bytes(bytes.fromhex(fixture["memory"][1]["initial_bytes_hex"])[target:target+size], "little")
                if row["width"] == 8:
                    expected_rax = (old & ~0xFF) | value
                elif row["width"] == 16:
                    expected_rax = (old & ~0xFFFF) | value
                else:
                    expected_rax = value
                self.assertEqual(int(wanted["state"]["gpr"][0], 16), expected_rax, case_id)
            else:
                self.assertEqual(wanted["state"]["gpr"], fixture["initial_state"]["gpr"], case_id)
                target = int(row["effective_address"], 16) - 0x200000
                size = row["width"] // 8
                initial_data = bytes.fromhex(fixture["memory"][1]["initial_bytes_hex"])
                final_data = bytes.fromhex(wanted["memory"][1]["bytes_hex"])
                self.assertEqual(final_data[:target], initial_data[:target], case_id)
                self.assertEqual(final_data[target+size:], initial_data[target+size:], case_id)
                self.assertEqual(final_data[target:target+size], (int(fixture["initial_state"]["gpr"][0], 16) & ((1 << row["width"]) - 1)).to_bytes(size, "little"), case_id)
        forms = {(row["width"], row["direction"], row["form"]): row for row in rows}
        self.assertEqual(bytes.fromhex(fixtures["s2_42q_mov_08_load_sib"]["memory"][0]["initial_bytes_hex"]), bytes.fromhex("8a448b11"))
        self.assertEqual(bytes.fromhex(fixtures["s2_42q_mov_08_load_address32"]["memory"][0]["initial_bytes_hex"]), bytes.fromhex("678a440b05"))
        self.assertEqual(bytes.fromhex(fixtures["s2_42q_mov_64_load_rex_b"]["memory"][0]["initial_bytes_hex"]), bytes.fromhex("498b00"))
        for direction, sign in (("load", 1), ("store", -1)):
            rip = forms[(64, direction, "rip_relative")]
            fixture = fixtures[rip["case_id"]]
            code = bytes.fromhex(fixture["memory"][0]["initial_bytes_hex"])
            disp = int.from_bytes(code[-4:], "little", signed=True)
            self.assertEqual((disp > 0) - (disp < 0), sign)
            self.assertEqual(int(fixture["stop_rip"], 16), int(fixture["initial_state"]["rip"], 16) + len(code))

    def _copy_bundle(self, root: Path) -> None:
        destination = root / controls.CONTROL_DIR
        shutil.copytree(ROOT / controls.CONTROL_DIR, destination)
        manifest_path = root / controls.MANIFEST_PATH
        manifest_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / controls.MANIFEST_PATH, manifest_path)

    def _verify_copy(self, root: Path) -> None:
        with patch.object(controls, "ROOT", root.resolve()):
            controls.verify_controls()

    def test_manifest_fixture_expected_symlink_extra_duplicate_and_pin_fail_closed(self) -> None:
        cases = ("missing", "fixture", "expected", "symlink", "extra", "duplicate", "traversal", "rehash")
        for corruption in cases:
            with self.subTest(corruption=corruption), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                self._copy_bundle(root)
                directory = root / controls.CONTROL_DIR
                manifest_path = root / controls.MANIFEST_PATH
                manifest = json.loads(manifest_path.read_bytes())
                first = manifest["controls"][0]
                if corruption == "missing":
                    (directory / Path(first["fixture_path"]).name).unlink()
                elif corruption == "fixture":
                    path = directory / Path(first["fixture_path"]).name
                    path.write_bytes(path.read_bytes() + b" ")
                elif corruption == "expected":
                    path = directory / Path(first["expected_path"]).name
                    path.write_bytes(path.read_bytes() + b" ")
                elif corruption == "symlink":
                    path = directory / Path(first["fixture_path"]).name
                    raw = path.read_bytes()
                    path.unlink()
                    source = directory / "symlink-source.json"
                    source.write_bytes(raw)
                    path.symlink_to(source.name)
                elif corruption == "extra":
                    (directory / "unexpected.case.json").write_text("{}")
                elif corruption == "duplicate":
                    manifest["controls"][1] = dict(manifest["controls"][0])
                    manifest_path.write_text(json.dumps(manifest))
                elif corruption == "traversal":
                    first["fixture_path"] = "../outside.case.json"
                    manifest_path.write_text(json.dumps(manifest))
                else:
                    path = directory / Path(first["fixture_path"]).name
                    path.write_bytes(path.read_bytes() + b" ")
                    first["fixture_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
                    manifest["control_set_digest"] = controls.canonical_control_digest(manifest["controls"])
                    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
                if corruption not in ("duplicate", "rehash"):
                    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
                with patch.object(controls, "ROOT", root.resolve()):
                    with self.assertRaises(controls.CorpusError):
                        controls.verify_controls()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self._copy_bundle(root)
            with patch.object(controls, "ROOT", root.resolve()), patch.object(controls, "CONTROL_SET_DIGEST", "0" * 64):
                with self.assertRaisesRegex(controls.CorpusError, "pinned control-set digest"):
                    controls.verify_controls()

    def test_mov_suite_orchestration_is_ordered_and_remote_calls_are_mocked(self) -> None:
        rows, fixture_map, expected_map, _, hand_sha = controls.verify_controls()
        selected_rows = rows[:2]
        selected_cases = []
        for index, row in enumerate(selected_rows):
            path = ROOT / row["fixture_path"]
            fixture = fixture_map[row["case_id"]]
            selected_cases.append((path, fixture, row["fixture_sha256"]))
        hand_by_id = {selected_rows[0]["case_id"]: selected_rows[0]}
        selected_expected = {row["case_id"]: expected_map[row["case_id"]] for row in selected_rows}
        fake_corpus = {"corpus_digest": "a" * 64, "case_digests": []}
        fake_hand = {"control_set_digest": controls.CONTROL_SET_DIGEST}
        fingerprint = "f" * 64
        interpreter_calls = []

        def prepare(_corpus, output):
            return selected_cases, selected_expected, hand_by_id, fake_corpus, "b" * 64, fake_hand, hand_sha

        def result_for(path, fixture, digest, backend):
            expected = selected_expected[fixture["case_id"]]
            build = {"revision": f"sha256:{fingerprint}", "compiler": "rustc test",
                     "target": "test-target", "profile": "debug"}
            machine = ({"host_os": "darwin", "host_arch": "arm64"} if backend == "interpreter"
                       else {"host_os": "linux", "host_arch": "x86_64"})
            return {"schema_version": 2, "case_id": fixture["case_id"], "input_sha256": digest,
                "profile": controls.PROFILE, "backend": backend, "build": build, "machine": machine,
                "outcome": expected["outcome"], "instructions_executed": expected["instructions_executed"],
                "snapshot": {"status": "available", "value": {"state": expected["state"],
                    "rflags_defined": expected["state"]["rflags_defined"], "memory": expected["memory"],
                    "exception": {"status": "available", "value": None}}}}

        def native_batch(_target, _remote_root, _runner, _fixtures, cases, repeat_dir, *, process_runner):
            results = {}
            for path, fixture, digest in cases:
                result = result_for(path, fixture, digest, "native_x86")
                case_dir = repeat_dir / fixture["case_id"]
                case_dir.mkdir()
                (case_dir / "native_x86.json").write_text(json.dumps(result))
                (case_dir / "native_x86.process.json").write_text('{"returncode":0}')
                results[path.name] = result
            return results

        def command(argv, **_kwargs):
            if "--fixture" in argv:
                path = Path(argv[argv.index("--fixture") + 1])
                fixture = json.loads(path.read_bytes())
                digest = hashlib.sha256(path.read_bytes()).hexdigest()
                interpreter_calls.append(fixture["case_id"])
                return subprocess.CompletedProcess(argv, 0,
                    json.dumps(result_for(path, fixture, digest, "interpreter")), "")
            return subprocess.CompletedProcess(argv, 0, "", "")

        def compare_results(_evidence, _runner, left, right, fixture_path, fixture, digest, report, env):
            return {"schema_version": 2, "case_id": fixture["case_id"], "input_sha256": digest,
                    "matches": True, "differences": []}

        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            relative_output = str(Path(temporary).relative_to(ROOT))
            with patch.object(stage2_oracle, "prepare_mov_memory_suite", side_effect=prepare), \
                 patch.object(stage2_oracle.Evidence, "ssh", side_effect=lambda _target, command, label, timeout=60:
                    fingerprint if "fingerprint" in label else "rustc test" if "rustc" in label else
                    "cargo test" if "cargo" in label else "Linux test"), \
                 patch.object(stage2_oracle.oracle, "source_fingerprint", return_value=fingerprint), \
                 patch.object(stage2_oracle.oracle, "local_machine", return_value={"rustc": "rustc test", "cargo": "cargo test"}), \
                 patch.object(stage2_oracle.oracle, "run", side_effect=command), \
                 patch.object(stage2_oracle.oracle, "run_remote_batch", side_effect=native_batch), \
                 patch.object(stage2_oracle, "compare", side_effect=compare_results), \
                 contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                status = stage2_oracle.main(["--suite", "mov-memory-corpus", "--host", "mock-host",
                    "--mov-memory-corpus", str(ROOT / "work/stage2/s2-42e-mov-memory"),
                    "--output-root", relative_output])
            manifests = list((ROOT / relative_output).glob("*/manifest.json"))
            self.assertEqual(len(manifests), 1)
            saved = json.loads(manifests[0].read_bytes())
            self.assertEqual(status, 0, saved)
            expected_order = [row["case_id"] for row in selected_rows]
            self.assertEqual(interpreter_calls, expected_order * 2)
            self.assertEqual([row["case_id"] for row in saved["inputs"]], expected_order)
            self.assertEqual([row["repeat"] for row in saved["fixtures"]], [1, 1, 2, 2])
            self.assertEqual(saved["case_counts"], {"generated_count": 1024, "handwritten_count": 56, "total_count": 1080})
            shutil.rmtree(ROOT / relative_output)

    def test_real_selected_set_is_controls_then_unchanged_generated_order(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            generated_dir = root / "generated"
            corpus.write_directory(generated_dir)
            output = root / "run"
            output.mkdir()
            cases, expected, controls_by_id, generated_manifest, generated_sha, hand_manifest, hand_sha = \
                stage2_oracle.prepare_mov_memory_suite(generated_dir, output)
            hand_rows = json.loads((ROOT / controls.MANIFEST_PATH).read_bytes())["controls"]
            generated_rows = json.loads((generated_dir / corpus.MANIFEST_NAME).read_bytes())["case_digests"]
            self.assertEqual(len(cases), 1080)
            self.assertEqual([fixture["case_id"] for _, fixture, _ in cases[:56]],
                             [row["case_id"] for row in hand_rows])
            self.assertEqual([fixture["case_id"] for _, fixture, _ in cases[56:]],
                             [row["case_id"] for row in generated_rows])
            self.assertEqual(len(expected), 1080)
            self.assertEqual(list(controls_by_id), [row["case_id"] for row in hand_rows])
            self.assertEqual(generated_manifest["corpus_digest"],
                             json.loads((generated_dir / corpus.MANIFEST_NAME).read_bytes())["corpus_digest"])
            self.assertEqual(generated_sha, hashlib.sha256((generated_dir / corpus.MANIFEST_NAME).read_bytes()).hexdigest())
            self.assertEqual(hand_manifest["control_set_digest"], controls.CONTROL_SET_DIGEST)
            self.assertEqual(hand_sha, hashlib.sha256((ROOT / controls.MANIFEST_PATH).read_bytes()).hexdigest())
            self.assertEqual([path.name for path, _, _ in cases[:56]],
                             [Path(row["fixture_path"]).name for row in hand_rows])

    def test_identical_wrong_backend_results_do_not_satisfy_independent_expected(self) -> None:
        _, fixtures, expected, _, _ = controls.verify_controls()
        case_id = controls.expected_ids()[0]
        wrong = dict(expected[case_id])
        wrong["state"] = dict(expected[case_id]["state"])
        wrong["state"]["gpr"] = list(wrong["state"]["gpr"])
        wrong["state"]["gpr"][0] = "0x0000000000000000"
        for backend in ("interpreter", "native_x86"):
            with self.assertRaisesRegex(stage2_oracle.Stage2Error, "gpr"):
                stage2_oracle.validate_mov_expected({"case_id": case_id, "outcome": {"kind": "stop_address"},
                    "instructions_executed": {"status": "available", "value": "0x0000000000000001"},
                    "snapshot": {"status": "available", "value": {"state": wrong["state"],
                    "rflags_defined": wrong["state"]["rflags_defined"], "memory": wrong["memory"],
                    "exception": {"status": "available", "value": None}}}}, expected[case_id], backend)

    def _write_valid_saved_run(self, root: Path) -> tuple[Path, Path]:
        generated_files = corpus.materialize()
        corpus_dir = root / "corpus"
        corpus_dir.mkdir()
        for name, raw in generated_files.items():
            (corpus_dir / name).write_bytes(raw)
        run = root / "run"
        run.mkdir()
        corpus_manifest = json.loads(generated_files[corpus.MANIFEST_NAME])
        generated_expected = json.loads(generated_files[corpus.EXPECTED_NAME])["cases"]
        hand_rows, hand_fixtures, hand_expected, hand_manifest, hand_sha = controls.verify_controls()
        frozen_generated = run / "fixtures"
        frozen_generated.mkdir()
        for name, raw in generated_files.items():
            if name.endswith(".case.json"):
                (frozen_generated / name).write_bytes(raw)
        frozen_hand = run / "hand-controls"
        frozen_hand.mkdir()
        for row in hand_rows:
            (frozen_hand / Path(row["fixture_path"]).name).write_bytes(
                (controls.ROOT / row["fixture_path"]).read_bytes())
        ordered_ids = [row["case_id"] for row in hand_rows]
        ordered_ids += [row["case_id"] for row in corpus_manifest["case_digests"]]
        digest_by_id = {row["case_id"]: row["sha256"] for row in corpus_manifest["case_digests"]}
        digest_by_id.update({row["case_id"]: row["fixture_sha256"] for row in hand_rows})
        expected_by_id = dict(hand_expected)
        expected_by_id.update({row["case_id"]: row for row in generated_expected})
        manifest = {
            "suite": "mov-memory-corpus", "status": "passed", "repeats": 2,
            "profile": controls.PROFILE, "commands": [{"returncode": 0}],
            "source_fingerprint": "a" * 64,
            "machines": {"local": {"rustc": "rustc local test"},
                         "remote_rustc": "rustc remote test", "remote_cargo": "cargo test",
                         "remote_uname": "Linux test"},
            "case_counts": {"generated_count": 1024, "handwritten_count": 56, "total_count": 1080},
            "selected_run_id": hashlib.sha256((corpus_manifest["corpus_digest"] + "\n"
                + hand_manifest["control_set_digest"] + "\n" + "\n".join(ordered_ids)).encode()).hexdigest(),
            "corpus_manifest": {"corpus_digest": corpus_manifest["corpus_digest"], "case_count": 1024,
                "sha256": hashlib.sha256(generated_files[corpus.MANIFEST_NAME]).hexdigest()},
            "hand_control_manifest": {"control_set_digest": hand_manifest["control_set_digest"],
                "case_count": 56, "sha256": hand_sha},
            "inputs": [], "fixtures": [],
        }
        hand_by_id = {row["case_id"]: row for row in hand_rows}
        for index, case_id in enumerate(ordered_ids):
            if index < 56:
                source = hand_by_id[case_id]["fixture_path"]
                fixture_path = f"work/stage2-oracle/run/hand-controls/{case_id}.case.json"
            else:
                source = f"{case_id}.case.json"
                fixture_path = f"work/stage2-oracle/run/fixtures/{case_id}.case.json"
            manifest["inputs"].append({"case_id": case_id, "filename": f"{case_id}.case.json",
                "fixture_path": fixture_path, "source_fixture_path": source,
                "fixture_sha256": digest_by_id[case_id], "schema_version": 2,
                "profile": controls.PROFILE, "group": "handwritten" if index < 56 else "generated"})
        builds = {"interpreter": {"revision": "sha256:" + manifest["source_fingerprint"],
                                  "compiler": manifest["machines"]["local"]["rustc"], "target": "target", "profile": "debug"},
                  "native_x86": {"revision": "sha256:" + manifest["source_fingerprint"],
                                 "compiler": manifest["machines"]["remote_rustc"], "target": "target", "profile": "debug"}}
        machines = {"interpreter": {"host_os": "darwin", "host_arch": "arm64"},
                    "native_x86": {"host_os": "linux", "host_arch": "x86_64"}}
        for repeat in (1, 2):
            for case_id in ordered_ids:
                expected = expected_by_id[case_id]
                manifest["fixtures"].append({"case_id": case_id, "repeat": repeat, "schema_version": 2,
                    "fixture_sha256": digest_by_id[case_id], "build": builds, "machines": machines,
                    "differences": {"interpreter_vs_native": 0}, "stable_repeat": repeat > 1})
                for backend in ("interpreter", "native_x86"):
                    result = {"schema_version": 2, "case_id": case_id,
                        "input_sha256": digest_by_id[case_id], "profile": controls.PROFILE,
                        "backend": backend, "build": builds[backend], "machine": machines[backend],
                        "outcome": expected["outcome"], "instructions_executed": expected["instructions_executed"],
                        "snapshot": {"status": "available", "value": {
                            "state": expected["state"], "rflags_defined": expected["state"]["rflags_defined"],
                            "memory": expected["memory"], "exception": {"status": "available", "value": None}}}}
                    case_dir = run / f"repeat-{repeat:02d}" / case_id
                    case_dir.mkdir(parents=True, exist_ok=True)
                    (case_dir / f"{backend}.json").write_text(json.dumps(result))
                    (case_dir / f"{backend}.process.json").write_text('{"returncode":0}')
        (run / "manifest.json").write_text(json.dumps(manifest))
        return corpus_dir, run

    def test_saved_1080_case_two_backend_run_roundtrip_and_failures(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            corpus_dir, run = self._write_valid_saved_run(Path(temporary))
            summary = corpus.verify_mov_oracle_run(corpus_dir, run)
            self.assertEqual(summary["status"], "passed")
            self.assertEqual((summary["generated_count"], summary["handwritten_count"], summary["total_count"]),
                             (1024, 56, 1080))
            run_manifest_path = run / "manifest.json"
            original = json.loads(run_manifest_path.read_bytes())
            for corruption in ("repeat", "backend", "provenance", "timeout"):
                with self.subTest(corruption=corruption):
                    altered = json.loads(json.dumps(original))
                    if corruption == "repeat":
                        altered["fixtures"].pop()
                    elif corruption == "backend":
                        path = run / "repeat-01" / controls.expected_ids()[0] / "native_x86.process.json"
                        path.unlink()
                    elif corruption == "provenance":
                        altered["fixtures"][0]["machines"]["native_x86"]["host_arch"] = "arm64"
                    else:
                        altered["commands"] = [{"returncode": None, "error": "timed out"}]
                    run_manifest_path.write_text(json.dumps(altered))
                    with self.assertRaises(corpus.CorpusError):
                        corpus.verify_mov_oracle_run(corpus_dir, run)
                    if corruption == "backend":
                        (run / "repeat-01" / controls.expected_ids()[0] / "native_x86.process.json").write_text('{"returncode":0}')
            run_manifest_path.write_text(json.dumps(original))

    def test_saved_run_rejects_missing_extra_symlink_or_tampered_frozen_inputs(self) -> None:
        for mutation in ("missing", "extra", "symlink", "tampered"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temporary:
                corpus_dir, run = self._write_valid_saved_run(Path(temporary))
                case_id = controls.expected_ids()[0]
                path = run / "hand-controls" / f"{case_id}.case.json"
                if mutation == "missing":
                    path.unlink()
                elif mutation == "extra":
                    (run / "hand-controls" / "unexpected.case.json").write_text("{}")
                elif mutation == "tampered":
                    path.write_bytes(path.read_bytes() + b" ")
                else:
                    raw = path.read_bytes()
                    path.unlink()
                    target = run / "hand-controls" / "linked.case.json"
                    target.write_bytes(raw)
                    path.symlink_to(target.name)
                with self.assertRaises(corpus.CorpusError):
                    corpus.verify_mov_oracle_run(corpus_dir, run)

        with tempfile.TemporaryDirectory() as temporary:
            corpus_dir, run = self._write_valid_saved_run(Path(temporary))
            generated_id = json.loads((corpus_dir / corpus.MANIFEST_NAME).read_bytes())["case_digests"][0]["case_id"]
            (run / "fixtures" / f"{generated_id}.case.json").unlink()
            with self.assertRaises(corpus.CorpusError):
                corpus.verify_mov_oracle_run(corpus_dir, run)

    def test_current_acceptance_rejects_legacy_integer_suite(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus_dir, run = self._write_valid_saved_run(root)
            run_manifest_path = run / "manifest.json"
            manifest = json.loads(run_manifest_path.read_bytes())
            manifest["suite"] = "integer"
            run_manifest_path.write_text(json.dumps(manifest))
            with self.assertRaises(corpus.CorpusError):
                corpus.verify_oracle_run(corpus_dir, run)

    def test_saved_run_cli_reports_new_backend_result_count_without_key_error(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus_dir, run = self._write_valid_saved_run(root)
            corpus_manifest = json.loads((corpus_dir / corpus.MANIFEST_NAME).read_bytes())
            output = root / "summary"
            argv = ["generate_stage2_mov_memory_corpus.py", "--verify", str(corpus_dir),
                    "--verify-oracle-run", str(run), "--oracle-output-root", str(output)]
            stdout = io.StringIO()
            with patch.object(corpus.sys, "argv", argv), \
                 patch.object(corpus, "verify_directory", return_value=corpus_manifest), \
                 patch.object(corpus, "run_oracle") as run_oracle, \
                 contextlib.redirect_stdout(stdout):
                self.assertEqual(corpus.main(), 0)
            run_oracle.assert_not_called()
            self.assertIn("verified 2160 saved interpreter and native results per backend", stdout.getvalue())
            summary = json.loads((output / "s2-42e-summary.json").read_bytes())
            self.assertEqual(summary["status"], "passed")
            self.assertEqual(summary["backend_result_count_per_backend"], 2160)

    def test_saved_run_binds_source_fingerprint_and_runtime_compilers(self) -> None:
        for mutation in ("fingerprint", "local-compiler", "remote-compiler", "record-compiler", "result-compiler"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temporary:
                corpus_dir, run = self._write_valid_saved_run(Path(temporary))
                path = run / "manifest.json"
                manifest = json.loads(path.read_bytes())
                if mutation == "fingerprint":
                    manifest["source_fingerprint"] = "b" * 64
                elif mutation == "local-compiler":
                    manifest["machines"]["local"]["rustc"] = "rustc altered"
                elif mutation == "remote-compiler":
                    manifest["machines"]["remote_rustc"] = "rustc altered"
                elif mutation == "record-compiler":
                    manifest["fixtures"][0]["build"]["native_x86"]["compiler"] = "rustc altered"
                else:
                    result_path = run / "repeat-01" / controls.expected_ids()[0] / "interpreter.json"
                    result = json.loads(result_path.read_bytes())
                    result["build"]["compiler"] = "rustc altered"
                    result_path.write_text(json.dumps(result))
                path.write_text(json.dumps(manifest))
                with self.assertRaises(corpus.CorpusError):
                    corpus.verify_mov_oracle_run(corpus_dir, run)


if __name__ == "__main__":
    unittest.main()
