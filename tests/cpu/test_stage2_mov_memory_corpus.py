from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/cpu/generate_stage2_mov_memory_corpus.py"
SPEC = importlib.util.spec_from_file_location("mov_memory_corpus", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)
TEST_BUILD = {
    "revision": "synthetic-revision", "compiler": "synthetic-compiler",
    "target": "synthetic-target", "profile": "debug",
}
TEST_BUILD_NAMES = ("interpreter", "native_x86")
TEST_MACHINES = {
    "interpreter": {"host_os": "darwin", "host_arch": "arm64"},
    "native_x86": {"host_os": "linux", "host_arch": "x86_64"},
}


class MovMemoryCorpusTests(unittest.TestCase):
    def _write_valid_saved_run(self, root: Path) -> tuple[Path, Path, dict, dict]:
        fixture, expected = module.generate_fixture(module.FIXED_SEEDS[0], 0)
        digest = module._sha256(module._json_bytes(fixture))
        corpus = root / "corpus"
        run = root / "run"
        corpus.mkdir()
        run.mkdir()
        (corpus / module.EXPECTED_NAME).write_bytes(module._json_bytes({
            "profile": module.PROFILE, "cases": [expected],
        }))
        run_manifest = {
            "status": "passed", "repeats": 2, "profile": module.PROFILE,
            "inputs": [{
                "case_id": fixture["case_id"], "schema_version": 2,
                "profile": module.PROFILE, "fixture_sha256": digest,
            }],
            "fixtures": [{
                "case_id": fixture["case_id"], "repeat": repeat,
                "schema_version": 2, "fixture_sha256": digest,
                "build": {backend: TEST_BUILD for backend in TEST_BUILD_NAMES},
                "machines": TEST_MACHINES,
                "differences": {"interpreter_vs_native": 0},
                "stable_repeat": repeat > 1,
            } for repeat in (1, 2)],
        }
        (run / "manifest.json").write_bytes(module._json_bytes(run_manifest))
        for repeat in (1, 2):
            for backend in ("interpreter", "native_x86"):
                result = {
                    "schema_version": 2, "case_id": fixture["case_id"],
                    "input_sha256": digest, "profile": module.PROFILE,
                    "backend": backend, "build": TEST_BUILD,
                    "machine": TEST_MACHINES[backend],
                    "outcome": expected["outcome"],
                    "instructions_executed": expected["instructions_executed"],
                    "snapshot": {"status": "available", "value": {
                        "state": expected["state"],
                        "rflags_defined": expected["state"]["rflags_defined"],
                        "memory": expected["memory"],
                        "exception": {"status": "available", "value": None},
                    }},
                }
                result_dir = run / f"repeat-{repeat:02d}" / fixture["case_id"]
                result_dir.mkdir(parents=True, exist_ok=True)
                (result_dir / f"{backend}.json").write_bytes(module._json_bytes(result))
        corpus_manifest = {
            "corpus_digest": "fixture-bound-corpus-digest",
            "case_digests": [{"case_id": fixture["case_id"], "sha256": digest}],
        }
        return corpus, run, corpus_manifest, run_manifest

    def test_fixed_corpus_is_1024_distinct_cases_with_full_matrix(self) -> None:
        first = module.materialize()
        second = module.materialize()
        self.assertEqual(first, second)
        case_files = {name: raw for name, raw in first.items() if name.endswith(".case.json")}
        self.assertEqual(len(case_files), 1024)
        self.assertEqual(len(set(case_files.values())), 1024)
        manifest = json.loads(first[module.MANIFEST_NAME])
        self.assertEqual(manifest["profile"], module.PROFILE)
        self.assertEqual(manifest["widths"], list(module.WIDTHS))
        self.assertEqual(manifest["directions"], list(module.DIRECTIONS))
        self.assertEqual(manifest["address_forms"], list(module.ADDRESS_FORMS))
        self.assertEqual(len(manifest["case_digests"]), 1024)
        effective_offsets = {
            int(entry["effective_address"], 16) - module.DATA_ADDRESS
            for entry in manifest["case_digests"]
        }
        self.assertTrue(set(module.BOUNDARY_OFFSETS).issubset(effective_offsets))
        disp8_values = {
            entry["displacement"] for entry in manifest["case_digests"]
            if entry["address_form"] == "disp8"
        }
        disp32_values = {
            entry["displacement"] for entry in manifest["case_digests"]
            if entry["address_form"] == "disp32"
        }
        self.assertEqual(disp8_values, set(module.DISP8_VALUES))
        self.assertEqual(disp32_values, set(module.DISP32_VALUES))
        matrix = {
            (entry["width"], entry["direction"], entry["address_form"])
            for entry in manifest["case_digests"]
        }
        self.assertEqual(
            matrix,
            {
                (width, direction, form)
                for width in module.WIDTHS
                for direction in module.DIRECTIONS
                for form in module.ADDRESS_FORMS
            },
        )
        self.assertEqual(
            manifest["boundary_vectors"],
            {
                "data_offsets": list(module.BOUNDARY_OFFSETS),
                "displacement8": list(module.DISP8_VALUES),
                "displacement32": list(module.DISP32_VALUES),
                "unaligned_multi_byte_accesses": True,
                "low_and_last_valid_spans": True,
            },
        )

    def test_every_generated_access_is_inside_its_memory_region(self) -> None:
        for seed in module.FIXED_SEEDS:
            for sequence in range(module.CASES_PER_SEED):
                fixture, expected = module.generate_fixture(seed, sequence)
                data = fixture["memory"][1]
                address = int(data["guest_address"], 16)
                candidates = [
                    (width, direction, form)
                    for width in module.WIDTHS
                    for direction in module.DIRECTIONS
                    for form in module.ADDRESS_FORMS
                ]
                width, _, _ = candidates[sequence % len(candidates)]
                gpr = [int(value, 16) for value in fixture["initial_state"]["gpr"]]
                code = bytes.fromhex(fixture["memory"][0]["initial_bytes_hex"])
                prefix_end = 0
                while prefix_end < len(code) and code[prefix_end] in (0x66, 0x67, 0x41, 0x48, 0x49):
                    prefix_end += 1
                opcode = code[prefix_end]
                modrm = code[prefix_end + 1]
                if opcode in (0x8A, 0x8B, 0x88, 0x89):
                    rm = modrm & 7
                    mod = modrm >> 6
                    end = prefix_end + 2
                    if rm == 5 and mod == 0:
                        displacement = int.from_bytes(code[end : end + 4], "little", signed=True)
                        effective = module.CODE_ADDRESS + len(code) + displacement
                    elif rm == 4:
                        sib = code[end]
                        end += 1
                        scale = 1 << (sib >> 6)
                        index = (sib >> 3) & 7
                        base = sib & 7
                        base_value = gpr[base]
                        if 0x67 in code[:prefix_end]:
                            base_value &= 0xFFFFFFFF
                        effective = base_value + (0 if index == 4 else (gpr[index] * scale))
                        if mod == 1:
                            effective += int.from_bytes(code[end : end + 1], "little", signed=True)
                        elif mod == 2:
                            effective += int.from_bytes(code[end : end + 4], "little", signed=True)
                        if 0x67 in code[:prefix_end]:
                            effective &= 0xFFFFFFFF
                    else:
                        base = (modrm & 7) | (8 if any(b in (0x41, 0x49) for b in code[:prefix_end]) else 0)
                        effective = gpr[base]
                        if mod == 1:
                            effective += int.from_bytes(code[end : end + 1], "little", signed=True)
                        elif mod == 2:
                            effective += int.from_bytes(code[end : end + 4], "little", signed=True)
                        if 0x67 in code[:prefix_end]:
                            effective &= 0xFFFFFFFF
                else:
                    self.fail(f"unexpected opcode {opcode:#x}")
                self.assertGreaterEqual(effective, address, fixture["case_id"])
                self.assertLessEqual(effective + width // 8, address + data["length"], fixture["case_id"])
                self.assertEqual(expected["case_id"], fixture["case_id"])

    def test_strict_manifest_and_fixture_tampering_are_rejected(self) -> None:
        contents = module.materialize()
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name, raw in contents.items():
                (directory / name).write_bytes(raw)
            module.verify_directory(directory)
            with self.assertRaises(module.CorpusError):
                (directory / "unexpected.txt").write_text("extra")
                module.verify_directory(directory)
            (directory / "unexpected.txt").unlink()
            fixture_path = next(directory.glob("*.case.json"))
            fixture_path.write_bytes(fixture_path.read_bytes() + b" ")
            with self.assertRaises(module.CorpusError):
                module.verify_directory(directory)

    def test_missing_native_oracle_fails_and_writes_machine_readable_summary(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            module.write_directory(corpus)
            output = root / "oracle"
            argv = ["generate_stage2_mov_memory_corpus.py", "--verify", str(corpus),
                    "--run-oracle", "--oracle-output-root", str(output)]
            failed_native = subprocess.CompletedProcess(
                args=["stage2_oracle.py"], returncode=255, stdout="", stderr="native unavailable"
            )
            with patch.object(module.sys, "argv", argv), patch.object(
                module.subprocess, "run", return_value=failed_native
            ) as oracle_call:
                self.assertEqual(module.main(), 1)
            command = oracle_call.call_args.args[0]
            self.assertIn("--suite", command)
            self.assertIn("mov-memory-corpus", command)
            summary = json.loads((output / "s2-42e-summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual(summary["physical_mac_acceptance"], "not_run")
            self.assertEqual(summary["generated_count"], 1024)
            self.assertEqual(summary["handwritten_count"], 56)
            self.assertEqual(summary["total_count"], 1080)
            self.assertEqual(summary["oracle_stderr"], "native unavailable")

    def test_preflight_missing_or_tampered_corpus_replaces_stale_pass_without_digest(self) -> None:
        for corruption in ("missing_fixture", "tampered_fixture", "tampered_manifest"):
            with self.subTest(corruption=corruption), tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
                root = Path(temporary)
                corpus = root / "corpus"
                module.write_directory(corpus)
                if corruption == "missing_fixture":
                    next(corpus.glob("*.case.json")).unlink()
                elif corruption == "tampered_fixture":
                    fixture_path = next(corpus.glob("*.case.json"))
                    fixture_path.write_bytes(fixture_path.read_bytes() + b" ")
                else:
                    manifest_path = corpus / module.MANIFEST_NAME
                    manifest = json.loads(manifest_path.read_bytes())
                    manifest["profile"] = "other-profile"
                    manifest_path.write_bytes(module._json_bytes(manifest))
                output = root / "oracle"
                output.mkdir()
                summary_path = output / "s2-42e-summary.json"
                summary_path.write_text('{"status":"passed","corpus_digest":"stale"}')
                argv = ["generate_stage2_mov_memory_corpus.py", "--verify", str(corpus),
                        "--run-oracle", "--oracle-output-root", str(output)]
                with patch.object(module.sys, "argv", argv), patch.object(module, "run_oracle") as run_oracle:
                    self.assertEqual(module.main(), 1)
                run_oracle.assert_not_called()
                summary = json.loads(summary_path.read_bytes())
                self.assertEqual(summary["status"], "failed")
                self.assertEqual(summary["profile"], module.PROFILE)
                self.assertEqual(summary["physical_mac_acceptance"], "not_run")
                self.assertNotIn("corpus_digest", summary)
                self.assertIn("error", summary)

    def test_saved_evidence_real_result_roundtrip_and_identity_bindings(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus, run, corpus_manifest, _ = self._write_valid_saved_run(root)
            with patch.object(module, "verify_directory", return_value=corpus_manifest):
                summary = module.verify_legacy_oracle_run(corpus, run)
            self.assertEqual(summary["status"], "passed")
            self.assertEqual(summary["result_count_per_backend"], 2)
            self.assertEqual(summary["native_x86"], "matched_linux_x86_64")

            mutations = (
                ("manifest-profile", "run", "profile", "other-profile"),
                ("input-schema", "input", "schema_version", 1),
                ("input-digest", "input", "fixture_sha256", "0" * 64),
                ("input-profile", "input", "profile", "other-profile"),
                ("record-schema", "record", "schema_version", 1),
                ("record-digest", "record", "fixture_sha256", "0" * 64),
                ("record-differences", "record", "differences", {"interpreter_vs_native": 1}),
                ("record-stability", "record", "stable_repeat", False),
            )
            for label, target, field, bad_value in mutations:
                with self.subTest(identity=label):
                    changed = json.loads((run / "manifest.json").read_bytes())
                    if target == "run":
                        changed[field] = bad_value
                    elif target == "input":
                        changed["inputs"][0][field] = bad_value
                    else:
                        changed["fixtures"][1][field] = bad_value
                    (run / "manifest.json").write_bytes(module._json_bytes(changed))
                    with patch.object(module, "verify_directory", return_value=corpus_manifest):
                        with self.assertRaises(module.CorpusError):
                            module.verify_legacy_oracle_run(corpus, run)
                    (run / "manifest.json").write_bytes(module._json_bytes(_))

    def test_saved_legacy_evidence_is_not_current_acceptance(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            corpus, run, corpus_manifest, _ = self._write_valid_saved_run(root)
            output = root / "summary"
            argv = ["generate_stage2_mov_memory_corpus.py", "--verify", str(corpus),
                    "--verify-oracle-run", str(run), "--oracle-output-root", str(output)]
            with patch.object(module.sys, "argv", argv), patch.object(
                module, "verify_directory", return_value=corpus_manifest
            ), patch.object(module, "run_oracle") as run_oracle:
                self.assertEqual(module.main(), 1)
            run_oracle.assert_not_called()
            summary = json.loads((output / "s2-42e-summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertIn("error", summary)

    def test_result_identity_and_selected_backend_machine_mismatches_are_rejected(self) -> None:
        for field, bad_value in (
            ("schema_version", 1), ("case_id", "other-case"),
            ("input_sha256", "0" * 64), ("profile", "other-profile"),
            ("backend", "interpreter"),
        ):
            with self.subTest(result_field=field), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                corpus, run, corpus_manifest, _ = self._write_valid_saved_run(root)
                native_path = next((run / "repeat-01").glob("*/native_x86.json"))
                result = json.loads(native_path.read_bytes())
                result[field] = bad_value
                native_path.write_bytes(module._json_bytes(result))
                with patch.object(module, "verify_directory", return_value=corpus_manifest):
                    with self.assertRaises(module.CorpusError):
                        module.verify_legacy_oracle_run(corpus, run)

        for corruption in ("result-build", "result-machine", "record-machine"):
            with self.subTest(machine_binding=corruption), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                corpus, run, corpus_manifest, _ = self._write_valid_saved_run(root)
                result_path = next((run / "repeat-01").glob("*/native_x86.json"))
                if corruption == "result-build":
                    result = json.loads(result_path.read_bytes())
                    result["build"]["revision"] = "unbound-revision"
                    result_path.write_bytes(module._json_bytes(result))
                elif corruption == "result-machine":
                    result = json.loads(result_path.read_bytes())
                    result["machine"] = TEST_MACHINES["interpreter"]
                    result_path.write_bytes(module._json_bytes(result))
                else:
                    run_manifest = json.loads((run / "manifest.json").read_bytes())
                    run_manifest["fixtures"][0]["machines"]["native_x86"] = TEST_MACHINES["interpreter"]
                    (run / "manifest.json").write_bytes(module._json_bytes(run_manifest))
                with patch.object(module, "verify_directory", return_value=corpus_manifest):
                    with self.assertRaises(module.CorpusError):
                        module.verify_legacy_oracle_run(corpus, run)

    def test_malformed_saved_json_replaces_stale_pass_summary_with_failure(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            module.write_directory(corpus)
            generated = module.materialize()
            manifest = json.loads(generated[module.MANIFEST_NAME])
            expected = json.loads(generated[module.EXPECTED_NAME])["cases"]
            run_directory = root / "run"
            result_dir = run_directory / "repeat-01" / expected[0]["case_id"]
            result_dir.mkdir(parents=True)
            (run_directory / "manifest.json").write_text(json.dumps({
                "status": "passed",
                "repeats": 2,
                "profile": module.PROFILE,
                "inputs": [
                    {"case_id": item["case_id"], "schema_version": 2, "profile": module.PROFILE,
                     "fixture_sha256": item["sha256"]}
                    for item in manifest["case_digests"]
                ],
                "fixtures": [
                    {"case_id": item["case_id"], "repeat": repeat, "schema_version": 2,
                     "fixture_sha256": item["sha256"], "differences": {"interpreter_vs_native": 0},
                     "stable_repeat": repeat > 1, "build": {"interpreter": TEST_BUILD, "native_x86": TEST_BUILD},
                     "machines": TEST_MACHINES}
                    for repeat in (1, 2) for item in manifest["case_digests"]
                ],
            }))
            result_path = run_directory / "repeat-01" / expected[0]["case_id"] / "interpreter.json"
            result_path.write_text('{"schema_version":')
            output = root / "oracle"
            output.mkdir()
            summary_path = output / "s2-42e-summary.json"
            summary_path.write_text('{"status":"passed"}')
            argv = ["generate_stage2_mov_memory_corpus.py", "--verify", str(corpus),
                    "--run-oracle", "--oracle-output-root", str(output)]
            with patch.object(module.sys, "argv", argv), patch.object(
                module, "run_oracle", return_value=(0, run_directory, "", "")
            ):
                self.assertEqual(module.main(), 1)
            summary = json.loads(summary_path.read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertIn("current MOV acceptance", summary["error"])


if __name__ == "__main__":
    unittest.main()
