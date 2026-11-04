from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tests/cpu/generate_stage2_stack_corpus.py"
SPEC = importlib.util.spec_from_file_location("stage2_stack_corpus", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class StackCorpusTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.files = module.materialize()

    def test_manifest_binds_1024_generated_and_32_handwritten_unique_cases(self) -> None:
        self.assertEqual(self.files, module.materialize())
        cases = {name: raw for name, raw in self.files.items() if name.endswith(".case.json")}
        self.assertEqual(len(cases), 1056)
        self.assertEqual(len(set(cases.values())), 1056)
        manifest = json.loads(self.files[module.MANIFEST_NAME])
        self.assertEqual(manifest["generated_case_count"], 1024)
        self.assertEqual(manifest["handwritten_case_count"], 32)
        self.assertEqual(manifest["case_count"], 1056)
        self.assertEqual(manifest["operations"], ["push", "pop"])
        self.assertEqual(manifest["registers"], list(module.REGISTER_NAMES))
        generated = [item for item in manifest["case_digests"] if item["kind"] == "generated"]
        handwritten = [item for item in manifest["case_digests"] if item["kind"] == "handwritten"]
        self.assertEqual(len(generated), 1024)
        self.assertEqual(len(handwritten), 32)
        matrix = {(item["operation"], item["register"]) for item in generated}
        self.assertEqual(matrix, {(op, reg) for op in module.OPERATIONS for reg in range(16)})
        handwritten_matrix = {(item["operation"], item["register"]) for item in handwritten}
        self.assertEqual(handwritten_matrix, matrix)

    def test_exact_encoding_and_rsp_alias_ordering(self) -> None:
        for operation in module.OPERATIONS:
            for register in range(16):
                fixture, _ = module._handwritten_fixture(operation, register)
                code = bytes.fromhex(fixture["memory"][0]["initial_bytes_hex"])
                opcode = (0x50 if operation == "push" else 0x58) + (register & 7)
                self.assertEqual(code, (b"\x41" if register >= 8 else b"") + bytes((opcode,)))

        push_rsp, push_expected = module.generate_fixture(module.FIXED_SEEDS[0], 4)
        old_rsp = module.STACK_ADDRESS + push_expected["access"]["old_rsp_offset"]
        push_memory = bytes.fromhex(push_expected["memory"][1]["bytes_hex"])
        push_offset = push_expected["access"]["access_offset"]
        self.assertEqual(push_memory[push_offset:push_offset + 8], old_rsp.to_bytes(8, "little"))
        self.assertEqual(push_expected["state"]["gpr"][4], module._hx(old_rsp - 8))

        pop_rsp, pop_expected = module.generate_fixture(module.FIXED_SEEDS[0], 20)
        old_offset = pop_expected["access"]["old_rsp_offset"]
        original_stack = bytes.fromhex(pop_rsp["memory"][1]["initial_bytes_hex"])
        popped = int.from_bytes(original_stack[old_offset:old_offset + 8], "little")
        self.assertEqual(pop_expected["state"]["gpr"][4], module._hx(popped))

    def test_handwritten_controls_cover_first_last_and_unaligned_spans(self) -> None:
        manifest = json.loads(self.files[module.MANIFEST_NAME])
        controls = {entry["case_id"]: entry for entry in manifest["case_digests"] if entry["kind"] == "handwritten"}
        self.assertEqual(controls["s2_42h_hand_push_rax"]["boundary"], "first_span")
        self.assertEqual(controls["s2_42h_hand_push_rcx"]["boundary"], "last_span")
        self.assertEqual(controls["s2_42h_hand_push_rdx"]["boundary"], "unaligned")
        self.assertEqual(controls["s2_42h_hand_pop_rax"]["boundary"], "first_span")
        self.assertEqual(controls["s2_42h_hand_pop_rcx"]["boundary"], "last_span")
        self.assertEqual(controls["s2_42h_hand_pop_rdx"]["boundary"], "unaligned")
        for case_id, control in controls.items():
            fixture = json.loads(self.files[f"{case_id}.case.json"])
            expected_stack = bytes.fromhex(fixture["memory"][1]["initial_bytes_hex"])
            actual_stack = bytes.fromhex(module._expected_result(
                fixture, control["operation"], control["register"], control["old_rsp_offset"]
            )["memory"][1]["bytes_hex"])
            start = control["access_offset"]
            if control["operation"] == "push":
                self.assertEqual(actual_stack[:start], expected_stack[:start])
                self.assertEqual(actual_stack[start + 8:], expected_stack[start + 8:])
            else:
                self.assertEqual(actual_stack, expected_stack)

    def test_strict_bundle_verification_rejects_missing_or_tampered_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name, raw in self.files.items():
                (directory / name).write_bytes(raw)
            module.verify_directory(directory)
            fixture_path = next(directory.glob("*.case.json"))
            fixture_path.write_bytes(fixture_path.read_bytes() + b" ")
            with self.assertRaises(module.CorpusError):
                module.verify_directory(directory)

    def test_missing_native_backend_writes_failed_machine_summary(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            module.write_directory(corpus)
            output = root / "oracle"
            argv = ["generate_stage2_stack_corpus.py", "--verify", str(corpus),
                    "--run-oracle", "--oracle-output-root", str(output)]
            failure = subprocess.CompletedProcess(["stage2_oracle.py"], 255, "", "native unavailable")
            with patch.object(module.sys, "argv", argv), patch.object(
                module.subprocess, "run", return_value=failure
            ) as call:
                self.assertEqual(module.main(), 1)
            command = call.call_args.args[0]
            self.assertEqual(sum(argument.endswith(".case.json") for argument in command), 1056)
            summary = json.loads((output / "s2-42h-summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual(summary["case_count"], 1056)
            self.assertEqual(summary["physical_mac_acceptance"], "not_run")
            self.assertEqual(summary["oracle_stderr"], "native unavailable")

    def test_malformed_result_replaces_stale_success_summary(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            module.write_directory(corpus)
            manifest = json.loads(self.files[module.MANIFEST_NAME])
            expected = json.loads(self.files[module.EXPECTED_NAME])["cases"]
            run = root / "run"
            (run / "repeat-01" / expected[0]["case_id"]).mkdir(parents=True)
            (run / "manifest.json").write_text(json.dumps({
                "status": "passed", "repeats": 2,
                "inputs": [{"case_id": item["case_id"], "profile": module.PROFILE,
                            "fixture_sha256": item["sha256"]} for item in manifest["case_digests"]],
                "fixtures": [{"case_id": item["case_id"], "repeat": repeat}
                             for repeat in (1, 2) for item in manifest["case_digests"]],
            }))
            malformed = {"case_id": expected[0]["case_id"], "outcome": expected[0]["outcome"],
                         "instructions_executed": expected[0]["instructions_executed"],
                         "snapshot": {"status": "available", "value": {"state": None}}}
            result = run / "repeat-01" / expected[0]["case_id"] / "interpreter.json"
            result.write_text(json.dumps(malformed))
            output = root / "oracle"
            output.mkdir()
            summary_path = output / "s2-42h-summary.json"
            summary_path.write_text('{"status":"passed"}')
            argv = ["generate_stage2_stack_corpus.py", "--verify", str(corpus),
                    "--run-oracle", "--oracle-output-root", str(output)]
            with patch.object(module.sys, "argv", argv), patch.object(
                module, "run_oracle", return_value=(0, run, "", "")
            ):
                self.assertEqual(module.main(), 1)
            summary = json.loads(summary_path.read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertIn("snapshot state must be an object", summary["error"])


if __name__ == "__main__":
    unittest.main()
