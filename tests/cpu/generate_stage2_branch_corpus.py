#!/usr/bin/env python3
"""Generate, execute, and verify the deterministic S2-42r Jcc rel8 corpus."""

from __future__ import annotations

import argparse
import copy
import json
import platform
import random
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

from stage2_corpus_support import CorpusError, json_bytes, sha256, strict_object, verify_file_set, write_file_set


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CORPUS = ROOT / "work/stage2/s2-42r-branch-corpus/corpus"
DEFAULT_RESULTS = ROOT / "work/stage2/s2-42r-branch-corpus/interpreter-results.json"
DEFAULT_ORACLE_ROOT = ROOT / "work/stage2-42r-oracle"
CONTROL_FIXTURE_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42r_branch_controls"
CONTROL_EXPECTED_PATH = CONTROL_FIXTURE_DIR / "expected.json"
CONTROL_MANIFEST_PATH = CONTROL_FIXTURE_DIR / "controls_manifest.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42r-jcc-rel8-varied-input-v2"
SEEDS = (0x42_0000_0001, 0x42_0000_0002, 0x42_0000_0003, 0x42_0000_0004)
CASES_PER_SEED = 256
REPEATS = 2
CODE_ADDRESS = 0x00100000
CODE_LENGTH = 512
ENTRY_OFFSETS = (128, 192)
DISPLACEMENTS = (-128, -127, -16, -3, 1, 16, 126, 127)
OPCODES = (0x74, 0x75)
RFLAGS_DEFINED = 0x0CD7
BASE_RFLAGS = 0x0000000000000002
RUNNER_BUILD_PROFILES = frozenset(("debug", "release"))
GENERATED_COUNT = len(SEEDS) * CASES_PER_SEED
HANDWRITTEN_COUNT = len(OPCODES) * 2 * len(DISPLACEMENTS)
TOTAL_COUNT = GENERATED_COUNT + HANDWRITTEN_COUNT
MANIFEST_NAME = "manifest.json"
EXPECTED_NAME = "expected.json"
MANIFEST_FIELDS = {
    "manifest_version", "generator_version", "profile", "seeds", "cases_per_seed",
    "entry_offsets", "displacements", "generated_state_variation", "generated_case_count", "handwritten_case_count",
    "case_count", "case_digests", "expected_sha256", "matrix_repetitions", "corpus_digest",
}
# Pins the complete checked-in expected-state document for all literal controls.
CONTROL_EXPECTED_DOCUMENT_SHA256 = "de6b45d8f7009bab0f5691680a89ace57ed107772236348e0b39749a9164a8f0"
CONTROL_MANIFEST_SHA256 = "ad1ce3c54d42f647c229ff6440fe30bf7cabd1ec03683bd7c76bf7d0ac527ed9"


def hx(value: int) -> str:
    return f"0x{value & 0xFFFFFFFFFFFFFFFF:016x}"


def load_json(path: Path) -> Any:
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def case_id(seed: int, sequence: int) -> str:
    return f"s2_42r_branch_{seed:016x}_{sequence:04x}"


def control_id(opcode: int, zf: bool, displacement: int) -> str:
    disp_id = f"p{displacement:03d}" if displacement >= 0 else f"m{-displacement:03d}"
    return f"s2_42r_hand_{opcode:02x}_zf{int(zf)}_{disp_id}"


def _fixture(identifier: str, opcode: int, zf: bool, displacement: int,
             entry_offset: int, seed: int | None, sequence: int | None,
             kind: str, gpr_values: list[int] | None = None,
             initial_rflags: int | None = None) -> dict[str, Any]:
    if opcode not in OPCODES or displacement not in DISPLACEMENTS or entry_offset not in ENTRY_OFFSETS:
        raise CorpusError("Jcc fixture input is outside the admitted matrix")
    code = bytearray(b"\x90" * CODE_LENGTH)
    code[entry_offset:entry_offset + 2] = bytes((opcode, displacement & 0xFF))
    entry = CODE_ADDRESS + entry_offset
    initial_flags = BASE_RFLAGS | ((1 << 6) if zf else 0) if initial_rflags is None else initial_rflags
    if bool(initial_flags & (1 << 6)) != zf or initial_flags & ~RFLAGS_DEFINED:
        raise CorpusError("initial RFLAGS contain an invalid or mismatched ZF value")
    zf_set = bool(initial_flags & (1 << 6))
    taken = (opcode == 0x74 and zf_set) or (opcode == 0x75 and not zf_set)
    next_rip = entry + 2
    stop_rip = next_rip + displacement if taken else next_rip
    default_gprs = [
        0x1122334455667788, 0x0000000000000001, 0xDEADBEEFFEDCBA98, 0x0000000000200000,
        0x00000000003000F8, 0x0000000000300080, 0x0123456789ABCDEF, 0x0102030405060708,
        0x0123456789ABCDEF, 0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
        0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF,
    ]
    gprs = default_gprs if gpr_values is None else gpr_values
    if len(gprs) != 16 or any(not 0 <= value <= 0xFFFFFFFFFFFFFFFF for value in gprs):
        raise CorpusError("initial GPR vector is malformed")
    return {
        "schema_version": 2,
        "case_id": identifier,
        "profile": PROFILE,
        "provenance": {
            "kind": "project_fixture",
            "source": ("S2-42r literal JZ/JNZ rel8 control table." if kind == "handwritten"
                       else f"S2-42r deterministic branch matrix seed=0x{seed:016x}, sequence={sequence}."),
            "specification": "Intel 64 and IA-32 Architectures Software Developer’s Manual, Rev. 093, Vol. 2A: Jcc short relative.",
            "specification_version": "Intel SDM revision 093; project S2-42r",
        },
        "seed": None if seed is None else hx(seed),
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in gprs],
            "rip": hx(entry),
            "rflags": hx(initial_flags),
        },
        "memory": [{
            "region_id": "code", "guest_address": hx(CODE_ADDRESS), "length": CODE_LENGTH,
            "permissions": {"read": True, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * CODE_LENGTH,
        }],
        "stop_rip": hx(stop_rip),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(RFLAGS_DEFINED),
        "expected_exit": "stop_address",
    }


