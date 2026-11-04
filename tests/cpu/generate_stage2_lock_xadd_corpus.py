#!/usr/bin/env python3
"""Build, run, and verify the bounded S2-42t LOCK XADD corpus."""

from __future__ import annotations

import argparse
import json
import platform
import random
import subprocess
import sys
from pathlib import Path
from typing import Any

from stage2_corpus_support import CorpusError, json_bytes, sha256, strict_object, verify_file_set, write_file_set


ROOT = Path(__file__).resolve().parents[2]
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42t_lock_xadd_controls"
CONTROL_EXPECTED = CONTROL_DIR / "expected.json"
CONTROL_MANIFEST = CONTROL_DIR / "controls_manifest.json"
CORPUS_DIR = ROOT / "work/stage2/s2-42t-lock-xadd-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42t-lock-xadd-corpus/interpreter-results.json"
NATIVE_BLOCKED_PATH = ROOT / "work/stage2/s2-42t-lock-xadd-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42t-lock-xadd-memory-v1"
CODE_ADDRESS = 0x00100000
DATA_ADDRESS = 0x00200000
CODE = bytes.fromhex("f0480fc103")  # LOCK XADD qword [RBX], RAX
DATA_LENGTH = 128
OFFSETS = (1, 120)
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
REPEATS = 2
RFLAGS_DEFINED = 0x0CD7
ARITHMETIC_FLAGS = 0x08D5
MASK64 = 0xFFFFFFFFFFFFFFFF
GENERATED_COUNT = len(SEEDS) * CASES_PER_SEED
CONTROL_COUNT = 32
TOTAL_COUNT = GENERATED_COUNT + CONTROL_COUNT
MANIFEST_NAME = "manifest.json"
EXPECTED_NAME = "expected.json"
MANIFEST_FIELDS = {
    "manifest_version", "generator_version", "profile", "seeds", "cases_per_seed",
    "control_count", "generated_case_count", "case_count", "case_digests",
    "control_manifest_sha256", "expected_sha256", "corpus_digest",
}
CONTROL_MANIFEST_SHA256 = "8af0b95cd67002bf69c748e90f3b81ffa1f6c58372addcf7e1976f2a24cc9367"


CONTROL_PAIRS = (
    (0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0446),
    (0x0000000000000000, 0x0000000000000001, 0x0000000000000001, 0x0402),
    (0x0000000000000001, 0x0000000000000000, 0x0000000000000001, 0x0402),
    (0xFFFFFFFFFFFFFFFF, 0x0000000000000001, 0x0000000000000000, 0x0457),
    (0x0000000000000001, 0xFFFFFFFFFFFFFFFF, 0x0000000000000000, 0x0457),
    (0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFE, 0x0493),
    (0x7FFFFFFFFFFFFFFF, 0x0000000000000001, 0x8000000000000000, 0x0C96),
    (0x8000000000000000, 0xFFFFFFFFFFFFFFFF, 0x7FFFFFFFFFFFFFFF, 0x0C07),
    (0x8000000000000000, 0x8000000000000000, 0x0000000000000000, 0x0C47),
    (0x7FFFFFFFFFFFFFFF, 0x7FFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFE, 0x0C92),
    (0x000000000000000F, 0x0000000000000001, 0x0000000000000010, 0x0412),
    (0x0000000000000010, 0xFFFFFFFFFFFFFFFF, 0x000000000000000F, 0x0407),
    (0x00000000000000FF, 0x0000000000000001, 0x0000000000000100, 0x0416),
    (0x0000000000000100, 0xFFFFFFFFFFFFFFFF, 0x00000000000000FF, 0x0407),
    (0xAAAAAAAAAAAAAAAA, 0x5555555555555555, 0xFFFFFFFFFFFFFFFF, 0x0486),
    (0x0123456789ABCDEF, 0xFEDCBA9876543210, 0xFFFFFFFFFFFFFFFF, 0x0486),
)


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def control_id(pair: int, offset: int) -> str:
    return f"s2_42t_lock_xadd_c{pair:02d}_{'u1' if offset == 1 else 'a120'}"


def generated_id(seed_index: int, sequence: int) -> str:
    return f"s2_42t_lock_xadd_g{seed_index}_{sequence:03d}"


def gprs_for(source: int, address: int, values: list[int]) -> list[int]:
    gprs = list(values)
    gprs[0] = source
    gprs[3] = address
    return gprs


