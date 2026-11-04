#!/usr/bin/env python3
"""Build, run, and verify the bounded S2-42u CMPXCHG corpus."""

from __future__ import annotations

import argparse
import json
import os
import platform
import random
import subprocess
import sys
from pathlib import Path
from typing import Any

from stage2_corpus_support import CorpusError, json_bytes, sha256, strict_object, verify_file_set, write_file_set


ROOT = Path(__file__).resolve().parents[2]
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42u_cmpxchg_qword_memory_controls"
CONTROL_EXPECTED = CONTROL_DIR / "expected.json"
CONTROL_MANIFEST = CONTROL_DIR / "controls_manifest.json"
CORPUS_DIR = ROOT / "work/stage2/s2-42u-cmpxchg-qword-memory-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42u-cmpxchg-qword-memory-corpus/interpreter-results.json"
NATIVE_BLOCKED_PATH = ROOT / "work/stage2/s2-42u-cmpxchg-qword-memory-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42u-cmpxchg-qword-memory-v1"
CODE_ADDRESS = 0x00100000
DATA_ADDRESS = 0x00002000
CODE = bytes.fromhex("480fb118")  # CMPXCHG qword [RAX], RBX
DATA_LEFT_LENGTH = 7
DATA_RIGHT_LENGTH = 8
OPERAND_ADDRESS = 0x2003
OPERAND_OFFSET = OPERAND_ADDRESS - DATA_ADDRESS
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
REPEATS = 2
RFLAGS_DEFINED = 0x0CD7
ARITHMETIC_FLAGS = 0x08D5
MASK64 = 0xFFFFFFFFFFFFFFFF
GENERATED_COUNT = len(SEEDS) * CASES_PER_SEED
CONTROL_COUNT = 2
TOTAL_COUNT = GENERATED_COUNT + CONTROL_COUNT
MANIFEST_NAME = "manifest.json"
EXPECTED_NAME = "expected.json"
MANIFEST_FIELDS = {
    "manifest_version", "generator_version", "profile", "seeds", "cases_per_seed",
    "control_count", "generated_case_count", "case_count", "case_digests",
    "control_manifest_sha256", "expected_sha256", "corpus_digest",
}
CONTROL_MANIFEST_SHA256 = "12bae0befa199f462ee65858832997992146bd57460e965d567b80e779604b0a"
GENERATED_MEMORY_CLASSES = (
    ("equal", OPERAND_ADDRESS),
    ("below_zero", 0),
    ("below_one", 1),
    ("below_accumulator", OPERAND_ADDRESS - 1),
    ("above_accumulator", OPERAND_ADDRESS + 1),
    ("positive_max", 0x7FFFFFFFFFFFFFFF),
    ("negative_min", 0x8000000000000000),
    ("negative_one", MASK64),
)
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def control_id(pair: int, offset: int) -> str:
    return f"s2_42u_cmpxchg_qword_memory_{'equal' if pair == 0 else 'mismatch'}"


def generated_id(seed_index: int, sequence: int) -> str:
    return f"s2_42u_cmpxchg_qword_memory_g{seed_index}_{sequence:03d}"


def gprs_for(source: int, address: int, values: list[int]) -> list[int]:
    gprs = list(values)
    gprs[0] = address
    gprs[3] = source
    return gprs


