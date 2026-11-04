#!/usr/bin/env python3
"""Generate and verify a deterministic S2-42h qword GPR PUSH/POP corpus."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
from typing import Any

from stage2_corpus_support import (
    CorpusError,
    json_bytes,
    sha256,
    strict_object,
    verify_file_set,
    write_file_set,
)


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CORPUS = ROOT / "work/stage2/s2-42h-stack"
DEFAULT_ORACLE_ROOT = ROOT / "work/stage2-42h-oracle"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42h-qword-stack-v1"
MANIFEST_VERSION = 1
FIXED_SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
GENERATED_CASE_COUNT = len(FIXED_SEEDS) * CASES_PER_SEED
REGISTER_NAMES = ("rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
                  "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15")
OPERATIONS = ("push", "pop")
HANDWRITTEN_CASE_COUNT = len(OPERATIONS) * len(REGISTER_NAMES)
TOTAL_CASE_COUNT = GENERATED_CASE_COUNT + HANDWRITTEN_CASE_COUNT
CODE_ADDRESS = 0x00100000
STACK_ADDRESS = 0x00300000
STACK_LENGTH = 256
RFLAGS = 0x802
RFLAGS_MASK = 0x0CD7
MANIFEST_NAME = "manifest.json"
EXPECTED_NAME = "expected.json"
MANIFEST_FIELDS = {
    "manifest_version", "generator_version", "profile", "seeds", "cases_per_seed",
    "operations", "registers", "generated_case_count", "handwritten_case_count", "case_count",
    "case_digests", "expected_sha256", "boundary_vectors", "corpus_digest",
}


def _hx(value: int) -> str:
    return f"0x{value & 0xFFFFFFFFFFFFFFFF:016x}"


class SplitMix64:
    def __init__(self, seed: int, sequence: int) -> None:
        self.state = seed ^ ((sequence * 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF)

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF
        value = self.state
        value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
        value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
        return value ^ (value >> 31)


def _case_id(seed: int, sequence: int) -> str:
    return f"s2_42h_stack_{seed:016x}_{sequence:04x}"


def _instruction(operation: str, register: int) -> bytes:
    if operation not in OPERATIONS or not 0 <= register < 16:
        raise CorpusError("operation or GPR index is outside the S2-42h matrix")
    opcode = (0x50 if operation == "push" else 0x58) + (register & 7)
    return (b"\x41" if register >= 8 else b"") + bytes((opcode,))


def _stack_offset(operation: str, sequence: int, random: SplitMix64) -> int:
    variant = (sequence // 32) % 8
    if operation == "push":
        if variant == 0:
            return 8  # First writable qword.
        if variant == 1:
            return STACK_LENGTH  # Last writable qword.
        if variant == 2:
            return 41  # Unaligned stack pointer and write span.
        if variant == 3:
            return 120
        return 8 + random.next() % (STACK_LENGTH - 7)
    if variant == 0:
        return 0  # First readable qword.
    if variant == 1:
        return STACK_LENGTH - 8  # Last readable qword.
    if variant == 2:
        return 41  # Unaligned read span.
    if variant == 3:
        return 120
    return random.next() % (STACK_LENGTH - 7)


def _make_fixture(case_id: str, seed: int | None, sequence: int | None, operation: str,
                  register: int, old_rsp_offset: int, stack: bytes,
                  gpr: list[int]) -> tuple[dict[str, Any], dict[str, Any]]:
    instruction = _instruction(operation, register)
    gpr[4] = STACK_ADDRESS + old_rsp_offset
    fixture: dict[str, Any] = {
        "schema_version": 2,
        "case_id": case_id,
        "profile": PROFILE,
        "provenance": {
            "kind": "deterministic_generator" if seed is not None else "handwritten_control",
            "source": f"{GENERATOR_VERSION}; seed=0x{seed:016x}; sequence={sequence}"
                      if seed is not None and sequence is not None else f"S2-42h fixed {operation} {REGISTER_NAMES[register]} boundary control",
            "specification": "Intel SDM revision 093, Vol.2B PUSH pp.4-522–4-525 and POP pp.4-398–4-402",
            "specification_version": "Intel SDM revision 093; project S2-17a",
        },
        "seed": _hx(seed) if seed is not None else None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64", "gpr": [_hx(value) for value in gpr],
            "rip": _hx(CODE_ADDRESS), "rflags": _hx(RFLAGS),
        },
        "memory": [
            {
                "region_id": "code", "guest_address": _hx(CODE_ADDRESS), "length": len(instruction),
                "permissions": {"read": True, "write": False, "execute": True},
                "initial_bytes_hex": instruction.hex(), "comparison_mask_hex": "ff" * len(instruction),
            },
            {
                "region_id": "stack", "guest_address": _hx(STACK_ADDRESS), "length": STACK_LENGTH,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": stack.hex(), "comparison_mask_hex": "ff" * STACK_LENGTH,
            },
        ],
        "stop_rip": _hx(CODE_ADDRESS + len(instruction)),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": _hx(RFLAGS_MASK),
        "expected_exit": "stop_address",
    }
    expected = _expected_result(fixture, operation, register, old_rsp_offset)
    access_offset = old_rsp_offset - 8 if operation == "push" else old_rsp_offset
    expected["access"] = {
        "operation": operation,
        "register": register,
        "register_name": REGISTER_NAMES[register],
        "old_rsp_offset": old_rsp_offset,
        "access_offset": access_offset,
        "stack_address": _hx(STACK_ADDRESS + access_offset),
        "old_rsp": _hx(STACK_ADDRESS + old_rsp_offset),
        "boundary": "first_span" if access_offset == 0 else
                    "last_span" if access_offset == STACK_LENGTH - 8 else
                    "unaligned" if access_offset % 8 else "interior",
    }
    return fixture, expected


def generate_fixture(seed: int, sequence: int) -> tuple[dict[str, Any], dict[str, Any]]:
    if seed not in FIXED_SEEDS or not 0 <= sequence < CASES_PER_SEED:
        raise CorpusError("seed or sequence is outside the fixed S2-42h corpus")
    random = SplitMix64(seed, sequence)
    matrix_index = sequence % (len(OPERATIONS) * len(REGISTER_NAMES))
    operation = OPERATIONS[matrix_index // len(REGISTER_NAMES)]
    register = matrix_index % len(REGISTER_NAMES)
    offset = _stack_offset(operation, sequence, random)
    stack = bytearray(random.next() & 0xFF for _ in range(STACK_LENGTH))
    gpr = [random.next() for _ in REGISTER_NAMES]
    return _make_fixture(_case_id(seed, sequence), seed, sequence, operation,
                         register, offset, bytes(stack), gpr)


def _handwritten_fixture(operation: str, register: int) -> tuple[dict[str, Any], dict[str, Any]]:
    if operation == "push":
        offset = 8 if register % 4 == 0 else STACK_LENGTH if register % 4 == 1 else 9 + (register * 7) % 240
    else:
        offset = 0 if register % 4 == 0 else STACK_LENGTH - 8 if register % 4 == 1 else 1 + (register * 7) % 247
    stack = bytearray([0xA5] * STACK_LENGTH)
    if operation == "pop":
        pop_value = (0x8877665544332211 ^ (register * 0x0101010101010101)) & 0xFFFFFFFFFFFFFFFF
        stack[offset:offset + 8] = pop_value.to_bytes(8, "little")
    gpr = [0x1122334455667788 ^ (index * 0x0101010101010101) for index in range(16)]
    case_id = f"s2_42h_hand_{operation}_{REGISTER_NAMES[register]}"
    return _make_fixture(case_id, None, None, operation, register, offset, bytes(stack), gpr)


def _expected_result(fixture: dict[str, Any], operation: str, register: int,
                     offset: int) -> dict[str, Any]:
    """Model only the S2-17a qword stack transfer, independent of the CPU executor."""
    initial = fixture["initial_state"]
    gpr = [int(value, 16) for value in initial["gpr"]]
    stack = bytearray.fromhex(fixture["memory"][1]["initial_bytes_hex"])
    old_rsp = gpr[4]
    if operation == "push":
        value = old_rsp if register == 4 else gpr[register]
        new_rsp = old_rsp - 8
        stack[offset - 8:offset] = value.to_bytes(8, "little")
        gpr[4] = new_rsp
    else:
        value = int.from_bytes(stack[offset:offset + 8], "little")
        gpr[4] = old_rsp + 8
        gpr[register] = value
    return {
        "case_id": fixture["case_id"],
        "state": {
            "mode": "long64", "gpr": [_hx(value) for value in gpr],
            "rip": fixture["stop_rip"], "rflags": initial["rflags"],
            "rflags_defined": _hx(RFLAGS_MASK),
        },
        "memory": [
            {"region_id": region["region_id"], "guest_address": region["guest_address"],
             "bytes_hex": region["initial_bytes_hex"] if region["region_id"] == "code" else stack.hex()}
            for region in fixture["memory"]
        ],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": _hx(1)},
    }


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    case_digests: list[dict[str, Any]] = []
    expected_cases: list[dict[str, Any]] = []

    def add_case(fixture: dict[str, Any], expected: dict[str, Any], kind: str) -> None:
        raw = json_bytes(fixture)
        files[f"{fixture['case_id']}.case.json"] = raw
        case_digests.append({"case_id": fixture["case_id"], "kind": kind,
                             "sha256": sha256(raw), **expected["access"]})
        expected_cases.append(expected)

    for seed in FIXED_SEEDS:
        for sequence in range(CASES_PER_SEED):
            fixture, expected = generate_fixture(seed, sequence)
            add_case(fixture, expected, "generated")
    for operation in OPERATIONS:
        for register in range(len(REGISTER_NAMES)):
            fixture, expected = _handwritten_fixture(operation, register)
            add_case(fixture, expected, "handwritten")
    expected_raw = json_bytes({"profile": PROFILE, "cases": expected_cases})
    files[EXPECTED_NAME] = expected_raw
    manifest = {
        "manifest_version": MANIFEST_VERSION,
        "generator_version": GENERATOR_VERSION,
        "profile": PROFILE,
        "seeds": [f"0x{seed:016x}" for seed in FIXED_SEEDS],
        "cases_per_seed": CASES_PER_SEED,
        "operations": list(OPERATIONS),
        "registers": list(REGISTER_NAMES),
        "generated_case_count": GENERATED_CASE_COUNT,
        "handwritten_case_count": HANDWRITTEN_CASE_COUNT,
        "case_count": len(case_digests),
        "case_digests": case_digests,
        "expected_sha256": sha256(expected_raw),
        "boundary_vectors": {
            "first_push_span": STACK_ADDRESS,
            "last_push_span": STACK_ADDRESS + STACK_LENGTH - 8,
            "first_pop_span": STACK_ADDRESS,
            "last_pop_span": STACK_ADDRESS + STACK_LENGTH - 8,
            "unaligned_stack_offsets": True,
            "all_operation_register_combinations": True,
        },
        "corpus_digest": sha256("\n".join(item["sha256"] for item in case_digests).encode()),
    }
    files[MANIFEST_NAME] = json_bytes(manifest)
    return files


def verify_directory(directory: Path) -> dict[str, Any]:
    expected = materialize()
    verify_file_set(directory, expected)
    manifest = json.loads((directory / MANIFEST_NAME).read_bytes(), object_pairs_hook=strict_object)
    if not isinstance(manifest, dict) or set(manifest) != MANIFEST_FIELDS:
        raise CorpusError("manifest fields do not match the S2-42h contract")
    return manifest


def write_directory(directory: Path) -> None:
    write_file_set(directory, materialize())


def _verify_result(result: Any, expected: dict[str, Any], label: str) -> None:
    try:
        if not isinstance(result, dict) or result.get("case_id") != expected["case_id"]:
            raise CorpusError(f"{label}: case identity differs")
        if result.get("outcome") != expected["outcome"]:
            raise CorpusError(f"{label}: execution outcome differs")
        if result.get("instructions_executed") != expected["instructions_executed"]:
            raise CorpusError(f"{label}: exact one-instruction retirement differs")
        snapshot = result.get("snapshot")
        if not isinstance(snapshot, dict) or snapshot.get("status") != "available":
            raise CorpusError(f"{label}: full snapshot is unavailable")
        actual = snapshot.get("value")
        if not isinstance(actual, dict) or not isinstance(actual.get("state"), dict):
            raise CorpusError(f"{label}: snapshot state must be an object")
        state = actual["state"]
        wanted_state = expected["state"]
        for field in ("mode", "gpr", "rip", "rflags"):
            if state.get(field) != wanted_state[field]:
                raise CorpusError(f"{label}: {field} differs from the independent stack oracle")
        if actual.get("rflags_defined") != wanted_state["rflags_defined"]:
            raise CorpusError(f"{label}: flag-definedness differs")
        memory = actual.get("memory")
        if not isinstance(memory, list) or len(memory) != len(expected["memory"]):
            raise CorpusError(f"{label}: complete memory snapshot is unavailable")
        for observed, wanted in zip(memory, expected["memory"], strict=True):
            if not isinstance(observed, dict) or any(
                observed.get(field) != wanted[field] for field in ("region_id", "guest_address", "bytes_hex")
            ):
                raise CorpusError(f"{label}: memory bytes or canaries differ")
        exception = actual.get("exception")
        if not isinstance(exception, dict) or exception.get("status") != "available" or exception.get("value") is not None:
            raise CorpusError(f"{label}: exception observation is missing or non-empty")
    except (KeyError, TypeError, AttributeError, IndexError, ValueError) as error:
        raise CorpusError(f"{label}: malformed execution result: {error}") from error


def verify_oracle_run(corpus_directory: Path, run_directory: Path) -> dict[str, Any]:
    manifest = verify_directory(corpus_directory)
    expected_document = json.loads((corpus_directory / EXPECTED_NAME).read_bytes(), object_pairs_hook=strict_object)
    run_manifest = json.loads((run_directory / "manifest.json").read_bytes(), object_pairs_hook=strict_object)
    if not isinstance(run_manifest, dict) or run_manifest.get("status") != "passed" or run_manifest.get("repeats") != 2:
        raise CorpusError("interpreter/native oracle did not pass two repeats")
    cases = expected_document.get("cases") if isinstance(expected_document, dict) else None
    inputs, records = run_manifest.get("inputs"), run_manifest.get("fixtures")
    if not isinstance(cases, list) or not isinstance(inputs, list) or not isinstance(records, list):
        raise CorpusError("oracle manifest case collections must be arrays")
    expected_by_id = {entry["case_id"]: entry for entry in cases if isinstance(entry, dict) and isinstance(entry.get("case_id"), str)}
    if len(expected_by_id) != len(cases) or len(inputs) != len(cases):
        raise CorpusError("oracle manifest omitted or duplicated generated cases")
    digest_by_id = {item["case_id"]: item["sha256"] for item in manifest["case_digests"]}
    if any(not isinstance(entry, dict) for entry in inputs):
        raise CorpusError("oracle input manifest has malformed entries")
    if {entry.get("case_id") for entry in inputs} != set(expected_by_id):
        raise CorpusError("oracle input identities do not match generated fixtures")
    if any(entry.get("profile") != PROFILE or entry.get("fixture_sha256") != digest_by_id.get(entry.get("case_id")) for entry in inputs):
        raise CorpusError("oracle input profile or exact fixture digest differs")
    if len(records) != 2 * len(cases):
        raise CorpusError("oracle manifest omitted repeat records")
    seen: set[tuple[str, int]] = set()
    for record in records:
        if not isinstance(record, dict):
            raise CorpusError("oracle result record must be an object")
        case_id, repeat = record.get("case_id"), record.get("repeat")
        if case_id not in expected_by_id or type(repeat) is not int or repeat not in (1, 2) or (case_id, repeat) in seen:
            raise CorpusError("oracle manifest has an unexpected or duplicate result identity")
        seen.add((case_id, repeat))
        for backend in ("interpreter", "native_x86"):
            path = run_directory / f"repeat-{repeat:02d}" / case_id / f"{backend}.json"
            result = json.loads(path.read_bytes(), object_pairs_hook=strict_object)
            _verify_result(result, expected_by_id[case_id], f"{backend} {case_id} repeat {repeat}")
            if backend == "native_x86":
                machine = result.get("machine")
                if not isinstance(machine, dict) or machine.get("host_os") != "linux" or machine.get("host_arch") != "x86_64":
                    raise CorpusError("native result is not from Linux x86_64 hardware")
    if len(seen) != 2 * len(cases):
        raise CorpusError("oracle repeat matrix is incomplete")
    return {
        "status": "passed", "profile": PROFILE, "corpus_digest": manifest["corpus_digest"],
        "case_count": len(cases), "repeat_count": 2, "operations": list(OPERATIONS),
        "registers": list(REGISTER_NAMES), "result_count_per_backend": 2 * len(cases),
        "independent_stack_oracle": "matched_full_gpr_rip_rflags_retirement_memory_canaries",
        "interpreter": "matched", "native_x86": "matched_linux_x86_64",
        "physical_mac_acceptance": "not_run", "oracle_manifest": str(run_directory / "manifest.json"),
    }


def run_oracle(corpus_directory: Path, host: str, user: str, remote_root: str,
               output_root: Path) -> tuple[int, Path | None, str, str]:
    output_root.mkdir(parents=True, exist_ok=True)
    prior = {entry.resolve() for entry in output_root.iterdir() if entry.is_dir()}
    fixtures = sorted(corpus_directory.glob("*.case.json"))
    command = [
        sys.executable, str(ROOT / "tests/cpu/stage2_oracle.py"), "--host", host,
        "--user", user, "--remote-root", remote_root, "--repeats", "2",
        "--output-root", str(output_root.resolve().relative_to(ROOT)),
        *(str(path.resolve().relative_to(ROOT)) for path in fixtures),
    ]
    try:
        process = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
        created = [entry for entry in output_root.iterdir() if entry.is_dir() and entry.resolve() not in prior]
        run_directory = created[0] if len(created) == 1 else None
        if run_directory is not None:
            (run_directory / "s2-42h-command.stdout.log").write_text(process.stdout)
            (run_directory / "s2-42h-command.stderr.log").write_text(process.stderr)
        return process.returncode, run_directory, process.stdout, process.stderr
    except OSError as error:
        return 127, None, "", str(error)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--output-dir", type=Path)
    action.add_argument("--verify", type=Path, metavar="DIR")
    parser.add_argument("--run-oracle", action="store_true", help="run two interpreter and Linux x86 native repeats")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--oracle-output-root", type=Path, default=DEFAULT_ORACLE_ROOT)
    args = parser.parse_args()
    corpus = args.verify if args.verify is not None else (args.output_dir or DEFAULT_CORPUS)
    try:
        if args.verify is None:
            write_directory(corpus)
        manifest = verify_directory(corpus)
        if not args.run_oracle:
            print(f"verified {manifest['case_count']} cases; corpus digest {manifest['corpus_digest']}")
            return 0
        status, run_directory, stdout, stderr = run_oracle(
            corpus, args.host, args.user, args.remote_root, args.oracle_output_root
        )
        summary: dict[str, Any] = {
            "status": "failed", "profile": PROFILE, "corpus_digest": manifest["corpus_digest"],
            "case_count": TOTAL_CASE_COUNT, "physical_mac_acceptance": "not_run",
            "oracle_stdout": stdout[-4000:], "oracle_stderr": stderr[-4000:],
        }
        if run_directory is not None and (run_directory / "manifest.json").exists() and status == 0:
            try:
                summary.update(verify_oracle_run(corpus, run_directory))
            except (CorpusError, OSError, UnicodeError, json.JSONDecodeError, KeyError, TypeError,
                    AttributeError, IndexError, ValueError) as error:
                summary["error"] = str(error)
        summary_path = args.oracle_output_root / "s2-42h-summary.json"
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_bytes(json_bytes(summary))
        print(f"machine-readable summary: {summary_path}")
        if status != 0 or summary.get("status") != "passed":
            print("S2-42h oracle failed or native evidence is missing", file=sys.stderr)
            return 1
        print(f"matched {summary['result_count_per_backend']} interpreter and native results")
        return 0
    except (CorpusError, OSError, UnicodeError, json.JSONDecodeError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