def make_fixture(identifier: str, source: int, offset: int, memory_value: int,
                 gprs: list[int], flags: int, kind: str, seed: int | None = None,
                 sequence: int | None = None) -> dict[str, Any]:
    if offset not in OFFSETS or len(gprs) != 16 or any(type(v) is not int or not 0 <= v <= MASK64 for v in gprs):
        raise CorpusError("LOCK XADD input is outside the exact S2-42t profile")
    if gprs[0] != source or gprs[3] != DATA_ADDRESS + offset:
        raise CorpusError("LOCK XADD RAX/RBX inputs do not match source and effective address")
    if flags & ~RFLAGS_DEFINED or not flags & 2:
        raise CorpusError("initial RFLAGS contain reserved bits or omit the fixed bit 1")
    data = bytearray(b"\xa5" * DATA_LENGTH)
    data[offset:offset + 8] = memory_value.to_bytes(8, "little")
    return {
        "schema_version": 2,
        "case_id": identifier,
        "profile": PROFILE,
        "provenance": {
            "kind": "project_fixture",
            "source": ("S2-42t handwritten LOCK XADD boundary/control matrix." if kind == "handwritten"
                       else f"S2-42t deterministic private-RAM corpus seed=0x{seed:016x}, sequence={sequence}."),
            "specification": "Intel SDM revision 093, Vol. 2D XADD and Vol. 2A LOCK prefix; bounded project S2-42t profile.",
            "specification_version": "Intel SDM revision 093; project S2-42t",
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
            {"region_id": "data", "guest_address": hx(DATA_ADDRESS), "length": DATA_LENGTH,
             "permissions": {"read": True, "write": True, "execute": False},
             "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * DATA_LENGTH},
        ],
        "stop_rip": hx(CODE_ADDRESS + len(CODE)),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(RFLAGS_DEFINED),
        "expected_exit": "stop_address",
    }


def control_inputs() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    for pair, (memory_value, source, _sum, _flags) in enumerate(CONTROL_PAIRS):
        for offset in OFFSETS:
            address = DATA_ADDRESS + offset
            base = [0x0123456789ABCDEF, 0x1122334455667788, 0xDEADBEEFFEDCBA98,
                    address, 0x5566778899AABBCC, 0x8877665544332211,
                    0x0000000000200004, 0x0000000000200010,
                    0xAABBCCDDEEFF0011, 0xFEDCBA9876543210,
                    0x0123456789ABCDEF, 0x9988776655443322,
                    0x1122112211221122, 0x3344334433443344,
                    0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
            fixture = make_fixture(control_id(pair, offset), source, offset, memory_value,
                                   gprs_for(source, address, base), RFLAGS_DEFINED, "handwritten")
            files[f"{fixture['case_id']}.case.json"] = json_bytes(fixture)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    fixtures = []
    input_states: set[tuple[tuple[str, ...], str, str, str]] = set()
    for seed_index, seed in enumerate(SEEDS):
        for sequence in range(CASES_PER_SEED):
            rng = random.Random(seed ^ ((sequence + 1) * 0x9E3779B97F4A7C15))
            offset = OFFSETS[sequence % 2]
            memory_value = rng.getrandbits(64)
            source = rng.getrandbits(64)
            values = [rng.getrandbits(64) for _ in range(16)]
            values[15] = 0x4274000000000000 | (seed_index << 32) | sequence
            gprs = gprs_for(source, DATA_ADDRESS + offset, values)
            flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
            fixture = make_fixture(generated_id(seed_index, sequence), source, offset,
                                   memory_value, gprs, flags, "generated", seed, sequence)
            state = fixture["initial_state"]
            input_states.add((tuple(state["gpr"]), state["rip"], state["rflags"],
                              fixture["memory"][1]["initial_bytes_hex"]))
            fixtures.append(fixture)
    if len(input_states) != GENERATED_COUNT:
        raise CorpusError("generated architectural input states are not unique")
    return fixtures


def arithmetic_flags(memory_value: int, source: int, result: int) -> int:
    wide = memory_value + source
    carry = int(wide > MASK64)
    overflow = ((~(memory_value ^ source) & (memory_value ^ result)) >> 63) & 1
    auxiliary = ((memory_value ^ source ^ result) >> 4) & 1
    sign = (result >> 63) & 1
    zero = int(result == 0)
    parity = int((result & 0xFF).bit_count() % 2 == 0)
    return carry | (parity << 2) | (auxiliary << 4) | (zero << 6) | (sign << 7) | (overflow << 11)


def expected_snapshot(fixture: dict[str, Any]) -> dict[str, Any]:
    """Integer-only oracle; independent of decoder and production arithmetic helpers."""
    initial = fixture["initial_state"]
    code = fixture["memory"][0]
    data_region = fixture["memory"][1]
    if bytes.fromhex(code["initial_bytes_hex"]) != CODE:
        raise CorpusError("S2-42t oracle requires the exact five-byte LOCK XADD encoding")
    gprs = [int(value, 16) for value in initial["gpr"]]
    base = int(data_region["guest_address"], 16)
    effective = gprs[3]
    offset = effective - base
    if offset not in OFFSETS or effective & MASK64 != base + offset:
        raise CorpusError("S2-42t effective address is outside its fixed control offset set")
    data = bytearray.fromhex(data_region["initial_bytes_hex"])
    memory_value = int.from_bytes(data[offset:offset + 8], "little")
    source = gprs[0]
    result = (memory_value + source) & MASK64
    flags = int(initial["rflags"], 16)
    final_flags = (flags & ~ARITHMETIC_FLAGS) | arithmetic_flags(memory_value, source, result)
    gprs[0] = memory_value
    data[offset:offset + 8] = result.to_bytes(8, "little")
    return {
        "case_id": fixture["case_id"],
        "state": {"mode": "long64", "gpr": [hx(value) for value in gprs],
                  "rip": hx(CODE_ADDRESS + len(CODE)), "rflags": hx(final_flags),
                  "rflags_defined": hx(RFLAGS_DEFINED)},
        "memory": [
            {"region_id": "code", "guest_address": hx(CODE_ADDRESS), "bytes_hex": CODE.hex()},
            {"region_id": "data", "guest_address": hx(DATA_ADDRESS), "bytes_hex": data.hex()},
        ],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": hx(1)},
        "exception": None,
    }


def load_controls() -> tuple[list[tuple[str, bytes, dict[str, Any]]], bytes, dict[str, Any]]:
    if not CONTROL_DIR.is_dir() or CONTROL_DIR.is_symlink():
        raise CorpusError("S2-42t control directory is missing or unsafe")
    if not CONTROL_EXPECTED.is_file() or CONTROL_EXPECTED.is_symlink() or not CONTROL_MANIFEST.is_file() or CONTROL_MANIFEST.is_symlink():
        raise CorpusError("S2-42t static control snapshots or manifest are missing")
    manifest_raw = CONTROL_MANIFEST.read_bytes()
    if not CONTROL_MANIFEST_SHA256 or sha256(manifest_raw) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned handwritten control-manifest digest changed")
    manifest = json.loads(manifest_raw, object_pairs_hook=strict_object)
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
            or manifest.get("schema_version") != 1 or manifest.get("profile") != PROFILE
            or manifest.get("control_count") != CONTROL_COUNT or not isinstance(manifest.get("fixtures"), list)):
        raise CorpusError("S2-42t static control manifest has invalid shape")
    expected_raw = CONTROL_EXPECTED.read_bytes()
    if sha256(expected_raw) != manifest.get("expected_sha256"):
        raise CorpusError("S2-42t static control snapshots digest differs from pinned manifest")
    expected_doc = json.loads(expected_raw, object_pairs_hook=strict_object)
    if not isinstance(expected_doc, dict) or set(expected_doc) != {"profile", "cases"} or expected_doc["profile"] != PROFILE:
        raise CorpusError("S2-42t static control snapshot document has invalid shape")
    expected_cases = expected_doc["cases"]
    if not isinstance(expected_cases, list) or len(expected_cases) != CONTROL_COUNT:
        raise CorpusError("S2-42t static control snapshot count differs")
    expected_order = [control_id(pair, offset) for pair in range(16) for offset in OFFSETS]
    if [case.get("case_id") if isinstance(case, dict) else None for case in expected_cases] != expected_order:
        raise CorpusError("S2-42t static full snapshots are missing, duplicated, or out of order")
    expected_by_id = {case.get("case_id"): case for case in expected_cases if isinstance(case, dict)}
    if len(expected_by_id) != CONTROL_COUNT:
        raise CorpusError("S2-42t static control snapshots have duplicate or missing identities")
    rows = manifest["fixtures"]
    if len(rows) != CONTROL_COUNT:
        raise CorpusError("S2-42t control manifest fixture count differs")
    controls: list[tuple[str, bytes, dict[str, Any]]] = []
    expected_names: set[str] = {"controls_manifest.json", "expected.json"}
    for pair in range(16):
        for offset in OFFSETS:
            identifier = control_id(pair, offset)
            name = f"{identifier}.case.json"
            expected_names.add(name)
            row = rows[len(controls)]
            if not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "expected_sha256"} or row["case_id"] != identifier:
                raise CorpusError("S2-42t pinned controls are missing, extra, duplicated, or out of order")
            path = CONTROL_DIR / name
            if path.is_symlink() or not path.is_file():
                raise CorpusError(f"S2-42t control fixture is missing or unsafe: {identifier}")
            raw = path.read_bytes()
            if sha256(raw) != row["fixture_sha256"]:
                raise CorpusError(f"S2-42t control fixture digest mismatch: {identifier}")
            if raw != control_inputs()[name]:
                raise CorpusError(f"S2-42t control fixture differs from its fixed input table: {identifier}")
            fixture = json.loads(raw, object_pairs_hook=strict_object)
            expected = expected_by_id.get(identifier)
            if not isinstance(expected, dict) or sha256(json_bytes(expected)) != row["expected_sha256"]:
                raise CorpusError(f"S2-42t static full snapshot digest mismatch: {identifier}")
            if expected.get("case_id") != identifier:
                raise CorpusError(f"S2-42t static snapshot identity mismatch: {identifier}")
            controls.append((identifier, raw, expected))
    entries = list(CONTROL_DIR.iterdir())
    if any(p.is_symlink() or not p.is_file() for p in entries) or {p.name for p in entries} != expected_names:
        raise CorpusError("S2-42t controls directory has missing, extra, or symlink entries")
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
        raise CorpusError("S2-42t corpus directory is missing or unsafe")
    files = materialize()
    verify_file_set(directory, files)
    manifest = load_json(directory / MANIFEST_NAME)
    if not isinstance(manifest, dict) or set(manifest) != MANIFEST_FIELDS:
        raise CorpusError("manifest fields do not match the S2-42t contract")
    if manifest.get("case_count") != TOTAL_COUNT or len(manifest.get("case_digests", [])) != TOTAL_COUNT:
        raise CorpusError("S2-42t manifest case accounting is invalid")
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
            raise CorpusError(f"{label}: {field} differs from exact S2-42t identity/oracle")
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