def make_fixture(identifier: str, source: int, memory_value: int,
                 gprs: list[int], flags: int, kind: str, seed: int | None = None,
                 sequence: int | None = None) -> dict[str, Any]:
    if len(gprs) != 16 or any(type(v) is not int or not 0 <= v <= MASK64 for v in gprs):
        raise CorpusError("CMPXCHG input is outside the exact S2-42u profile")
    if gprs[0] != OPERAND_ADDRESS or gprs[3] != source:
        raise CorpusError("CMPXCHG RAX/RBX inputs do not match effective address and source")
    if flags & ~RFLAGS_DEFINED or not flags & 2:
        raise CorpusError("initial RFLAGS contain reserved bits or omit the fixed bit 1")
    data = bytearray(b"\xa5" * (DATA_LEFT_LENGTH + DATA_RIGHT_LENGTH))
    data[OPERAND_OFFSET:OPERAND_OFFSET + 8] = memory_value.to_bytes(8, "little")
    return {
        "schema_version": 2,
        "case_id": identifier,
        "profile": PROFILE,
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten exact-byte S2-21e CMPXCHG equal/mismatch control." if kind == "handwritten"
                       else f"S2-42u deterministic private-RAM corpus seed=0x{seed:016x}, sequence={sequence}."),
            "specification": "Intel SDM revision 093, Vol. 2A CMPXCHG: REX.W + 0F B1 /r compares RAX with old memory; equality stores RBX, mismatch loads old memory into RAX; flags reflect subtraction. Bounded project S2-42u profile.",
            "specification_version": "Intel SDM revision 093; project S2-42u",
        },
        "seed": None if seed is None else hx(seed),
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64", "gpr": [hx(value) for value in gprs],
            "rip": hx(CODE_ADDRESS), "rflags": hx(flags),
        },
        "memory": [
            {"region_id": "code", "guest_address": hx(CODE_ADDRESS), "length": len(CODE),
             "permissions": {"read": False, "write": False, "execute": True},
             "initial_bytes_hex": CODE.hex(), "comparison_mask_hex": "ff" * len(CODE)},
            {"region_id": "data_left", "guest_address": hx(DATA_ADDRESS), "length": DATA_LEFT_LENGTH,
             "permissions": {"read": True, "write": True, "execute": False},
             "initial_bytes_hex": data[:DATA_LEFT_LENGTH].hex(), "comparison_mask_hex": "ff" * DATA_LEFT_LENGTH},
            {"region_id": "data_right", "guest_address": hx(DATA_ADDRESS + DATA_LEFT_LENGTH), "length": DATA_RIGHT_LENGTH,
             "permissions": {"read": True, "write": True, "execute": False},
             "initial_bytes_hex": data[DATA_LEFT_LENGTH:].hex(), "comparison_mask_hex": "ff" * DATA_RIGHT_LENGTH},
        ],
        "stop_rip": hx(CODE_ADDRESS + len(CODE)),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(RFLAGS_DEFINED),
        "expected_exit": "stop_address",
    }