def matrix_rows(seed: int) -> list[tuple[int, bool, int, int]]:
    # Each of the 32 opcode × ZF × rel8 cells appears exactly eight times per seed.
    cells = [(opcode, zf, displacement) for opcode in OPCODES for zf in (False, True)
             for displacement in DISPLACEMENTS]
    rows = cells * 8
    random.Random(seed).shuffle(rows)
    rng = random.Random(seed ^ 0x9E3779B97F4A7C15)
    return [(opcode, zf, displacement, rng.choice(ENTRY_OFFSETS))
            for opcode, zf, displacement in rows]


def generated_cpu_state(seed: int, sequence: int, zf: bool) -> tuple[list[int], int]:
    """Vary all GPRs and defined non-ZF flags while preserving the cell's ZF input."""
    seed_index = SEEDS.index(seed)
    rng = random.Random(seed ^ 0xD1B54A32D192ED03 ^ (sequence * 0x9E3779B97F4A7C15))
    gprs = [rng.getrandbits(64) for _ in range(16)]
    # A monotonic tag makes CPU input uniqueness deterministic, not probabilistic.
    gprs[0] = 0x4200000000000000 | (seed_index * CASES_PER_SEED + sequence + 1)
    variable_flags = RFLAGS_DEFINED & ~((1 << 1) | (1 << 6))
    flags = BASE_RFLAGS | ((1 << 6) if zf else 0) | (rng.getrandbits(64) & variable_flags)
    return gprs, flags


def expected_snapshot(fixture: dict[str, Any]) -> dict[str, Any]:
    """Independent Jcc oracle: inspect literal opcode and ZF bit, then sign-extend rel8."""
    initial = fixture["initial_state"]
    code = bytes.fromhex(fixture["memory"][0]["initial_bytes_hex"])
    entry = int(initial["rip"], 16)
    offset = entry - int(fixture["memory"][0]["guest_address"], 16)
    opcode, raw_disp = code[offset], code[offset + 1]
    if opcode not in (0x74, 0x75):
        raise CorpusError("expected-state oracle saw an unadmitted opcode")
    zf_set = bool(int(initial["rflags"], 16) & (1 << 6))
    taken = (opcode == 0x74 and zf_set) or (opcode == 0x75 and not zf_set)
    signed_disp = raw_disp if raw_disp < 0x80 else raw_disp - 0x100
    next_rip = entry + 2
    rip = next_rip + signed_disp if taken else next_rip
    if not 0 <= rip - int(fixture["memory"][0]["guest_address"], 16) < CODE_LENGTH:
        raise CorpusError("branch target escaped its code mapping")
    if offset <= rip - int(fixture["memory"][0]["guest_address"], 16) < offset + 2:
        raise CorpusError("branch target overlaps its own instruction")
    if fixture.get("stop_rip") != hx(rip):
        raise CorpusError("fixture stop_rip differs from independent branch calculation")
    return {
        "case_id": fixture["case_id"],
        "state": {"mode": "long64", "gpr": copy.deepcopy(initial["gpr"]), "rip": hx(rip),
                  "rflags": initial["rflags"], "rflags_defined": hx(RFLAGS_DEFINED)},
        "memory": [{"region_id": "code", "guest_address": hx(CODE_ADDRESS), "bytes_hex": code.hex()}],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": hx(1)},
        "exception": None,
    }


