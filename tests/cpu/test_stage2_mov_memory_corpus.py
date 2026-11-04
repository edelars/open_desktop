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


class MovMemoryCorpusTests(unittest.TestCase):
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
            self.assertEqual(sum(argument.endswith(".case.json") for argument in command), 1024)
            summary = json.loads((output / "s2-42e-summary.json").read_bytes())
            self.assertEqual(summary["status"], "failed")
            self.assertEqual(summary["physical_mac_acceptance"], "not_run")
            self.assertEqual(summary["case_count"], 1024)
            self.assertEqual(summary["oracle_stderr"], "native unavailable")

    def test_malformed_result_replaces_any_stale_pass_summary_with_failure(self) -> None:
        with tempfile.TemporaryDirectory(dir=ROOT / "work") as temporary:
            root = Path(temporary)
            corpus = root / "corpus"
            module.write_directory(corpus)
            generated = module.materialize()
            manifest = json.loads(generated[module.MANIFEST_NAME])
            expected = json.loads(generated[module.EXPECTED_NAME])["cases"]
            run_directory = root / "run"
            (run_directory / "repeat-01" / expected[0]["case_id"]).mkdir(parents=True)
            (run_directory / "manifest.json").write_text(json.dumps({
                "status": "passed",
                "repeats": 2,
                "inputs": [
                    {"case_id": item["case_id"], "profile": module.PROFILE,
                     "fixture_sha256": item["sha256"]}
                    for item in manifest["case_digests"]
                ],
                "fixtures": [
                    {"case_id": item["case_id"], "repeat": repeat}
                    for repeat in (1, 2) for item in manifest["case_digests"]
                ],
            }))
            malformed = {
                "case_id": expected[0]["case_id"],
                "outcome": expected[0]["outcome"],
                "instructions_executed": expected[0]["instructions_executed"],
                "snapshot": {"status": "available", "value": {"state": None}},
            }
            result_path = run_directory / "repeat-01" / expected[0]["case_id"] / "interpreter.json"
            result_path.write_text(json.dumps(malformed))
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
            self.assertIn("architectural state must be an object", summary["error"])


if __name__ == "__main__":
    unittest.main()