def control_inputs() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    rows = ((0, OPERAND_ADDRESS, 0xAABBCCDDEEFF0011),
            (1, 0x1122334455667788, 0xAABBCCDDEEFF0011))
    for pair, memory_value, source in rows:
        base = [OPERAND_ADDRESS, 0x1122334455667788, 0xDEADBEEFFEDCBA98,
                source, 0x5566778899AABBCC, 0x8877665544332211,
                0x0000000000200004, 0x0000000000200010,
                0xAABBCCDDEEFF0011, 0xFEDCBA9876543210,
                0x0123456789ABCDEF, 0x9988776655443322,
                0x1122112211221122, 0x3344334433443344,
                0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
        fixture = make_fixture(control_id(pair, 0), source, memory_value, base,
                               RFLAGS_DEFINED, "handwritten")
        files[f"{fixture['case_id']}.case.json"] = json_bytes(fixture)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    fixtures = []
    input_states: set[tuple[tuple[str, ...], str, str, str]] = set()
    for seed_index, seed in enumerate(SEEDS):
        for sequence in range(CASES_PER_SEED):
            rng = random.Random(seed ^ ((sequence + 1) * 0x9E3779B97F4A7C15))
            _case_class, memory_value = GENERATED_MEMORY_CLASSES[sequence % len(GENERATED_MEMORY_CLASSES)]
            source = rng.getrandbits(64)
            values = [rng.getrandbits(64) for _ in range(16)]
            values[15] = 0x4274000000000000 | (seed_index << 32) | sequence
            gprs = gprs_for(source, OPERAND_ADDRESS, values)
            flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
            fixture = make_fixture(generated_id(seed_index, sequence), source,
                                   memory_value, gprs, flags, "generated", seed, sequence)
            state = fixture["initial_state"]
            input_states.add((tuple(state["gpr"]), state["rip"], state["rflags"],
                              fixture["memory"][1]["initial_bytes_hex"], fixture["memory"][2]["initial_bytes_hex"]))
            fixtures.append(fixture)
    if len(input_states) != GENERATED_COUNT:
        raise CorpusError("generated architectural input states are not unique")
    return fixtures


def arithmetic_flags(accumulator: int, memory_value: int) -> int:
    result = (accumulator - memory_value) & MASK64
    carry = int(accumulator < memory_value)
    overflow = (((accumulator ^ memory_value) & (accumulator ^ result)) >> 63) & 1
    auxiliary = ((accumulator ^ memory_value ^ result) >> 4) & 1
    sign = (result >> 63) & 1
    zero = int(result == 0)
    parity = int((result & 0xFF).bit_count() % 2 == 0)
    return carry | (parity << 2) | (auxiliary << 4) | (zero << 6) | (sign << 7) | (overflow << 11)


def expected_snapshot(fixture: dict[str, Any]) -> dict[str, Any]:
    """Integer-only oracle; independent of decoder and production arithmetic helpers."""
    initial = fixture["initial_state"]
    code = fixture["memory"][0]
    left_region, right_region = fixture["memory"][1:]
    if bytes.fromhex(code["initial_bytes_hex"]) != CODE:
        raise CorpusError("S2-42u oracle requires exact bare effective-REX.W CMPXCHG bytes")
    gprs = [int(value, 16) for value in initial["gpr"]]
    if gprs[0] != OPERAND_ADDRESS or int(left_region["guest_address"], 16) != DATA_ADDRESS or int(right_region["guest_address"], 16) != DATA_ADDRESS + DATA_LEFT_LENGTH:
        raise CorpusError("S2-42u CMPXCHG address or split-region layout differs")
    left = bytearray.fromhex(left_region["initial_bytes_hex"])
    right = bytearray.fromhex(right_region["initial_bytes_hex"])
    data = left + right
    memory_value = int.from_bytes(data[OPERAND_OFFSET:OPERAND_OFFSET + 8], "little")
    accumulator = gprs[0]
    source = gprs[3]
    equal = accumulator == memory_value
    flags = int(initial["rflags"], 16)
    final_flags = (flags & ~ARITHMETIC_FLAGS) | arithmetic_flags(accumulator, memory_value)
    if equal:
        data[OPERAND_OFFSET:OPERAND_OFFSET + 8] = source.to_bytes(8, "little")
    else:
        gprs[0] = memory_value
    return {
        "case_id": fixture["case_id"],
        "state": {"mode": "long64", "gpr": [hx(value) for value in gprs],
                  "rip": hx(CODE_ADDRESS + len(CODE)), "rflags": hx(final_flags),
                  "rflags_defined": hx(RFLAGS_DEFINED)},
        "memory": [
            {"region_id": "code", "guest_address": hx(CODE_ADDRESS), "bytes_hex": CODE.hex()},
            {"region_id": "data_left", "guest_address": hx(DATA_ADDRESS), "bytes_hex": data[:DATA_LEFT_LENGTH].hex()},
            {"region_id": "data_right", "guest_address": hx(DATA_ADDRESS + DATA_LEFT_LENGTH), "bytes_hex": data[DATA_LEFT_LENGTH:].hex()},
        ],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": hx(1)},
        "exception": None,
    }


def load_controls() -> tuple[list[tuple[str, bytes, dict[str, Any]]], bytes, dict[str, Any]]:
    if not CONTROL_DIR.is_dir() or CONTROL_DIR.is_symlink():
        raise CorpusError("S2-42u control directory is missing or unsafe")
    if not CONTROL_EXPECTED.is_file() or CONTROL_EXPECTED.is_symlink() or not CONTROL_MANIFEST.is_file() or CONTROL_MANIFEST.is_symlink():
        raise CorpusError("S2-42u static control snapshots or manifest are missing")
    manifest_raw = CONTROL_MANIFEST.read_bytes()
    if not CONTROL_MANIFEST_SHA256 or sha256(manifest_raw) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned handwritten control-manifest digest changed")
    manifest = json.loads(manifest_raw, object_pairs_hook=strict_object)
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
            or manifest.get("schema_version") != 1 or manifest.get("profile") != PROFILE
            or manifest.get("control_count") != CONTROL_COUNT or not isinstance(manifest.get("fixtures"), list)):
        raise CorpusError("S2-42u static control manifest has invalid shape")
    expected_raw = CONTROL_EXPECTED.read_bytes()
    if sha256(expected_raw) != manifest.get("expected_sha256"):
        raise CorpusError("S2-42u static control snapshots digest differs from pinned manifest")
    expected_doc = json.loads(expected_raw, object_pairs_hook=strict_object)
    if not isinstance(expected_doc, dict) or set(expected_doc) != {"profile", "cases"} or expected_doc["profile"] != PROFILE:
        raise CorpusError("S2-42u static control snapshot document has invalid shape")
    expected_cases = expected_doc["cases"]
    if not isinstance(expected_cases, list) or len(expected_cases) != CONTROL_COUNT:
        raise CorpusError("S2-42u static control snapshot count differs")
    expected_order = [control_id(pair, 0) for pair in range(CONTROL_COUNT)]
    if [case.get("case_id") if isinstance(case, dict) else None for case in expected_cases] != expected_order:
        raise CorpusError("S2-42u static full snapshots are missing, duplicated, or out of order")
    expected_by_id = {case.get("case_id"): case for case in expected_cases if isinstance(case, dict)}
    if len(expected_by_id) != CONTROL_COUNT:
        raise CorpusError("S2-42u static control snapshots have duplicate or missing identities")
    rows = manifest["fixtures"]
    if len(rows) != CONTROL_COUNT:
        raise CorpusError("S2-42u control manifest fixture count differs")
    controls: list[tuple[str, bytes, dict[str, Any]]] = []
    expected_names: set[str] = {"controls_manifest.json", "expected.json"}
    fixed = control_inputs()
    for identifier in expected_order:
        name = f"{identifier}.case.json"
        expected_names.add(name)
        row = rows[len(controls)]
        if not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "expected_sha256"} or row["case_id"] != identifier:
            raise CorpusError("S2-42u pinned controls are missing, extra, duplicated, or out of order")
        path = CONTROL_DIR / name
        if path.is_symlink() or not path.is_file():
            raise CorpusError(f"S2-42u control fixture is missing or unsafe: {identifier}")
        raw = path.read_bytes()
        if sha256(raw) != row["fixture_sha256"] or raw != fixed[name]:
            raise CorpusError(f"S2-42u control fixture digest or fixed bytes differ: {identifier}")
        expected = expected_by_id.get(identifier)
        if not isinstance(expected, dict) or sha256(json_bytes(expected)) != row["expected_sha256"]:
            raise CorpusError(f"S2-42u static full snapshot digest mismatch: {identifier}")
        if expected.get("case_id") != identifier:
            raise CorpusError(f"S2-42u static snapshot identity mismatch: {identifier}")
        controls.append((identifier, raw, expected))
    entries = list(CONTROL_DIR.iterdir())
    if any(p.is_symlink() or not p.is_file() for p in entries) or {p.name for p in entries} != expected_names:
        raise CorpusError("S2-42u controls directory has missing, extra, or symlink entries")
    return controls, expected_raw, manifest