def materialize() -> dict[str, bytes]:
    fixtures: list[tuple[dict[str, Any], bytes, str, dict[str, Any], dict[str, Any] | None]] = []
    for seed in SEEDS:
        for sequence, (opcode, zf, displacement, entry_offset) in enumerate(matrix_rows(seed)):
            gprs, initial_flags = generated_cpu_state(seed, sequence, zf)
            fixture = _fixture(case_id(seed, sequence), opcode, zf, displacement, entry_offset,
                               seed, sequence, "generated", gprs, initial_flags)
            fixtures.append((fixture, json_bytes(fixture), "generated", {"opcode": f"0x{opcode:02x}", "zf": zf,
                "displacement": displacement, "entry_offset": entry_offset, "seed": hx(seed),
                "sequence": sequence}, None))
    # These source fixtures and snapshots are literal checked-in controls, independent of the matrix PRNG.
    control_manifest_raw = CONTROL_MANIFEST_PATH.read_bytes()
    if sha256(control_manifest_raw) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned handwritten fixture manifest digest changed")
    control_manifest = json.loads(control_manifest_raw, object_pairs_hook=strict_object)
    manifest_fields = {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
    if (not isinstance(control_manifest, dict) or set(control_manifest) != manifest_fields
            or type(control_manifest["schema_version"]) is not int or control_manifest["schema_version"] != 1
            or control_manifest["profile"] != PROFILE or control_manifest["control_count"] != HANDWRITTEN_COUNT
            or control_manifest["expected_sha256"] != CONTROL_EXPECTED_DOCUMENT_SHA256
            or not isinstance(control_manifest["fixtures"], list)):
        raise CorpusError("pinned handwritten fixture manifest schema or identity is invalid")
    source_fixture_hashes = {entry.get("case_id"): entry.get("fixture_sha256")
                             for entry in control_manifest["fixtures"] if isinstance(entry, dict)}
    expected_control_names = {f"{identifier}.case.json" for identifier in source_fixture_hashes}
    expected_control_names |= {CONTROL_EXPECTED_PATH.name, CONTROL_MANIFEST_PATH.name}
    directory_entries = list(CONTROL_FIXTURE_DIR.iterdir())
    if (any(path.is_symlink() or not path.is_file() for path in directory_entries)
            or {path.name for path in directory_entries} != expected_control_names
            or len(source_fixture_hashes) != HANDWRITTEN_COUNT):
        raise CorpusError("handwritten fixture directory has missing, extra, or unsafe entries")
    control_raw = CONTROL_EXPECTED_PATH.read_bytes()
    if sha256(control_raw) != CONTROL_EXPECTED_DOCUMENT_SHA256:
        raise CorpusError("pinned handwritten expected-snapshot document digest changed")
    control_document = json.loads(control_raw, object_pairs_hook=strict_object)
    if (not isinstance(control_document, dict) or set(control_document) != {"profile", "cases"}
            or control_document["profile"] != PROFILE or not isinstance(control_document["cases"], list)):
        raise CorpusError("pinned handwritten expected snapshots have an invalid schema")
    pinned_controls = {entry.get("case_id"): entry for entry in control_document["cases"]
                       if isinstance(entry, dict) and isinstance(entry.get("case_id"), str)}
    if len(pinned_controls) != HANDWRITTEN_COUNT:
        raise CorpusError("pinned handwritten expected snapshots are incomplete or duplicated")
    for opcode in OPCODES:
        for zf in (False, True):
            for displacement in DISPLACEMENTS:
                identifier = control_id(opcode, zf, displacement)
                fixture_path = CONTROL_FIXTURE_DIR / f"{identifier}.case.json"
                raw = fixture_path.read_bytes()
                if source_fixture_hashes.get(identifier) != sha256(raw):
                    raise CorpusError(f"pinned control fixture digest changed: {identifier}")
                fixture = json.loads(raw, object_pairs_hook=strict_object)
                entry_offset = int(fixture["initial_state"]["rip"], 16) - CODE_ADDRESS
                matrix = {"opcode": f"0x{opcode:02x}", "zf": zf,
                          "displacement": displacement, "entry_offset": entry_offset}
                if (fixture.get("case_id") != identifier or fixture.get("schema_version") != 2
                        or fixture.get("profile") != PROFILE or fixture.get("seed") is not None
                        or fixture.get("provenance", {}).get("kind") != "handwritten_control"
                        or entry_offset not in ENTRY_OFFSETS):
                    raise CorpusError(f"pinned control fixture identity is invalid: {identifier}")
                actual = expected_snapshot(fixture)
                pinned = pinned_controls.get(identifier)
                if pinned is None or actual != pinned:
                    raise CorpusError(f"literal control fixture differs from its pinned full snapshot: {identifier}")
                fixtures.append((fixture, raw, "handwritten", matrix, pinned))

    case_files: dict[str, bytes] = {}
    expected_cases: list[dict[str, Any]] = []
    digest_rows: list[dict[str, Any]] = []
    per_seed: dict[int, dict[tuple[int, bool, int], int]] = {seed: {} for seed in SEEDS}
    generated_input_states: set[tuple[tuple[str, ...], str, str]] = set()
    for fixture, raw, kind, matrix, pinned in fixtures:
        identifier = fixture["case_id"]
        case_files[f"{identifier}.case.json"] = raw
        wanted = pinned if pinned is not None else expected_snapshot(fixture)
        expected_cases.append(wanted)
        digest_rows.append({"case_id": identifier, "kind": kind, "fixture_sha256": sha256(raw),
                            "expected_sha256": sha256(json_bytes(wanted)), **matrix})
        if kind == "generated":
            seed = int(matrix["seed"], 16)
            cell = (int(matrix["opcode"], 16), matrix["zf"], matrix["displacement"])
            per_seed[seed][cell] = per_seed[seed].get(cell, 0) + 1
            state = fixture["initial_state"]
            generated_input_states.add((tuple(state["gpr"]), state["rflags"], state["rip"]))
    if any(len(counts) != 32 or set(counts.values()) != {8} for counts in per_seed.values()):
        raise CorpusError("generated seed did not cover each Jcc/ZF/rel8 cell eight times")
    if len(generated_input_states) != GENERATED_COUNT:
        raise CorpusError("generated CPU input states are not architecturally unique")

    expected_raw = json_bytes({"profile": PROFILE, "cases": expected_cases})
    control_ids = {entry["case_id"] for entry in expected_cases if entry["case_id"].startswith("s2_42r_hand_")}
    if control_ids != set(pinned_controls):
        raise CorpusError("pinned handwritten expected snapshots do not cover the literal controls")
    manifest = {
        "manifest_version": 1, "generator_version": GENERATOR_VERSION, "profile": PROFILE,
        "seeds": [hx(seed) for seed in SEEDS], "cases_per_seed": CASES_PER_SEED,
        "entry_offsets": list(ENTRY_OFFSETS), "displacements": list(DISPLACEMENTS),
        "generated_state_variation": {"gprs": "all-16-seeded", "non_zf_defined_flags": "seeded",
                                       "entry_offsets": list(ENTRY_OFFSETS)},
        "generated_case_count": GENERATED_COUNT, "handwritten_case_count": HANDWRITTEN_COUNT,
        "case_count": len(digest_rows), "case_digests": digest_rows,
        "expected_sha256": sha256(expected_raw), "matrix_repetitions": 8,
        "corpus_digest": sha256("\n".join(item["fixture_sha256"] for item in digest_rows).encode()),
    }
    files = dict(case_files)
    files[EXPECTED_NAME] = expected_raw
    files[MANIFEST_NAME] = json_bytes(manifest)
    return files


def verify_corpus(directory: Path) -> dict[str, Any]:
    files = materialize()
    verify_file_set(directory, files)
    manifest = load_json(directory / MANIFEST_NAME)
    if (not isinstance(manifest, dict) or set(manifest) != MANIFEST_FIELDS
            or not isinstance(manifest.get("case_digests"), list)):
        raise CorpusError("manifest fields do not match the S2-42r contract")
    if manifest["case_count"] != TOTAL_COUNT or len(manifest["case_digests"]) != TOTAL_COUNT:
        raise CorpusError("manifest case accounting is invalid")
    return manifest


def validate_provenance(build: Any, machine: Any, label: str) -> None:
    if not isinstance(build, dict) or any(not isinstance(build.get(k), str) or not build[k]
                                          for k in ("revision", "compiler", "target", "profile")):
        raise CorpusError(f"{label}: incomplete build provenance")
    if not isinstance(machine, dict) or any(not isinstance(machine.get(k), str) or not machine[k]
                                            for k in ("host_os", "host_arch")):
        raise CorpusError(f"{label}: incomplete machine provenance")


def validate_run_build(build: Any, expected_revision: str,
                       expected_compiler: str, label: str) -> tuple[str, str, str, str]:
    if not isinstance(build, dict):
        raise CorpusError(f"{label}: runner build identity is malformed")
    if (build.get("revision") != expected_revision or build.get("compiler") != expected_compiler
            or not isinstance(build.get("profile"), str)
            or build["profile"] not in RUNNER_BUILD_PROFILES
            or not isinstance(build.get("target"), str) or not build["target"]):
        raise CorpusError(f"{label}: runner build differs from run-level source/compiler/configuration identity")
    return (build["revision"], build["compiler"], build["target"], build["profile"])


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
            raise CorpusError(f"{label}: {field} differs from exact corpus identity/oracle")
    validate_provenance(result.get("build"), result.get("machine"), label)
    if backend == "native_x86" and (machine.get("host_os") != "linux" or machine.get("host_arch") != "x86_64"):
        raise CorpusError(f"{label}: physical oracle is not Linux x86_64")
    snap = result.get("snapshot")
    if not isinstance(snap, dict) or snap.get("status") != "available" or not isinstance(snap.get("value"), dict):
        raise CorpusError(f"{label}: full architectural snapshot unavailable")
    actual = snap["value"]
    state = expected["state"]
    if actual.get("state") != {k: state[k] for k in ("mode", "gpr", "rip", "rflags")}:
        raise CorpusError(f"{label}: complete GPR/RIP/RFLAGS state differs from independent oracle")
    if actual.get("rflags_defined") != state["rflags_defined"]:
        raise CorpusError(f"{label}: RFLAGS definedness differs")
    if actual.get("memory") != expected["memory"]:
        raise CorpusError(f"{label}: complete code mapping changed")
    exception = actual.get("exception")
    if not isinstance(exception, dict) or exception.get("status") != "available" or exception.get("value") is not None:
        raise CorpusError(f"{label}: exception observation is missing or nonempty")


def command_identity(executable: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    digest = sha256(executable.read_bytes())
    revision = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
    host_os = platform.system().lower()
    host_arch = platform.machine().lower()
    host_os = {"darwin": "macos"}.get(host_os, host_os)
    host_arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(host_arch, host_arch)
    return {"executable_sha256": digest, "revision": revision}, {
        "host_os": host_os, "host_arch": host_arch}


def run_interpreter(directory: Path, results_path: Path, executable: Path) -> dict[str, Any]:
    manifest = verify_corpus(directory)
    identity, execution_machine = command_identity(executable)
    rows: list[dict[str, Any]] = []
    failures: list[str] = []
    build = machine = None
    expected_doc = load_json(directory / EXPECTED_NAME)
    expected_by_id = {item["case_id"]: item for item in expected_doc["cases"]}
    for item in manifest["case_digests"]:
        identifier = item["case_id"]
        fixture_path = directory / f"{identifier}.case.json"
        raw = fixture_path.read_bytes()
        fixture = load_json(fixture_path)
        observations = []
        for repeat in range(REPEATS):
            process = subprocess.run([str(executable), "--fixture", str(fixture_path),
                                      "--engine", "interpreter", "--json"],
                                     capture_output=True, text=True, check=False)
            try:
                if process.returncode != 0:
                    raise CorpusError(f"runner exited {process.returncode}: {process.stderr.strip()}")
                result = json.loads(process.stdout, object_pairs_hook=strict_object)
                current_build, current_machine = result.get("build"), result.get("machine")
                validate_provenance(current_build, current_machine, "interpreter result")
                if build is None:
                    build, machine = current_build, current_machine
                if current_build != build or current_machine != machine:
                    raise CorpusError("runner identity changed during corpus run")
                compare_result(result, raw, expected_by_id[identifier], "interpreter", build, machine,
                               f"{identifier} repeat {repeat + 1}")
                observations.append(result)
            except (CorpusError, json.JSONDecodeError, OSError, subprocess.SubprocessError) as error:
                failures.append(f"{identifier} repeat {repeat + 1}: {error}")
                observations.append({"failure": str(error), "returncode": process.returncode})
        if len(observations) == 2 and all("failure" not in row for row in observations) and observations[0] != observations[1]:
            failures.append(f"{identifier}: repeat results differ")
        rows.append({"case_id": identifier, "fixture_sha256": sha256(raw), "repeats": observations})
    summary = {"schema_version": 1, "status": "failed" if failures else "passed", "backend": "interpreter",
        "physical_native_acceptance": "not_run", "profile": PROFILE,
        "manifest_sha256": sha256((directory / MANIFEST_NAME).read_bytes()), "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "execution_identity": identity, "execution_machine": execution_machine,
        "runner_build": build, "runner_machine": machine, "results": rows, "failures": failures}
    results_path.parent.mkdir(parents=True, exist_ok=True)
    results_path.write_bytes(json_bytes(summary))
    if failures:
        raise CorpusError(f"{len(failures)} local interpreter failure(s); summary: {results_path}")
    return summary


def verify_results(directory: Path, results_path: Path) -> None:
    manifest = verify_corpus(directory)
    result = load_json(results_path)
    if (not isinstance(result, dict) or result.get("schema_version") != 1 or result.get("status") != "passed"
            or result.get("backend") != "interpreter" or result.get("profile") != PROFILE
            or result.get("physical_native_acceptance") != "not_run" or result.get("failures") != []):
        raise CorpusError("saved evidence is not a successful local-only interpreter record")
    if result.get("manifest_sha256") != sha256((directory / MANIFEST_NAME).read_bytes()):
        raise CorpusError("saved evidence is not bound to the exact corpus manifest")
    if result.get("case_count") != TOTAL_COUNT or result.get("repeats_per_case") != REPEATS:
        raise CorpusError("saved evidence case/repeat count differs")
    build, machine = result.get("runner_build"), result.get("runner_machine")
    validate_provenance(build, machine, "saved interpreter evidence")
    execution_identity, execution_machine = result.get("execution_identity"), result.get("execution_machine")
    if (not isinstance(execution_identity, dict)
            or set(execution_identity) != {"executable_sha256", "revision"}
            or not isinstance(execution_identity.get("executable_sha256"), str)
            or len(execution_identity["executable_sha256"]) != 64
            or any(c not in "0123456789abcdef" for c in execution_identity["executable_sha256"])
            or not isinstance(execution_identity.get("revision"), str)
            or not execution_identity["revision"]):
        raise CorpusError("saved evidence lacks a complete executable identity")
    if (not isinstance(execution_machine, dict)
            or any(not isinstance(execution_machine.get(k), str) or not execution_machine[k]
                   for k in ("host_os", "host_arch"))):
        raise CorpusError("saved evidence lacks a complete execution machine identity")
    if (execution_identity["revision"] != build["revision"]
            or any(execution_machine[key] != machine.get(key) for key in ("host_os", "host_arch"))):
        raise CorpusError("saved executable/machine identity differs from runner result provenance")
    rows = result.get("results")
    ordered = [item["case_id"] for item in manifest["case_digests"]]
    if not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows) \
            or [row.get("case_id") for row in rows] != ordered:
        raise CorpusError("saved evidence case order or count differs")
    expected_by_id = {item["case_id"]: item for item in load_json(directory / EXPECTED_NAME)["cases"]}
    for row in rows:
        identifier = row["case_id"]
        raw = (directory / f"{identifier}.case.json").read_bytes()
        observations = row.get("repeats")
        if row.get("fixture_sha256") != sha256(raw) or not isinstance(observations, list) or len(observations) != REPEATS:
            raise CorpusError(f"saved evidence {identifier}: fixture digest or repeat count differs")
        for index, observed in enumerate(observations, 1):
            if not isinstance(observed, dict) or "failure" in observed:
                raise CorpusError(f"saved evidence {identifier} repeat {index} contains a failure")
            compare_result(observed, raw, expected_by_id[identifier], "interpreter", build, machine,
                           f"saved {identifier} repeat {index}")
        if observations[0] != observations[1]:
            raise CorpusError(f"saved evidence {identifier}: repeat results differ")


def run_native_oracle(directory: Path, host: str, user: str, remote_root: str,
                      output_root: Path) -> tuple[int, Path | None, str, str]:
    resolved_output = output_root.resolve()
    work_root = (ROOT / "work").resolve()
    if resolved_output != work_root and work_root not in resolved_output.parents:
        raise CorpusError("native oracle output must remain under work/")
    output_root.mkdir(parents=True, exist_ok=True)
    previous = {path.resolve() for path in output_root.iterdir() if path.is_dir()}
    fixtures = [directory / f"{item['case_id']}.case.json" for item in load_json(directory / MANIFEST_NAME)["case_digests"]]
    command = [sys.executable, str(ROOT / "tests/cpu/stage2_oracle.py"), "--host", host, "--user", user,
               "--remote-root", remote_root, "--repeats", "2", "--output-root",
               str(output_root.resolve().relative_to(ROOT)), *(str(path.resolve().relative_to(ROOT)) for path in fixtures)]
    try:
        process = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)
        created = [path for path in output_root.iterdir() if path.is_dir() and path.resolve() not in previous]
        run_dir = created[0] if len(created) == 1 else None
        if run_dir is not None:
            (run_dir / "s2-42r-command.stdout.log").write_text(process.stdout)
            (run_dir / "s2-42r-command.stderr.log").write_text(process.stderr)
        return process.returncode, run_dir, process.stdout, process.stderr
    except OSError as error:
        return 127, None, "", str(error)