def write_incomplete_summary(results_path: Path) -> None:
    results_path.parent.mkdir(parents=True, exist_ok=True)
    results_path.write_bytes(json_bytes({
        "schema_version": 1, "status": "incomplete", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS,
        "results": [], "failures": ["run is in progress or preflight did not complete"],
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
    for item in manifest["case_digests"]:
        identifier = item["case_id"]
        fixture_path = (directory / f"{identifier}.case.json").resolve(strict=True)
        raw = fixture_path.read_bytes()
        observations = []
        processes = []
        for repeat in range(REPEATS):
            command = [str(executable.resolve(strict=True)), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"]
            try:
                process = subprocess.run(command, capture_output=True, text=True, check=False, timeout=12)
                process_record = {"returncode": process.returncode, "stdout": process.stdout,
                                  "stderr": process.stderr, "command": command}
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
            except (CorpusError, json.JSONDecodeError, OSError, UnicodeError, subprocess.SubprocessError) as error:
                if len(processes) == repeat:
                    processes.append({"returncode": None, "stdout": "", "stderr": str(error), "command": command})
                failures.append(f"{identifier} repeat {repeat + 1}: {error}")
                observations.append({"failure": str(error), "returncode": processes[-1]["returncode"]})
        if len(observations) == 2 and all("failure" not in row for row in observations) and observations[0] != observations[1]:
            failures.append(f"{identifier}: repeat results differ")
        rows.append({"case_id": identifier, "fixture_sha256": sha256(raw),
                     "repeats": observations, "processes": processes})
    summary = {
        "schema_version": 1, "status": "failed" if failures else "passed", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "manifest_sha256": sha256((directory / MANIFEST_NAME).read_bytes()), "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "execution_identity": identity,
        "execution_machine": execution_machine, "runner_build": runner_build,
        "runner_machine": runner_machine, "results": rows, "failures": failures,
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
            or result.get("native_x86") != "not_run_form_not_admitted" or result.get("failures") != []):
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
    for row in rows:
        identifier = row["case_id"]
        raw = (directory / f"{identifier}.case.json").read_bytes()
        observations, processes = row.get("repeats"), row.get("processes")
        if (row.get("fixture_sha256") != sha256(raw) or not isinstance(observations, list)
                or len(observations) != REPEATS or not isinstance(processes, list) or len(processes) != REPEATS):
            raise CorpusError(f"saved evidence {identifier}: fixture digest, repeat, or process count differs")
        for index, (observed, process) in enumerate(zip(observations, processes), 1):
            if (not isinstance(process, dict) or set(process) != {"returncode", "stdout", "stderr", "command"}
                    or type(process.get("returncode")) is not int or process["returncode"] != 0
                    or not isinstance(process.get("stdout"), str) or not isinstance(process.get("stderr"), str)):
                raise CorpusError(f"saved evidence {identifier} repeat {index}: process record failed or missing")
            fixture_path = (directory / f"{identifier}.case.json").resolve(strict=True)
            expected_command = [str(selected_executable), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"]
            if process.get("command") != expected_command:
                raise CorpusError(f"saved evidence {identifier} repeat {index}: replay argv is empty or differs from the selected interpreter, fixture, or runner")
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
        "blocked_reason": "S2-42t explicitly denies native execution; LOCK XADD memory corpus is not admitted to the native backend.",
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
            print(f"generated {TOTAL_COUNT} S2-42t fixtures")
        elif args.verify:
            manifest = verify_corpus(args.corpus)
            print(f"verified {TOTAL_COUNT} exact S2-42t fixtures; digest {manifest['corpus_digest']}")
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
        print(f"S2-42t failed closed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