def all_fixtures_and_expected() -> tuple[list[tuple[str, bytes, str, dict[str, Any]]], bytes, list[dict[str, Any]]]:
    controls, control_expected_raw, _ = load_controls()
    generated = generated_fixtures()
    rows: list[tuple[str, bytes, str, dict[str, Any]]] = []
    expected_cases: list[dict[str, Any]] = []
    for identifier, raw, expected in controls:
        rows.append((identifier, raw, "handwritten", expected))
        expected_cases.append(expected)
    for fixture in generated:
        raw = json_bytes(fixture)
        wanted = expected_snapshot(fixture)
        rows.append((fixture["case_id"], raw, "generated", wanted))
        expected_cases.append(wanted)
    return rows, control_expected_raw, expected_cases


def materialize() -> dict[str, bytes]:
    rows, control_expected_raw, expected_cases = all_fixtures_and_expected()
    files: dict[str, bytes] = {}
    digest_rows = []
    for identifier, raw, kind, expected in rows:
        files[f"{identifier}.case.json"] = raw
        expected_raw = json_bytes(expected)
        digest_rows.append({"case_id": identifier, "kind": kind, "fixture_sha256": sha256(raw),
                            "expected_sha256": sha256(expected_raw)})
    expected_document = json_bytes({"profile": PROFILE, "cases": expected_cases})
    manifest = {
        "manifest_version": 1, "generator_version": GENERATOR_VERSION, "profile": PROFILE,
        "seeds": [hx(seed) for seed in SEEDS], "cases_per_seed": CASES_PER_SEED,
        "control_count": CONTROL_COUNT, "generated_case_count": GENERATED_COUNT,
        "case_count": TOTAL_COUNT, "case_digests": digest_rows,
        "control_manifest_sha256": CONTROL_MANIFEST_SHA256,
        "expected_sha256": sha256(expected_document),
        "corpus_digest": sha256("\n".join(row["fixture_sha256"] for row in digest_rows).encode()),
    }
    files[EXPECTED_NAME] = expected_document
    files[MANIFEST_NAME] = json_bytes(manifest)
    # Bind the static control document to the bundle independently of the combined expected file.
    if not control_expected_raw:
        raise CorpusError("static controls document is empty")
    return files


def verify_corpus(directory: Path) -> dict[str, Any]:
    if directory.is_symlink() or not directory.is_dir():
        raise CorpusError("S2-42u corpus directory is missing or unsafe")
    files = materialize()
    verify_file_set(directory, files)
    manifest = load_json(directory / MANIFEST_NAME)
    if not isinstance(manifest, dict) or set(manifest) != MANIFEST_FIELDS:
        raise CorpusError("manifest fields do not match the S2-42u contract")
    if manifest.get("case_count") != TOTAL_COUNT or len(manifest.get("case_digests", [])) != TOTAL_COUNT:
        raise CorpusError("S2-42u manifest case accounting is invalid")
    return manifest


def validate_provenance(build: Any, machine: Any, label: str) -> None:
    if not isinstance(build, dict) or any(not isinstance(build.get(k), str) or not build[k]
                                          for k in ("revision", "compiler", "target", "profile")):
        raise CorpusError(f"{label}: incomplete build/compiler provenance")
    if not isinstance(machine, dict) or any(not isinstance(machine.get(k), str) or not machine[k]
                                            for k in ("host_os", "host_arch")):
        raise CorpusError(f"{label}: incomplete machine provenance")


