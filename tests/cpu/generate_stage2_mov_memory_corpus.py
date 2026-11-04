#!/usr/bin/env python3
"""Generate and verify a strict S2-42e native-admitted MOV memory corpus.

The inputs use the existing s2-long-integer-v1 profile and already implemented
MOV load/store forms. The independent result oracle below models only these
one-instruction transfers; it does not decode or execute x86 instructions.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
from typing import Any

from stage2_corpus_support import (
    CorpusError,
    json_bytes as _json_bytes,
    sha256 as _sha256,
    strict_object as _strict_object,
    verify_file_set,
    write_file_set,
)


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CORPUS = ROOT / "work/stage2/s2-42e-mov-memory"
DEFAULT_ORACLE_ROOT = ROOT / "work/stage2-42e-oracle"
GENERATOR_VERSION = "s2-42e-mov-memory-v1"
MANIFEST_VERSION = 1
PROFILE = "s2-long-integer-v1"
FIXED_SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
CODE_ADDRESS = 0x00100000
DATA_ADDRESS = 0x00200000
DATA_LENGTH = 128
WIDTHS = (8, 16, 32, 64)
ADDRESS_FORMS = ("base", "disp8", "disp32", "sib", "rip_relative", "address32", "rex_b")
DIRECTIONS = ("load", "store")
MANIFEST_NAME = "manifest.json"
EXPECTED_NAME = "expected.json"
MANIFEST_FIELDS = {
    "manifest_version", "generator_version", "profile", "seeds", "cases_per_seed",
    "widths", "directions", "address_forms", "case_digests", "expected_sha256",
    "boundary_vectors", "corpus_digest",
}
BOUNDARY_OFFSETS = (0, 1, 7, 8, 15, 16, 31, 63, 64, 119, 120)
DISP8_VALUES = (-128, -17, -1, 0, 1, 17, 127)
DISP32_VALUES = (-0x12345, -0x10000, 0x10000, 0x12345)


def _hx(value: int) -> str:
    return f"0x{value & 0xFFFFFFFFFFFFFFFF:016x}"


def _encode(data: bytes) -> str:
    return data.hex()


class SplitMix64:
    def __init__(self, seed: int, sequence: int) -> None:
        self.state = (seed ^ ((sequence * 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF))

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF
        value = self.state
        value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
        value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
        return value ^ (value >> 31)


def _case_id(seed: int, sequence: int) -> str:
    return f"s2_42e_mov_{seed:016x}_{sequence:04x}"


def _modrm(mod: int, reg: int, rm: int) -> int:
    return (mod << 6) | (reg << 3) | rm


def _instruction(width: int, direction: str, form: str, target_offset: int,
                 sequence: int) -> tuple[bytes, dict[int, int], int]:
    """Return instruction bytes, effective GPR initializers and data byte offset."""
    if width not in WIDTHS or direction not in DIRECTIONS or form not in ADDRESS_FORMS:
        raise CorpusError("unsupported generator matrix entry")
    prefixes = bytearray()
    if width == 16:
        prefixes.append(0x66)
    if width == 64:
        prefixes.append(0x48)
    if form == "address32":
        prefixes.insert(0, 0x67)
    if form == "rex_b":
        if width == 64:
            prefixes[-1] |= 0x01
        else:
            prefixes.append(0x41)
    opcode = 0x8A if direction == "load" and width == 8 else \
             0x8B if direction == "load" else \
             0x88 if width == 8 else 0x89
    operand = bytearray()
    initializers: dict[int, int] = {}
    if form in ("base", "disp8", "disp32", "sib", "address32"):
        base_register = 3  # RBX / EBX
        index_value = sequence % 3
        if form == "base":
            mod, rm = 0, 3
            displacement = 0
        elif form == "disp8":
            mod, rm = 1, 3
            displacement = DISP8_VALUES[(sequence // len(ADDRESS_FORMS)) % len(DISP8_VALUES)]
        elif form == "disp32":
            mod, rm = 2, 3
            displacement = DISP32_VALUES[sequence % len(DISP32_VALUES)]
        elif form == "sib":
            mod, rm = 1, 4
            displacement = (sequence % 15) - 7
            operand.append(_modrm(mod, 0, rm))
            scale = 2
            operand.append((scale << 6) | (1 << 3) | base_register)
            initializers[1] = index_value
        if form != "sib" and form != "address32":
            operand.append(_modrm(mod, 0, rm))
        if form == "address32":
            mod = 1
            operand.append(_modrm(mod, 0, 4))
            operand.append((1 << 3) | base_register)
            displacement = (sequence % 11) - 5
            initializers[1] = index_value
        index_contribution = initializers.get(1, 0) * (4 if form == "sib" else 1)
        base_value = DATA_ADDRESS + target_offset - index_contribution - displacement
        if form == "address32":
            base_value &= 0xFFFFFFFF
            base_value |= 0xFFFF_FFFF_0000_0000
        initializers[base_register] = base_value
        if form in ("disp8", "sib", "address32"):
            operand.append(displacement & 0xFF)
        elif form == "disp32":
            operand.extend((displacement & 0xFFFFFFFF).to_bytes(4, "little"))
    elif form == "rip_relative":
        operand.append(_modrm(0, 0, 5))
        length = len(prefixes) + 1 + 1 + 4
        displacement = DATA_ADDRESS + target_offset - (CODE_ADDRESS + length)
        operand.extend((displacement & 0xFFFFFFFF).to_bytes(4, "little"))
    elif form == "rex_b":
        operand.append(_modrm(0, 0, 0))  # [R8]
        initializers[8] = DATA_ADDRESS + target_offset
    instruction = bytes(prefixes) + bytes([opcode]) + bytes(operand)
    return instruction, initializers, target_offset


def generate_fixture(seed: int, sequence: int) -> tuple[dict[str, Any], dict[str, Any]]:
    if seed not in FIXED_SEEDS or not 0 <= sequence < CASES_PER_SEED:
        raise CorpusError("seed or sequence is outside the fixed S2-42e corpus")
    random = SplitMix64(seed, sequence)
    matrix_index = sequence % (len(WIDTHS) * len(DIRECTIONS) * len(ADDRESS_FORMS))
    form = ADDRESS_FORMS[matrix_index % len(ADDRESS_FORMS)]
    direction = DIRECTIONS[(matrix_index // len(ADDRESS_FORMS)) % len(DIRECTIONS)]
    width = WIDTHS[(matrix_index // (len(ADDRESS_FORMS) * len(DIRECTIONS))) % len(WIDTHS)]
    byte_width = width // 8
    max_offset = DATA_LENGTH - byte_width
    offset = BOUNDARY_OFFSETS[(sequence // len(ADDRESS_FORMS)) % len(BOUNDARY_OFFSETS)]
    offset = min(offset, max_offset)
    if sequence % 11 == 10:
        offset = max_offset
    if sequence % 11 == 9:
        offset = 0
    instruction, address_initializers, target_offset = _instruction(width, direction, form, offset, sequence)
    data = bytes(random.next() & 0xFF for _ in range(DATA_LENGTH))
    base_gpr = [
        0x1122334455667788,
        0xAABBCCDDEEFF0011,
        0xDEADBEEFFEDCBA98,
        DATA_ADDRESS + 16,
        0x0000000000300080,
        0x0000000000300080,
        0x00200040,
        0x00200050,
        0x0123456789ABCDEF,
        0xFEDCBA9876543210,
        0x8877665544332211,
        0xAABBCCDDEEFF0011,
        0x1122112211221122,
        0x3344334433443344,
        0x778899AABBCCDDEE,
        0xFFFFFFFFFFFFFFFF,
    ]
    for register, value in address_initializers.items():
        base_gpr[register] = value
    case_id = _case_id(seed, sequence)
    code = instruction
    fixture = {
        "schema_version": 2,
        "case_id": case_id,
        "profile": PROFILE,
        "provenance": {
            "kind": "deterministic_generator",
            "source": f"{GENERATOR_VERSION}; seed=0x{seed:016x}; sequence={sequence}",
            "specification": "Intel SDM revision 093, Vol.2B MOV pp.4-28–4-31; Vol.2A §2.2.1",
            "specification_version": "Intel SDM revision 093; project S2-09/S2-10/S2-42e",
        },
        "seed": _hx(seed),
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64", "gpr": [_hx(value) for value in base_gpr],
            "rip": _hx(CODE_ADDRESS), "rflags": _hx(0x8D7),
        },
        "memory": [
            {
                "region_id": "code", "guest_address": _hx(CODE_ADDRESS), "length": len(code),
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": _encode(code), "comparison_mask_hex": "ff" * len(code),
            },
            {
                "region_id": "data", "guest_address": _hx(DATA_ADDRESS), "length": DATA_LENGTH,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": _encode(data), "comparison_mask_hex": "ff" * DATA_LENGTH,
            },
        ],
        "stop_rip": _hx(CODE_ADDRESS + len(code)),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": _hx(0x0CD7),
        "expected_exit": "stop_address",
    }
    expected = _expected_result(fixture, direction, width, target_offset)
    expected["access"] = {
        "width": width,
        "direction": direction,
        "address_form": form,
        "effective_address": _hx(DATA_ADDRESS + target_offset),
        "displacement": (
            DISP8_VALUES[(sequence // len(ADDRESS_FORMS)) % len(DISP8_VALUES)]
            if form == "disp8" else
            DISP32_VALUES[sequence % len(DISP32_VALUES)]
            if form == "disp32" else
            (sequence % 15) - 7
            if form == "sib" else
            (sequence % 11) - 5
            if form == "address32" else
            DATA_ADDRESS + target_offset - (CODE_ADDRESS + len(instruction))
            if form == "rip_relative" else 0
        ),
    }
    return fixture, expected


def _expected_result(fixture: dict[str, Any], direction: str, width: int,
                     target_offset: int) -> dict[str, Any]:
    """Model one MOV transfer from raw fixture bytes, without using the decoder."""
    initial = fixture["initial_state"]
    gpr = [int(value, 16) for value in initial["gpr"]]
    data = bytearray.fromhex(fixture["memory"][1]["initial_bytes_hex"])
    count = width // 8
    if direction == "load":
        value = int.from_bytes(data[target_offset : target_offset + count], "little")
        old = gpr[0]
        if width == 8:
            gpr[0] = (old & ~0xFF) | value
        elif width == 16:
            gpr[0] = (old & ~0xFFFF) | value
        elif width == 32:
            gpr[0] = value
        else:
            gpr[0] = value
    else:
        value = gpr[0] & ((1 << width) - 1)
        data[target_offset : target_offset + count] = value.to_bytes(count, "little")
    return {
        "case_id": fixture["case_id"],
        "state": {
            "mode": "long64", "gpr": [_hx(value) for value in gpr],
            "rip": fixture["stop_rip"], "rflags": initial["rflags"],
            "rflags_defined": _hx(0x0CD7),
        },
        "memory": [
            {"region_id": region["region_id"], "guest_address": region["guest_address"],
             "bytes_hex": region["initial_bytes_hex"] if region["region_id"] == "code" else _encode(data)}
            for region in fixture["memory"]
        ],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": _hx(1)},
    }


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    case_digests: list[dict[str, str]] = []
    expected_cases: list[dict[str, Any]] = []
    for seed in FIXED_SEEDS:
        for sequence in range(CASES_PER_SEED):
            fixture, expected = generate_fixture(seed, sequence)
            filename = f"{fixture['case_id']}.case.json"
            raw = _json_bytes(fixture)
            files[filename] = raw
            matrix_index = sequence % (len(WIDTHS) * len(DIRECTIONS) * len(ADDRESS_FORMS))
            case_digests.append({
                "case_id": fixture["case_id"], "sha256": _sha256(raw),
                **expected["access"],
            })
            expected_cases.append(expected)
    expected_raw = _json_bytes({"profile": PROFILE, "cases": expected_cases})
    files[EXPECTED_NAME] = expected_raw
    digest = _sha256("\n".join(item["sha256"] for item in case_digests).encode())
    manifest = {
        "manifest_version": MANIFEST_VERSION,
        "generator_version": GENERATOR_VERSION,
        "profile": PROFILE,
        "seeds": [f"0x{seed:016x}" for seed in FIXED_SEEDS],
        "cases_per_seed": CASES_PER_SEED,
        "widths": list(WIDTHS),
        "directions": list(DIRECTIONS),
        "address_forms": list(ADDRESS_FORMS),
        "case_digests": case_digests,
        "expected_sha256": _sha256(expected_raw),
        "boundary_vectors": {
            "data_offsets": list(BOUNDARY_OFFSETS),
            "displacement8": list(DISP8_VALUES),
            "displacement32": list(DISP32_VALUES),
            "unaligned_multi_byte_accesses": True,
            "low_and_last_valid_spans": True,
        },
        "corpus_digest": digest,
    }
    files[MANIFEST_NAME] = _json_bytes(manifest)
    return files


def verify_directory(directory: Path) -> dict[str, Any]:
    expected = materialize()
    verify_file_set(directory, expected)
    manifest = json.loads((directory / MANIFEST_NAME).read_bytes(), object_pairs_hook=_strict_object)
    if set(manifest) != MANIFEST_FIELDS:
        raise CorpusError("manifest fields do not match the S2-42e contract")
    return manifest


def write_directory(directory: Path) -> None:
    expected = materialize()
    write_file_set(directory, expected)


def _result_field(value: Any, label: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"0x[0-9a-f]{16}", value):
        raise CorpusError(f"malformed result field: {label}")
    return value


def _verify_result(result: dict[str, Any], expected: dict[str, Any], label: str) -> None:
    try:
        if not isinstance(result, dict):
            raise CorpusError(f"{label}: result must be an object")
        if result.get("case_id") != expected["case_id"] or result.get("outcome") != expected["outcome"]:
            raise CorpusError(f"{label}: case identity or outcome differs")
        if result.get("instructions_executed") != expected["instructions_executed"]:
            raise CorpusError(f"{label}: exact one-instruction retirement differs")
        snapshot = result["snapshot"]
        if not isinstance(snapshot, dict) or snapshot.get("status") != "available":
            raise CorpusError(f"{label}: full snapshot is unavailable")
        actual = snapshot["value"]
        if not isinstance(actual, dict):
            raise CorpusError(f"{label}: snapshot value must be an object")
        state = actual["state"]
        if not isinstance(state, dict):
            raise CorpusError(f"{label}: architectural state must be an object")
        wanted_state = expected["state"]
        for field in ("mode", "gpr", "rip", "rflags"):
            if state.get(field) != wanted_state[field]:
                raise CorpusError(f"{label}: {field} differs from the independent MOV oracle")
        if actual.get("rflags_defined") != wanted_state["rflags_defined"]:
            raise CorpusError(f"{label}: flag-definedness differs")
        observed_memory = actual.get("memory")
        if not isinstance(observed_memory, list) or len(observed_memory) != len(expected["memory"]):
            raise CorpusError(f"{label}: memory snapshot is incomplete")
        for observed, wanted in zip(observed_memory, expected["memory"], strict=True):
            if not isinstance(observed, dict):
                raise CorpusError(f"{label}: memory entry must be an object")
            if any(observed.get(field) != wanted[field] for field in ("region_id", "guest_address", "bytes_hex")):
                raise CorpusError(f"{label}: memory output or canary differs")
        exception = actual.get("exception")
        if not isinstance(exception, dict):
            raise CorpusError(f"{label}: exception observation must be an object")
        if exception.get("status") != "available" or exception.get("value") is not None:
            raise CorpusError(f"{label}: successful MOV produced an exception or lacked an observation")
    except (KeyError, TypeError, AttributeError, IndexError, ValueError) as error:
        raise CorpusError(f"{label}: malformed execution result: {error}") from error


def verify_oracle_run(corpus_directory: Path, run_directory: Path) -> dict[str, Any]:
    manifest = verify_directory(corpus_directory)
    expected_document = json.loads(
        (corpus_directory / EXPECTED_NAME).read_bytes(), object_pairs_hook=_strict_object
    )
    run_manifest = json.loads((run_directory / "manifest.json").read_bytes(), object_pairs_hook=_strict_object)
    if not isinstance(run_manifest, dict):
        raise CorpusError("oracle run manifest must be an object")
    if (run_manifest.get("status") != "passed" or run_manifest.get("repeats") != 2
            or run_manifest.get("profile") != PROFILE):
        raise CorpusError("interpreter/native oracle did not pass two repeats")
    if not isinstance(expected_document, dict) or expected_document.get("profile") != PROFILE:
        raise CorpusError("independent expected results do not match the selected corpus profile")
    cases = expected_document.get("cases")
    inputs = run_manifest.get("inputs")
    results = run_manifest.get("fixtures")
    if not isinstance(cases, list) or not isinstance(inputs, list) or not isinstance(results, list):
        raise CorpusError("oracle manifest case collections must be arrays")
    expected_by_id = {entry["case_id"]: entry for entry in cases}
    if (len(expected_by_id) != len(cases) or len(inputs) != len(cases)
            or any(not isinstance(entry, dict) for entry in inputs)):
        raise CorpusError("oracle input manifest omitted or duplicated generated cases")
    if {entry.get("case_id") for entry in inputs} != set(expected_by_id):
        raise CorpusError("oracle input identities do not match generated fixtures")
    expected_digests = {item["case_id"]: item["sha256"] for item in manifest["case_digests"]}
    if any(
        type(entry.get("schema_version")) is not int or entry.get("schema_version") != 2
        or entry.get("profile") != PROFILE
        or entry.get("fixture_sha256") != expected_digests.get(entry.get("case_id"))
        for entry in inputs
    ):
        raise CorpusError("oracle input profile or exact fixture digest differs from the corpus manifest")
    if not isinstance(results, list) or len(results) != 2 * len(cases):
        raise CorpusError("oracle manifest omitted repeat records")
    seen: set[tuple[str, int]] = set()
    for record in results:
        if not isinstance(record, dict):
            raise CorpusError("oracle result record must be an object")
        case_id, repeat = record.get("case_id"), record.get("repeat")
        if (not isinstance(case_id, str) or case_id not in expected_by_id
                or type(repeat) is not int or repeat not in (1, 2)):
            raise CorpusError("oracle manifest has an unexpected result identity")
        key = (case_id, repeat)
        if key in seen:
            raise CorpusError("oracle manifest has a duplicate result identity")
        if (type(record.get("schema_version")) is not int or record.get("schema_version") != 2
                or record.get("fixture_sha256") != expected_digests[case_id]
                or record.get("differences") != {"interpreter_vs_native": 0}
                or record.get("stable_repeat") is not (repeat > 1)):
            raise CorpusError("oracle result record does not bind the expected fixture and repeat comparison")
        builds, machines = record.get("build"), record.get("machines")
        if not isinstance(builds, dict) or not isinstance(machines, dict):
            raise CorpusError("oracle result record lacks backend build or machine identity")
        seen.add(key)
        for backend in ("interpreter", "native_x86"):
            result_path = run_directory / f"repeat-{repeat:02d}" / case_id / f"{backend}.json"
            result = json.loads(result_path.read_bytes(), object_pairs_hook=_strict_object)
            if not isinstance(result, dict):
                raise CorpusError(f"{backend} {case_id} repeat {repeat}: result must be an object")
            if (type(result.get("schema_version")) is not int or result.get("schema_version") != 2
                    or result.get("case_id") != case_id
                    or result.get("input_sha256") != expected_digests[case_id]
                    or result.get("profile") != PROFILE
                    or result.get("backend") != backend):
                raise CorpusError(f"{backend} {case_id} repeat {repeat}: result identity differs from selected input/backend")
            build = result.get("build")
            if (not isinstance(build, dict)
                    or any(not isinstance(build.get(field), str) or not build[field]
                           for field in ("revision", "compiler", "target", "profile"))
                    or build != builds.get(backend)):
                raise CorpusError(f"{backend} {case_id} repeat {repeat}: build provenance differs from run manifest/profile")
            machine = result.get("machine")
            if not isinstance(machine, dict) or machine != machines.get(backend):
                raise CorpusError(f"{backend} {case_id} repeat {repeat}: machine provenance differs from selected backend")
            if backend == "native_x86" and (
                machine.get("host_os") != "linux" or machine.get("host_arch") != "x86_64"
            ):
                raise CorpusError("native result is not from Linux x86_64 hardware")
            _verify_result(result, expected_by_id[case_id], f"{backend} {case_id} repeat {repeat}")
    if len(seen) != 2 * len(cases):
        raise CorpusError("oracle repeat matrix is incomplete")
    return {
        "status": "passed",
        "profile": PROFILE,
        "corpus_digest": manifest["corpus_digest"],
        "case_count": len(cases),
        "repeat_count": 2,
        "widths": list(WIDTHS),
        "directions": list(DIRECTIONS),
        "address_forms": list(ADDRESS_FORMS),
        "result_count_per_backend": 2 * len(cases),
        "independent_mov_oracle": "matched_full_gpr_rip_rflags_retirement_memory_canaries",
        "interpreter": "matched",
        "native_x86": "matched_linux_x86_64",
        "physical_mac_acceptance": "not_run",
        "oracle_manifest": str(run_directory / "manifest.json"),
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
    process = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
    created = [entry for entry in output_root.iterdir() if entry.is_dir() and entry.resolve() not in prior]
    run_directory = created[0] if len(created) == 1 else None
    if run_directory is not None:
        (run_directory / "s2-42e-command.stdout.log").write_text(process.stdout)
        (run_directory / "s2-42e-command.stderr.log").write_text(process.stderr)
    return process.returncode, run_directory, process.stdout, process.stderr


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--output-dir", type=Path, help="write corpus (default: work/stage2/s2-42e-mov-memory)")
    action.add_argument("--verify", type=Path, metavar="DIR", help="verify generated inputs and digests")
    parser.add_argument("--run-oracle", action="store_true", help="run two existing interpreter and Linux x86 native repeats")
    parser.add_argument("--verify-oracle-run", type=Path, metavar="DIR",
                        help="verify saved interpreter/native evidence without rerunning backends")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--oracle-output-root", type=Path, default=DEFAULT_ORACLE_ROOT)
    args = parser.parse_args()
    if args.run_oracle and args.verify_oracle_run is not None:
        parser.error("--run-oracle and --verify-oracle-run are mutually exclusive")
    if args.verify_oracle_run is not None and args.verify is None:
        parser.error("--verify-oracle-run requires --verify CORPUS_DIR")
    corpus = args.verify if args.verify is not None else (args.output_dir or DEFAULT_CORPUS)
    summary_path = args.oracle_output_root / "s2-42e-summary.json"
    if args.run_oracle or args.verify_oracle_run is not None:
        try:
            args.oracle_output_root.mkdir(parents=True, exist_ok=True)
            summary_path.write_bytes(_json_bytes({
                "status": "failed",
                "profile": PROFILE,
                "physical_mac_acceptance": "not_run",
                "error": "S2-42e oracle run did not complete",
            }))
        except OSError as error:
            print(f"could not initialize failed machine summary: {error}", file=sys.stderr)
            return 1
    manifest: dict[str, Any] | None = None
    try:
        if args.verify is not None:
            manifest = verify_directory(corpus)
        else:
            write_directory(corpus)
            manifest = verify_directory(corpus)
        if args.verify_oracle_run is not None:
            summary = verify_oracle_run(corpus, args.verify_oracle_run)
            summary_path.write_bytes(_json_bytes(summary))
            print(f"machine-readable summary: {summary_path}")
            print(f"verified {summary['result_count_per_backend']} saved interpreter and native results")
            return 0
        if args.run_oracle:
            return_code, run_directory, stdout, stderr = run_oracle(
                corpus, args.host, args.user, args.remote_root, args.oracle_output_root
            )
            summary: dict[str, Any] = {
                "status": "failed",
                "profile": PROFILE,
                "corpus_digest": manifest["corpus_digest"],
                "case_count": len(FIXED_SEEDS) * CASES_PER_SEED,
                "physical_mac_acceptance": "not_run",
                "oracle_stdout": stdout[-4000:],
                "oracle_stderr": stderr[-4000:],
            }
            if run_directory is not None and (run_directory / "manifest.json").exists():
                summary["oracle_manifest"] = str(run_directory / "manifest.json")
                if return_code == 0:
                    try:
                        summary.update(verify_oracle_run(corpus, run_directory))
                    except (CorpusError, OSError, UnicodeError, json.JSONDecodeError, KeyError, TypeError,
                            AttributeError, IndexError, ValueError) as error:
                        summary["error"] = str(error)
            summary_path.parent.mkdir(parents=True, exist_ok=True)
            summary_path.write_bytes(_json_bytes(summary))
            print(f"machine-readable summary: {summary_path}")
            if return_code != 0 or summary.get("status") != "passed":
                print("S2-42e oracle failed or native evidence is missing", file=sys.stderr)
                return 1
            print(f"matched {summary['result_count_per_backend']} interpreter and native results")
            return 0
        print(f"verified {len(FIXED_SEEDS) * CASES_PER_SEED} generated cases; corpus digest {manifest['corpus_digest']}")
    except (CorpusError, OSError, UnicodeError, json.JSONDecodeError, TypeError, ValueError) as error:
        if args.run_oracle or args.verify_oracle_run is not None:
            summary: dict[str, Any] = {
                "status": "failed",
                "profile": PROFILE,
                "physical_mac_acceptance": "not_run",
                "error": str(error),
            }
            if manifest is not None:
                summary["corpus_digest"] = manifest["corpus_digest"]
                summary["case_count"] = len(manifest["case_digests"])
            try:
                summary_path.parent.mkdir(parents=True, exist_ok=True)
                summary_path.write_bytes(_json_bytes(summary))
                print(f"machine-readable summary: {summary_path}")
            except OSError as write_error:
                print(f"could not write failed machine summary: {write_error}", file=sys.stderr)
            print("S2-42e oracle failed before backend execution", file=sys.stderr)
            return 1
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