def verify_native_run(directory: Path, run_dir: Path, process_status: int) -> dict[str, Any]:
    if type(process_status) is not int or process_status != 0:
        raise CorpusError("generic native oracle process did not exit successfully")
    manifest = verify_corpus(directory)
    run = load_json(run_dir / "manifest.json")
    if not isinstance(run, dict):
        raise CorpusError("generic native oracle manifest must be an object")
    commands = run.get("commands")
    if (not isinstance(commands, list) or not commands
            or any(not isinstance(command, dict) or type(command.get("returncode")) is not int
                   or command["returncode"] != 0 or "error" in command for command in commands)):
        raise CorpusError("generic oracle command evidence contains failure, timeout, or missing status")
    if (run.get("status") != "passed" or run.get("repeats") != 2 or run.get("profile") != PROFILE
            or run.get("suite") != "integer" or not isinstance(run.get("run_id"), str) or not run["run_id"]
            or not isinstance(run.get("source_fingerprint"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", run["source_fingerprint"])):
        raise CorpusError("generic native oracle did not pass two repeats for this profile")
    expected_revision = f"sha256:{run['source_fingerprint']}"
    machines = run.get("machines")
    if not isinstance(machines, dict) or not isinstance(machines.get("local"), dict):
        raise CorpusError("generic native oracle lacks run-level machine/compiler provenance")
    local_machine = machines["local"]
    local_compiler = local_machine.get("rustc")
    remote_compiler = machines.get("remote_rustc")
    if (not isinstance(local_compiler, str) or not local_compiler
            or not isinstance(remote_compiler, str) or not remote_compiler
            or any(not isinstance(machines.get(field), str) or not machines[field]
                   for field in ("remote_cargo", "remote_uname"))):
        raise CorpusError("generic native oracle run-level local/remote compiler identity is incomplete")
    expected_doc = load_json(directory / EXPECTED_NAME)
    expected_by_id = {item["case_id"]: item for item in expected_doc["cases"]}
    digest_by_id = {item["case_id"]: item["fixture_sha256"] for item in manifest["case_digests"]}
    inputs, records = run.get("inputs"), run.get("fixtures")
    if (not isinstance(inputs, list) or any(not isinstance(item, dict) for item in inputs)
            or len(inputs) != TOTAL_COUNT):
        raise CorpusError("generic oracle evidence omitted or malformed frozen inputs")
    expected_ids = [item["case_id"] for item in manifest["case_digests"]]
    if [item.get("case_id") for item in inputs] != expected_ids:
        raise CorpusError("generic oracle fixture identities differ")
    frozen_dir = run_dir / "fixtures"
    frozen_entries = list(frozen_dir.iterdir()) if frozen_dir.is_dir() else []
    wanted_frozen_names = {f"{identifier}.case.json" for identifier in expected_ids}
    if (not frozen_dir.is_dir() or any(path.is_symlink() or not path.is_file() for path in frozen_entries)
            or {path.name for path in frozen_entries} != wanted_frozen_names):
        raise CorpusError("generic oracle frozen fixture set is missing, extra, or unsafe")
    for item in inputs:
        identifier = item["case_id"]
        name = f"{identifier}.case.json"
        frozen_path = frozen_dir / name
        source_path = directory / name
        if (item.get("profile") != PROFILE or item.get("schema_version") != 2
                or item.get("filename") != name
                or item.get("fixture_sha256") != digest_by_id[identifier]
                or item.get("fixture_path") != str(frozen_path.resolve().relative_to(ROOT))
                or frozen_path.read_bytes() != source_path.read_bytes()
                or sha256(frozen_path.read_bytes()) != digest_by_id[identifier]):
            raise CorpusError(f"generic oracle frozen fixture differs: {identifier}")
    if not isinstance(records, list) or len(records) != TOTAL_COUNT * 2:
        raise CorpusError("generic oracle evidence omitted repeat records")
    seen: set[tuple[str, int]] = set()
    backend_build_identity: dict[str, tuple[str, str, str, str]] = {}
    for record in records:
        if not isinstance(record, dict):
            raise CorpusError("generic oracle result record is malformed")
        identifier, repeat = record.get("case_id"), record.get("repeat")
        if not isinstance(identifier, str):
            raise CorpusError("generic oracle result case_id is malformed")
        key = (identifier, repeat)
        if identifier not in expected_by_id or type(repeat) is not int or repeat not in (1, 2) or key in seen:
            raise CorpusError("generic oracle result identity is unexpected or duplicated")
        if (record.get("fixture_sha256") != digest_by_id[identifier]
                or record.get("differences") != {"interpreter_vs_native": 0}
                or record.get("stable_repeat") is not (repeat > 1)):
            raise CorpusError("generic oracle evidence does not report exact match and stable repeats")
        builds, machines = record.get("build"), record.get("machines")
        if not isinstance(builds, dict) or not isinstance(machines, dict):
            raise CorpusError("generic oracle backend provenance is missing")
        for backend in ("interpreter", "native_x86"):
            build = builds.get(backend)
            machine = machines.get(backend)
            compiler = local_compiler if backend == "interpreter" else remote_compiler
            build_identity = validate_run_build(build, expected_revision, compiler,
                                                f"{backend} {identifier} repeat {repeat}")
            previous_build = backend_build_identity.setdefault(backend, build_identity)
            if build_identity != previous_build:
                raise CorpusError(f"{backend} runner build/configuration changed during the evidence run")
            if not isinstance(machine, dict):
                raise CorpusError(f"{backend} result machine identity is missing")
            case_dir = run_dir / f"repeat-{repeat:02d}" / identifier
            process_path = case_dir / f"{backend}.process.json"
            if not process_path.is_file():
                raise CorpusError(f"{backend} process evidence is missing for {identifier} repeat {repeat}")
            process_record = load_json(process_path)
            if (not isinstance(process_record, dict) or type(process_record.get("returncode")) is not int
                    or process_record["returncode"] != 0):
                raise CorpusError(f"{backend} process did not exit successfully for {identifier} repeat {repeat}")
            result_path = case_dir / f"{backend}.json"
            result_obj = load_json(result_path)
            fixture_raw = (directory / f"{identifier}.case.json").read_bytes()
            compare_result(result_obj, fixture_raw, expected_by_id[identifier], backend,
                           builds.get(backend), machines.get(backend), f"{backend} {identifier} repeat {repeat}")
        seen.add(key)
    if len(seen) != TOTAL_COUNT * 2:
        raise CorpusError("generic oracle repeat matrix is incomplete")
    for backend in ("interpreter", "native_x86"):
        for identifier in expected_by_id:
            first = load_json(run_dir / "repeat-01" / identifier / f"{backend}.json")
            second = load_json(run_dir / "repeat-02" / identifier / f"{backend}.json")
            if first != second:
                raise CorpusError(f"{backend} repeat results differ for {identifier}")
    return {"status": "passed", "profile": PROFILE, "corpus_digest": manifest["corpus_digest"],
            "case_count": TOTAL_COUNT, "repeat_count": 2, "results_per_backend": TOTAL_COUNT * 2,
            "interpreter": "matched_exact_independent_oracle", "native_x86": "matched_exact_independent_oracle",
            "native_acceptance": "passed", "physical_mac_acceptance": "not_run",
            "oracle_manifest": str(run_dir / "manifest.json")}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--generate", action="store_true")
    action.add_argument("--verify", action="store_true")
    action.add_argument("--run-interpreter", action="store_true")
    action.add_argument("--verify-results", action="store_true")
    action.add_argument("--run-oracle", action="store_true", help="require two-repeat interpreter and Linux x86 native acceptance")
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--results", type=Path, default=DEFAULT_RESULTS)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--oracle-output-root", type=Path, default=DEFAULT_ORACLE_ROOT)
    args = parser.parse_args(argv)
    try:
        if args.generate:
            write_file_set(args.corpus, materialize())
            print(f"generated {TOTAL_COUNT} fixtures at {args.corpus}")
        elif args.verify:
            manifest = verify_corpus(args.corpus)
            print(f"verified {TOTAL_COUNT} exact fixtures; corpus digest {manifest['corpus_digest']}")
        elif args.run_interpreter:
            args.results.parent.mkdir(parents=True, exist_ok=True)
            args.results.write_bytes(json_bytes({"status": "failed", "profile": PROFILE,
                "backend": "interpreter", "physical_native_acceptance": "not_run",
                "case_count": TOTAL_COUNT, "failures": ["interpreter run did not complete"]}))
            if not args.runner.is_file():
                raise CorpusError(f"interpreter runner is unavailable: {args.runner}")
            summary = run_interpreter(args.corpus, args.results, args.runner)
            print(f"passed {TOTAL_COUNT} cases × {REPEATS} interpreter repeats; physical native=not_run")
        elif args.verify_results:
            verify_results(args.corpus, args.results)
            print("saved local interpreter evidence verified against all exact snapshots")
        elif args.run_oracle:
            summary_path = args.oracle_output_root / "s2-42r-summary.json"
            summary: dict[str, Any] = {"status": "failed", "profile": PROFILE, "case_count": TOTAL_COUNT,
                "physical_mac_acceptance": "not_run", "native_acceptance": "not_run"}
            summary_path.parent.mkdir(parents=True, exist_ok=True)
            summary_path.write_bytes(json_bytes(summary))
            manifest = verify_corpus(args.corpus)
            summary["corpus_digest"] = manifest["corpus_digest"]
            summary_path.write_bytes(json_bytes(summary))
            status, run_dir, stdout, stderr = run_native_oracle(
                args.corpus, args.host, args.user, args.remote_root, args.oracle_output_root)
            summary.update({"oracle_stdout": stdout[-4000:], "oracle_stderr": stderr[-4000:]})
            if status == 0 and run_dir is not None and (run_dir / "manifest.json").is_file():
                try:
                    summary.update(verify_native_run(args.corpus, run_dir, status))
                except (CorpusError, OSError, UnicodeError, json.JSONDecodeError,
                        KeyError, TypeError, IndexError, ValueError) as error:
                    summary["native_acceptance"] = "failed"
                    summary["error"] = str(error)
            elif status != 0:
                summary["native_acceptance"] = "failed"
                summary["error"] = f"generic oracle exited {status}; native acceptance did not complete"
            elif run_dir is None or not (run_dir / "manifest.json").is_file():
                summary["native_acceptance"] = "failed"
                summary["error"] = "generic oracle did not produce a complete run manifest"
            summary_path.parent.mkdir(parents=True, exist_ok=True)
            summary_path.write_bytes(json_bytes(summary))
            print(f"machine-readable acceptance evidence: {summary_path}")
            if status != 0 or summary.get("status") != "passed":
                print("native acceptance failed or was unavailable; physical acceptance remains not_run", file=sys.stderr)
                return 1
            print(f"matched {TOTAL_COUNT} cases twice on interpreter and Linux x86 native")
        return 0
    except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError,
            KeyError, TypeError, IndexError, ValueError) as error:
        if args.run_oracle:
            failure_path = args.oracle_output_root / "s2-42r-summary.json"
            failure_path.parent.mkdir(parents=True, exist_ok=True)
            failure_path.write_bytes(json_bytes({"status": "failed", "profile": PROFILE,
                "case_count": TOTAL_COUNT, "physical_mac_acceptance": "not_run",
                "native_acceptance": "not_run", "error": str(error)}))
            print(f"machine-readable acceptance evidence: {failure_path}")
        print(f"S2-42r failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