def compare_result(result: Any, fixture_raw: bytes, expected: dict[str, Any], backend: str,
                   build: Any, machine: Any, label: str) -> None:
    if not isinstance(result, dict):
        raise CorpusError(f"{label}: result must be an object")
    identity = {"schema_version": 2, "case_id": expected["case_id"],
                "input_sha256": sha256(fixture_raw), "profile": PROFILE, "backend": backend,
                "outcome": expected["outcome"], "instructions_executed": expected["instructions_executed"],
                "build": build, "machine": machine}
    for field, wanted in identity.items():
        if result.get(field) != wanted:
            raise CorpusError(f"{label}: {field} differs from exact S2-42u identity/oracle")
    validate_provenance(result.get("build"), result.get("machine"), label)
    snapshot = result.get("snapshot")
    if not isinstance(snapshot, dict) or snapshot.get("status") != "available" or not isinstance(snapshot.get("value"), dict):
        raise CorpusError(f"{label}: full architectural snapshot is unavailable")
    actual = snapshot["value"]
    state = expected["state"]
    if actual.get("state") != {k: state[k] for k in ("mode", "gpr", "rip", "rflags")}:
        raise CorpusError(f"{label}: complete GPR/RIP/RFLAGS state differs from integer oracle")
    if actual.get("rflags_defined") != state["rflags_defined"]:
        raise CorpusError(f"{label}: RFLAGS definedness differs")
    if actual.get("memory") != expected["memory"]:
        raise CorpusError(f"{label}: full code/data bytes or canaries differ")
    exception = actual.get("exception")
    if not isinstance(exception, dict) or exception.get("status") != "available" or exception.get("value") is not None:
        raise CorpusError(f"{label}: exception observation is missing or nonempty")


def command_identity(executable: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    executable = executable.resolve(strict=True)
    if not executable.is_file():
        raise CorpusError("selected runner is not a regular file")
    digest = sha256(executable.read_bytes())
    revision = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
    host_os = platform.system().lower()
    host_arch = platform.machine().lower()
    host_os = {"darwin": "macos"}.get(host_os, host_os)
    host_arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(host_arch, host_arch)
    return {"executable_sha256": digest, "revision": revision}, {"host_os": host_os, "host_arch": host_arch}


def expected_case_ids() -> list[str]:
    return ([control_id(pair, 0) for pair in range(CONTROL_COUNT)]
            + [generated_id(seed_index, sequence)
               for seed_index in range(len(SEEDS)) for sequence in range(CASES_PER_SEED)])


def missing_attempts(case_ids: list[str], rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    recorded = {row.get("case_id"): row for row in rows if isinstance(row, dict)}
    missing = []
    for case_id in case_ids:
        row = recorded.get(case_id, {})
        processes = row.get("processes", [])
        observations = row.get("repeats", [])
        for repeat in range(REPEATS):
            process = processes[repeat] if isinstance(processes, list) and repeat < len(processes) else None
            observation = observations[repeat] if isinstance(observations, list) and repeat < len(observations) else None
            if (not isinstance(process, dict) or process.get("returncode") != 0
                    or not isinstance(observation, dict) or "failure" in observation):
                missing.append({"case_id": case_id, "repeat": repeat + 1})
    return missing


def process_environment() -> dict[str, str | None]:
    return {key: os.environ.get(key) for key in PROCESS_ENV_KEYS}


def output_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def write_incomplete_summary(results_path: Path) -> None:
    results_path.parent.mkdir(parents=True, exist_ok=True)
    results_path.write_bytes(json_bytes({
        "schema_version": 1, "status": "incomplete", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS,
        "results": [], "failures": ["run is in progress or preflight did not complete"],
        "missing_attempts": missing_attempts(expected_case_ids(), []),
    }))


def write_run_checkpoint(directory: Path, results_path: Path, manifest: dict[str, Any], rows: list[dict[str, Any]],
                         failures: list[str], identity: dict[str, Any], execution_machine: dict[str, Any],
                         runner_build: dict[str, Any] | None, runner_machine: dict[str, Any] | None) -> None:
    case_ids = [item["case_id"] for item in manifest["case_digests"]]
    results_path.parent.mkdir(parents=True, exist_ok=True)
    results_path.write_bytes(json_bytes({
        "schema_version": 1, "status": "incomplete", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "manifest_sha256": sha256((directory / MANIFEST_NAME).read_bytes()), "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "execution_identity": identity,
        "execution_machine": execution_machine, "runner_build": runner_build,
        "runner_machine": runner_machine, "results": rows, "failures": failures,
        "missing_attempts": missing_attempts(case_ids, rows),
    }))


def write_failed_summary(results_path: Path, error: BaseException) -> None:
    try:
        summary = load_json(results_path)
    except (CorpusError, OSError, UnicodeError, json.JSONDecodeError):
        summary = {}
    if not isinstance(summary, dict) or summary.get("status") not in ("incomplete", "failed"):
        summary = {}
    summary.update({"schema_version": 1, "status": "failed", "backend": "interpreter",
                    "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
                    "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS})
    failures = summary.get("failures")
    if not isinstance(failures, list):
        failures = []
    message = str(error)
    if message not in failures:
        failures.append(message)
    summary["failures"] = failures
    summary["error"] = message
    results_path.parent.mkdir(parents=True, exist_ok=True)
    results_path.write_bytes(json_bytes(summary))


def run_interpreter(directory: Path, results_path: Path, executable: Path) -> dict[str, Any]:
    write_incomplete_summary(results_path)
    try:
        manifest = verify_corpus(directory)
    except (CorpusError, OSError, UnicodeError, json.JSONDecodeError) as error:
        write_failed_summary(results_path, error)
        raise
    identity, execution_machine = command_identity(executable)
    expected_doc = load_json(directory / EXPECTED_NAME)
    expected_by_id = {item["case_id"]: item for item in expected_doc["cases"]}
    rows: list[dict[str, Any]] = []
    failures: list[str] = []
    runner_build = runner_machine = None
    recorded_environment = process_environment()
    child_environment = {key: value for key, value in recorded_environment.items() if value is not None}
    process_cwd = str(ROOT.resolve())
    for item in manifest["case_digests"]:
        identifier = item["case_id"]
        fixture_path = (directory / f"{identifier}.case.json").resolve(strict=True)
        raw = fixture_path.read_bytes()
        observations = []
        processes = []
        row = {"case_id": identifier, "fixture_sha256": sha256(raw),
               "repeats": observations, "processes": processes}
        rows.append(row)
        for repeat in range(REPEATS):
            command = [str(executable.resolve(strict=True)), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"]
            try:
                process = subprocess.run(command, capture_output=True, text=True, check=False, timeout=RUNNER_TIMEOUT_SECONDS,
                                         cwd=process_cwd, env=child_environment)
                process_record = {"returncode": process.returncode, "stdout": process.stdout,
                                  "stderr": process.stderr, "command": command, "cwd": process_cwd,
                                  "environment": recorded_environment, "timeout_diagnosis": None}
                processes.append(process_record)
                if process.returncode != 0:
                    raise CorpusError(f"runner exited {process.returncode}: {process.stderr.strip()}")
                result = json.loads(process.stdout, object_pairs_hook=strict_object)
                if not isinstance(result, dict):
                    raise CorpusError("runner output must be one JSON object")
                current_build, current_machine = result.get("build"), result.get("machine")
                validate_provenance(current_build, current_machine, "interpreter result")
                if runner_build is None:
                    runner_build, runner_machine = current_build, current_machine
                if current_build != runner_build or current_machine != runner_machine:
                    raise CorpusError("runner compiler/build/machine identity changed during corpus run")
                compare_result(result, raw, expected_by_id[identifier], "interpreter", runner_build,
                               runner_machine, f"{identifier} repeat {repeat + 1}")
                observations.append(result)
            except KeyboardInterrupt as error:
                if len(processes) == repeat:
                    processes.append({"returncode": None, "stdout": "", "stderr": "interrupted",
                                      "command": command, "cwd": process_cwd,
                                      "environment": recorded_environment, "timeout_diagnosis": None})
                message = f"{identifier} repeat {repeat + 1}: interrupted"
                failures.append(message)
                observations.append({"failure": message, "returncode": processes[-1]["returncode"]})
                row.update({"repeats": observations, "processes": processes})
                write_run_checkpoint(directory, results_path, manifest, rows, failures, identity,
                                     execution_machine, runner_build, runner_machine)
                raise CorpusError(f"interpreter corpus interrupted after {len(rows)} case(s); checkpoint: {results_path}") from error
            except subprocess.TimeoutExpired as error:
                message = str(error)
                processes.append({
                    "returncode": None,
                    "stdout": output_text(error.stdout),
                    "stderr": output_text(error.stderr),
                    "command": command,
                    "cwd": process_cwd,
                    "environment": recorded_environment,
                    "timeout_diagnosis": {
                        "kind": "timeout_expired",
                        "timeout_seconds": RUNNER_TIMEOUT_SECONDS,
                        "message": message,
                    },
                })
                failures.append(f"{identifier} repeat {repeat + 1}: timed out after {RUNNER_TIMEOUT_SECONDS}s: {message}")
                observations.append({"failure": message, "returncode": None})
            except (CorpusError, json.JSONDecodeError, OSError, UnicodeError, subprocess.SubprocessError) as error:
                if len(processes) == repeat:
                    processes.append({"returncode": None, "stdout": "", "stderr": str(error),
                                      "command": command, "cwd": process_cwd,
                                      "environment": recorded_environment, "timeout_diagnosis": None})
                failures.append(f"{identifier} repeat {repeat + 1}: {error}")
                observations.append({"failure": str(error), "returncode": processes[-1]["returncode"]})
            row.update({"repeats": observations, "processes": processes})
        if len(observations) == 2 and all("failure" not in row for row in observations) and observations[0] != observations[1]:
            failures.append(f"{identifier}: repeat results differ")
        write_run_checkpoint(directory, results_path, manifest, rows, failures, identity,
                             execution_machine, runner_build, runner_machine)
    summary = {
        "schema_version": 1, "status": "failed" if failures else "passed", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "manifest_sha256": sha256((directory / MANIFEST_NAME).read_bytes()), "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "execution_identity": identity,
        "execution_machine": execution_machine, "runner_build": runner_build,
        "runner_machine": runner_machine, "results": rows, "failures": failures,
        "missing_attempts": missing_attempts(expected_case_ids(), rows),
    }
    results_path.parent.mkdir(parents=True, exist_ok=True)
    results_path.write_bytes(json_bytes(summary))
    if failures:
        raise CorpusError(f"{len(failures)} local interpreter failure(s); summary: {results_path}")
    return summary


def verify_results(directory: Path, results_path: Path, executable: Path = ROOT / "target/debug/od-runner") -> None:
    manifest = verify_corpus(directory)
    result = load_json(results_path)
    if (not isinstance(result, dict) or result.get("schema_version") != 1 or result.get("status") != "passed"
            or result.get("backend") != "interpreter" or result.get("profile") != PROFILE
            or result.get("native_x86") != "not_run_form_not_admitted" or result.get("failures") != []
            or result.get("missing_attempts") != []):
        raise CorpusError("saved evidence is not a successful local-only interpreter record")
    if result.get("manifest_sha256") != sha256((directory / MANIFEST_NAME).read_bytes()):
        raise CorpusError("saved evidence is not bound to the exact corpus manifest")
    if result.get("case_count") != TOTAL_COUNT or result.get("repeats_per_case") != REPEATS:
        raise CorpusError("saved evidence case/repeat count differs")
    build, machine = result.get("runner_build"), result.get("runner_machine")
    validate_provenance(build, machine, "saved interpreter evidence")
    execution_identity, execution_machine = result.get("execution_identity"), result.get("execution_machine")
    if (not isinstance(execution_identity, dict) or set(execution_identity) != {"executable_sha256", "revision"}
            or not isinstance(execution_identity.get("executable_sha256"), str)
            or len(execution_identity["executable_sha256"]) != 64
            or any(char not in "0123456789abcdef" for char in execution_identity["executable_sha256"])
            or not isinstance(execution_identity.get("revision"), str) or not execution_identity["revision"]):
        raise CorpusError("saved evidence lacks executable/source identity")
    selected_executable = executable.resolve(strict=True)
    if not selected_executable.is_file() or sha256(selected_executable.read_bytes()) != execution_identity["executable_sha256"]:
        raise CorpusError("saved evidence executable path/hash differs from the selected runner")
    if (not isinstance(execution_machine, dict) or any(not isinstance(execution_machine.get(k), str) or not execution_machine[k]
                                                       for k in ("host_os", "host_arch"))):
        raise CorpusError("saved evidence lacks execution machine identity")
    if (execution_identity["revision"] != build.get("revision")
            or any(execution_machine[k] != machine.get(k) for k in ("host_os", "host_arch"))):
        raise CorpusError("saved executable/source/machine provenance differs from runner results")
    rows = result.get("results")
    ordered = [item["case_id"] for item in manifest["case_digests"]]
    if not isinstance(rows, list) or [row.get("case_id") if isinstance(row, dict) else None for row in rows] != ordered:
        raise CorpusError("saved evidence case order/count differs")
    expected_by_id = {item["case_id"]: item for item in load_json(directory / EXPECTED_NAME)["cases"]}
    recorded_environment: dict[str, str | None] | None = None
    for row in rows:
        identifier = row["case_id"]
        raw = (directory / f"{identifier}.case.json").read_bytes()
        observations, processes = row.get("repeats"), row.get("processes")
        if (row.get("fixture_sha256") != sha256(raw) or not isinstance(observations, list)
                or len(observations) != REPEATS or not isinstance(processes, list) or len(processes) != REPEATS):
            raise CorpusError(f"saved evidence {identifier}: fixture digest, repeat, or process count differs")
        for index, (observed, process) in enumerate(zip(observations, processes), 1):
            if (not isinstance(process, dict) or set(process) != {"returncode", "stdout", "stderr", "command", "cwd", "environment", "timeout_diagnosis"}
                    or type(process.get("returncode")) is not int or process["returncode"] != 0
                    or not isinstance(process.get("stdout"), str) or not isinstance(process.get("stderr"), str)):
                raise CorpusError(f"saved evidence {identifier} repeat {index}: process record failed or missing")
            if process.get("timeout_diagnosis") is not None:
                raise CorpusError(f"saved evidence {identifier} repeat {index}: successful record contains timeout diagnosis")
            fixture_path = (directory / f"{identifier}.case.json").resolve(strict=True)
            expected_command = [str(selected_executable), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"]
            if process.get("command") != expected_command:
                raise CorpusError(f"saved evidence {identifier} repeat {index}: replay argv is empty or differs from the selected interpreter, fixture, or runner")
            if process.get("cwd") != str(ROOT.resolve()):
                raise CorpusError(f"saved evidence {identifier} repeat {index}: process working directory differs")
            environment = process.get("environment")
            if (not isinstance(environment, dict) or set(environment) != set(PROCESS_ENV_KEYS)
                    or any(value is not None and not isinstance(value, str) for value in environment.values())):
                raise CorpusError(f"saved evidence {identifier} repeat {index}: relevant process environment is missing or malformed")
            if recorded_environment is None:
                recorded_environment = environment
            elif environment != recorded_environment:
                raise CorpusError(f"saved evidence {identifier} repeat {index}: process environment changed during replay")
            if not isinstance(observed, dict) or "failure" in observed:
                raise CorpusError(f"saved evidence {identifier} repeat {index} contains a failure")
            try:
                parsed_stdout = json.loads(process.get("stdout", ""), object_pairs_hook=strict_object)
            except (json.JSONDecodeError, CorpusError) as error:
                raise CorpusError(f"saved evidence {identifier} repeat {index}: frozen process output is invalid: {error}") from error
            if parsed_stdout != observed:
                raise CorpusError(f"saved evidence {identifier} repeat {index}: frozen process/result differ")
            compare_result(observed, raw, expected_by_id[identifier], "interpreter", build, machine,
                           f"saved {identifier} repeat {index}")
        if observations[0] != observations[1]:
            raise CorpusError(f"saved evidence {identifier}: repeat results differ")


def write_native_blocked_summary(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    summary = {
        "schema_version": 1, "status": "blocked", "profile": PROFILE,
        "native_x86": "not_run_form_not_admitted",
        "blocked_reason": "S2-42u explicitly denies native execution; CMPXCHG memory corpus is not admitted to the native backend.",
        "backend_execution": "not_started",
    }
    path.write_bytes(json_bytes(summary))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_mutually_exclusive_group(required=True)
    actions.add_argument("--generate", action="store_true")
    actions.add_argument("--verify", action="store_true")
    actions.add_argument("--run-interpreter", action="store_true")
    actions.add_argument("--verify-results", action="store_true")
    actions.add_argument("--native", action="store_true")
    parser.add_argument("--corpus", type=Path, default=CORPUS_DIR)
    parser.add_argument("--results", type=Path)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner")
    args = parser.parse_args(argv)
    try:
        if args.generate:
            write_file_set(args.corpus, materialize())
            print(f"generated {TOTAL_COUNT} S2-42u fixtures")
        elif args.verify:
            manifest = verify_corpus(args.corpus)
            print(f"verified {TOTAL_COUNT} exact S2-42u fixtures; digest {manifest['corpus_digest']}")
        elif args.run_interpreter:
            results_path = args.results or RESULTS_PATH
            write_incomplete_summary(results_path)
            try:
                summary = run_interpreter(args.corpus, results_path, args.runner)
            except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError) as error:
                write_failed_summary(results_path, error)
                raise
            print(f"passed {TOTAL_COUNT} cases × {REPEATS} local interpreter runs; native_x86=not_run_form_not_admitted")
            print(f"saved local evidence: {results_path}")
        elif args.verify_results:
            results_path = args.results or RESULTS_PATH
            verify_results(args.corpus, results_path, args.runner)
            print(f"verified saved local interpreter results: {results_path}")
        else:
            summary_path = args.results or NATIVE_BLOCKED_PATH
            write_native_blocked_summary(summary_path)
            print(json.dumps(load_json(summary_path), indent=2))
            return 1
    except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError) as error:
        print(f"S2-42u failed closed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
