#!/usr/bin/env python3
"""Build and strictly replay the local-only S2-42y memory XADD corpus."""
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
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42y_xadd_qword_memory_controls"
CONTROL_EXPECTED = CONTROL_DIR / "expected.json"
CONTROL_MANIFEST = CONTROL_DIR / "controls_manifest.json"
CORPUS_DIR = ROOT / "work/stage2/s2-42y-xadd-memory-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42y-xadd-memory-corpus/interpreter-results.json"
NATIVE_PATH = ROOT / "work/stage2/s2-42y-xadd-memory-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42y-xadd-qword-memory-v1"
RIP = 0x00100000
CODE = bytes.fromhex("480fc100")  # XADD qword [RAX], RAX; deliberately no LOCK prefix
DATA_LEFT = 0x00000000003000F8
DATA_RIGHT = 0x0000000000300100
OPERAND_ADDRESS = 0x00000000003000FB
OPERAND_OFFSET = OPERAND_ADDRESS - DATA_LEFT
DATA_LENGTH = 8
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
CONTROL_COUNT = 8
GENERATED_COUNT = len(SEEDS) * CASES_PER_SEED
TOTAL_COUNT = CONTROL_COUNT + GENERATED_COUNT
REPEATS = 2
RFLAGS_DEFINED = 0x0CD7
ARITHMETIC_FLAGS = 0x08D5
MASK64 = 0xFFFFFFFFFFFFFFFF
MANIFEST_FIELDS = {"manifest_version", "generator_version", "profile", "seeds", "cases_per_seed", "control_count", "generated_case_count", "case_count", "case_digests", "control_manifest_sha256", "expected_sha256", "corpus_digest"}
CONTROL_MANIFEST_SHA256 = "0f9689b4894120a4507095bbfddb83b905f9868c7769a311f2b2503f7e3c2ab1"
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12
MEMORY_CLASSES = (
    ("zero", 0), ("one", 1), ("af_carry", 5), ("low_nibble_boundary", 0xF),
    ("positive_max", 0x7FFFFFFFFFFFFFFF), ("positive_near_max", 0x7FFFFFFFFFFFFFFE),
    ("negative_min", 0x8000000000000000), ("negative_min_plus_one", 0x8000000000000001),
    ("negative_one", MASK64), ("negative_two", MASK64 - 1),
    ("wrap_to_zero", (-OPERAND_ADDRESS) & MASK64),
    ("wrap_near_zero", (-OPERAND_ADDRESS + 1) & MASK64),
    ("ordinary_page_edge", 0x0000000000000100), ("ordinary_high", 0x0100000000000000),
    ("ordinary_mixed", 0xAABBCCDDEEFF0011), ("ordinary_sparse", 0x0000000000001000),
)


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def control_id(index: int) -> str:
    return f"s2_42y_xadd_qword_memory_c{index:02d}"


def generated_id(seed_index: int, sequence: int) -> str:
    return f"s2_42y_xadd_qword_memory_g{seed_index}_{sequence:03d}"


def fixture(identifier: str, memory_value: int, gprs: list[int], flags: int,
            kind: str, seed: int | None = None, sequence: int | None = None) -> dict[str, Any]:
    if (len(gprs) != 16 or any(type(x) is not int or not 0 <= x <= MASK64 for x in gprs)
            or gprs[0] != OPERAND_ADDRESS):
        raise CorpusError("input is outside the exact S2-42y XADD qword memory profile")
    if type(memory_value) is not int or not 0 <= memory_value <= MASK64:
        raise CorpusError("memory input is outside the exact S2-42y profile")
    if flags & ~RFLAGS_DEFINED or not flags & 2:
        raise CorpusError("initial RFLAGS contain reserved bits or omit fixed bit 1")
    data = bytearray(b"\xa5" * (2 * DATA_LENGTH))
    data[OPERAND_OFFSET:OPERAND_OFFSET + 8] = memory_value.to_bytes(8, "little")
    source = ("Handwritten exact S2-21d bytes, fixed RAX/address, and selected old-memory value."
              if kind == "handwritten" else
              f"Deterministic S2-42y private-RAM corpus seed={hx(seed or 0)}, sequence={sequence}.")
    return {"schema_version": 2, "case_id": identifier, "profile": PROFILE,
            "provenance": {"kind": "project_fixture", "source": source,
                "specification": "Intel SDM revision 093, Vol. 2D XADD: 48 0F C1 00 performs a 64-bit addition of old qword memory and RAX, stores the sum to memory, copies old memory to RAX, and sets arithmetic flags from the addition. This bounded S2-42y local profile deliberately has no LOCK prefix.",
                "specification_version": "Intel SDM Vol. 2D revision 093 (334569-093US), XADD p.6-23; project S2-21d/S2-42y"},
            "seed": None if seed is None else hx(seed), "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gprs], "rip": hx(RIP), "rflags": hx(flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(RIP), "length": 4,
                 "permissions": {"read": False, "write": False, "execute": True},
                 "initial_bytes_hex": CODE.hex(), "comparison_mask_hex": "ff" * 4},
                {"region_id": "data_left", "guest_address": hx(DATA_LEFT), "length": DATA_LENGTH,
                 "permissions": {"read": True, "write": True, "execute": False},
                 "initial_bytes_hex": data[:DATA_LENGTH].hex(), "comparison_mask_hex": "ff" * DATA_LENGTH},
                {"region_id": "data_right", "guest_address": hx(DATA_RIGHT), "length": DATA_LENGTH,
                 "permissions": {"read": True, "write": True, "execute": False},
                 "initial_bytes_hex": data[DATA_LENGTH:].hex(), "comparison_mask_hex": "ff" * DATA_LENGTH}],
            "stop_rip": hx(RIP + 4), "instruction_budget": 1, "wall_timeout_ms": 2000,
            "rflags_mask": hx(RFLAGS_DEFINED), "expected_exit": "stop_address"}


CONTROL_MEMORY_VALUES = (0, 1, 5, 0x7FFFFFFFFFFFFFFF, 0x8000000000000000, MASK64,
                         (-OPERAND_ADDRESS) & MASK64, 0x100)


def control_inputs() -> dict[str, bytes]:
    files = {}
    base = [OPERAND_ADDRESS, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
            0x0000000000200000, 0x0000000000300080, 0x0000000000300080,
            0x0000000000200004, 0x0000000000200010, 0x0123456789ABCDEF,
            0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
            0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, MASK64]
    for index, old_memory in enumerate(CONTROL_MEMORY_VALUES):
        gprs = list(base)
        gprs[1] ^= index * 0x0101010101010101
        item = fixture(control_id(index), old_memory, gprs, RFLAGS_DEFINED, "handwritten")
        files[f"{item['case_id']}.case.json"] = json_bytes(item)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    result, unique = [], set()
    for seed_index, seed in enumerate(SEEDS):
        for sequence in range(CASES_PER_SEED):
            rng = random.Random(seed ^ ((sequence + 1) * 0x9E3779B97F4A7C15))
            memory_value = MEMORY_CLASSES[sequence % len(MEMORY_CLASSES)][1]
            gprs = [rng.getrandbits(64) for _ in range(16)]
            gprs[0] = OPERAND_ADDRESS
            gprs[15] = 0x4274000000000000 | (seed_index << 32) | sequence
            flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
            item = fixture(generated_id(seed_index, sequence), memory_value, gprs,
                           flags, "generated", seed, sequence)
            initial = item["initial_state"]
            unique.add((tuple(initial["gpr"]), initial["rflags"],
                        item["memory"][1]["initial_bytes_hex"], item["memory"][2]["initial_bytes_hex"]))
            result.append(item)
    if len(unique) != GENERATED_COUNT:
        raise CorpusError("generated architectural input states are not unique")
    return result


def arithmetic_flags(a: int, b: int) -> int:
    """Compute the six defined arithmetic flags for unsigned 64-bit addition."""
    result = (a + b) & MASK64
    carry = int(a + b > MASK64)
    parity = int((result & 0xFF).bit_count() % 2 == 0)
    auxiliary = int(((a ^ b ^ result) & 0x10) != 0)
    zero, sign = int(result == 0), (result >> 63) & 1
    overflow = int((~(a ^ b) & (a ^ result) & (1 << 63)) != 0)
    return carry | (parity << 2) | (auxiliary << 4) | (zero << 6) | (sign << 7) | (overflow << 11)


def expected_snapshot(item: dict[str, Any]) -> dict[str, Any]:
    """Standalone integer oracle; snapshots old RAX and memory before computing XADD."""
    initial = item["initial_state"]
    regions = item["memory"]
    if (len(regions) != 3 or bytes.fromhex(regions[0]["initial_bytes_hex"]) != CODE
            or int(regions[1]["guest_address"], 16) != DATA_LEFT
            or int(regions[2]["guest_address"], 16) != DATA_RIGHT
            or regions[1]["length"] != DATA_LENGTH or regions[2]["length"] != DATA_LENGTH):
        raise CorpusError("oracle requires exact bare XADD bytes and S2-21d split-memory geometry")
    gprs = [int(x, 16) for x in initial["gpr"]]
    old_address = gprs[0]
    if old_address != OPERAND_ADDRESS:
        raise CorpusError("oracle requires fixed old RAX/effective address")
    data = bytearray.fromhex(regions[1]["initial_bytes_hex"] + regions[2]["initial_bytes_hex"])
    old_memory = int.from_bytes(data[OPERAND_OFFSET:OPERAND_OFFSET + 8], "little")
    result = (old_memory + old_address) & MASK64
    flags = int(initial["rflags"], 16)
    gprs[0] = old_memory
    data[OPERAND_OFFSET:OPERAND_OFFSET + 8] = result.to_bytes(8, "little")
    final_flags = (flags & ~ARITHMETIC_FLAGS) | arithmetic_flags(old_memory, old_address)
    return {"case_id": item["case_id"],
            "state": {"mode": "long64", "gpr": [hx(v) for v in gprs],
                      "rip": hx(RIP + 4), "rflags": hx(final_flags),
                      "rflags_defined": hx(RFLAGS_DEFINED)},
            "memory": [{"region_id": "code", "guest_address": hx(RIP), "bytes_hex": CODE.hex()},
                       {"region_id": "data_left", "guest_address": hx(DATA_LEFT), "bytes_hex": data[:DATA_LENGTH].hex()},
                       {"region_id": "data_right", "guest_address": hx(DATA_RIGHT), "bytes_hex": data[DATA_LENGTH:].hex()}],
            "outcome": {"kind": "stop_address"},
            "instructions_executed": {"status": "available", "value": hx(1)}, "exception": None}


def load_controls():
    if not CONTROL_DIR.is_dir() or CONTROL_DIR.is_symlink(): raise CorpusError("S2-42y control directory missing or unsafe")
    if any(path.is_symlink() or not path.is_file() for path in (CONTROL_MANIFEST, CONTROL_EXPECTED)):
        raise CorpusError("S2-42y static control documents are missing or unsafe")
    if CONTROL_MANIFEST_SHA256 == "TO_BE_PINNED" or sha256(CONTROL_MANIFEST.read_bytes()) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned S2-42y control manifest digest differs")
    manifest_raw = CONTROL_MANIFEST.read_bytes()
    manifest = json.loads(manifest_raw, object_pairs_hook=strict_object)
    expected_raw = CONTROL_EXPECTED.read_bytes()
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
            or manifest.get("schema_version") != 1 or manifest.get("profile") != PROFILE
            or manifest.get("control_count") != CONTROL_COUNT or not isinstance(manifest.get("fixtures"), list)
            or len(manifest["fixtures"]) != CONTROL_COUNT or sha256(expected_raw) != manifest.get("expected_sha256")):
        raise CorpusError("S2-42y static control snapshots or manifest are invalid")
    expected_doc = json.loads(expected_raw, object_pairs_hook=strict_object)
    if not isinstance(expected_doc, dict) or set(expected_doc) != {"profile", "cases"} or expected_doc.get("profile") != PROFILE or not isinstance(expected_doc.get("cases"), list) or len(expected_doc["cases"]) != CONTROL_COUNT:
        raise CorpusError("S2-42y static expected-state document has invalid shape")
    expected_by_id = {x["case_id"]: x for x in expected_doc["cases"]}
    if len(expected_by_id) != CONTROL_COUNT or list(expected_by_id) != [control_id(i) for i in range(CONTROL_COUNT)]:
        raise CorpusError("S2-42y frozen expected states are duplicated, missing, or out of order")
    controls, fixed = [], control_inputs()
    names = {"controls_manifest.json", "expected.json"}
    for i in range(CONTROL_COUNT):
        identifier, name = control_id(i), f"{control_id(i)}.case.json"
        names.add(name)
        row = manifest["fixtures"][i]
        raw = (CONTROL_DIR / name).read_bytes()
        expected = expected_by_id[identifier]
        if (not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "expected_sha256"}
                or row.get("case_id") != identifier or raw != fixed[name]
                or sha256(raw) != row.get("fixture_sha256") or sha256(json_bytes(expected)) != row.get("expected_sha256")
                or expected != expected_snapshot(json.loads(raw, object_pairs_hook=strict_object))):
            raise CorpusError(f"S2-42y frozen control changed: {identifier}")
        controls.append((identifier, raw, expected))
    entries = list(CONTROL_DIR.iterdir())
    if any(x.is_symlink() or not x.is_file() for x in entries) or {x.name for x in entries} != names:
        raise CorpusError("S2-42y controls have missing, extra, or symlink entries")
    return controls, manifest_raw


def all_cases():
    controls, control_raw = load_controls()
    rows = [(i, raw, "handwritten", exp) for i, raw, exp in controls]
    for item in generated_fixtures():
        raw = json_bytes(item)
        rows.append((item["case_id"], raw, "generated", expected_snapshot(item)))
    return rows, control_raw


def materialize() -> dict[str, bytes]:
    rows, _ = all_cases()
    files, digests, snapshots = {}, [], []
    for identifier, raw, kind, expected in rows:
        files[f"{identifier}.case.json"] = raw
        eraw = json_bytes(expected)
        snapshots.append(expected)
        digests.append({"case_id": identifier, "kind": kind, "fixture_sha256": sha256(raw), "expected_sha256": sha256(eraw)})
    expected_raw = json_bytes({"profile": PROFILE, "cases": snapshots})
    files["expected.json"] = expected_raw
    files["manifest.json"] = json_bytes({"manifest_version": 1, "generator_version": GENERATOR_VERSION, "profile": PROFILE,
        "seeds": [hx(x) for x in SEEDS], "cases_per_seed": CASES_PER_SEED, "control_count": CONTROL_COUNT,
        "generated_case_count": GENERATED_COUNT, "case_count": TOTAL_COUNT, "case_digests": digests,
        "control_manifest_sha256": CONTROL_MANIFEST_SHA256, "expected_sha256": sha256(expected_raw),
        "corpus_digest": sha256("\n".join(x["fixture_sha256"] for x in digests).encode())})
    return files


def verify_corpus(directory: Path):
    if directory.is_symlink() or not directory.is_dir(): raise CorpusError("S2-42y corpus directory missing or unsafe")
    files = materialize()
    verify_file_set(directory, files)
    manifest = load_json(directory / "manifest.json")
    if set(manifest) != MANIFEST_FIELDS or manifest["case_count"] != TOTAL_COUNT or len(manifest["case_digests"]) != TOTAL_COUNT:
        raise CorpusError("S2-42y manifest shape or accounting differs")
    return manifest


RESULT_FIELDS = {"schema_version", "case_id", "input_sha256", "profile", "backend", "outcome",
                 "instructions_executed", "snapshot", "build", "machine"}
BUILD_FIELDS = {"revision", "target", "profile", "compiler"}
MACHINE_FIELDS = {"host_os", "host_arch", "cpu_model", "kernel", "page_size"}
SNAPSHOT_FIELDS = {"status", "value"}
SNAPSHOT_VALUE_FIELDS = {"state", "rflags_defined", "memory", "minss_memory_rax_controls",
                         "addss_memory_rax_controls", "exception"}
STATE_FIELDS = {"mode", "gpr", "rip", "rflags"}
MEMORY_FIELDS = {"region_id", "guest_address", "bytes_hex"}


def validate_provenance(build, machine, label):
    if not isinstance(build, dict) or set(build) != BUILD_FIELDS:
        raise CorpusError(f"{label}: build provenance fields differ")
    if any(not isinstance(build.get(key), str) or not build[key] for key in BUILD_FIELDS):
        raise CorpusError(f"{label}: incomplete build provenance")
    if not isinstance(machine, dict) or set(machine) != MACHINE_FIELDS:
        raise CorpusError(f"{label}: machine provenance fields differ")
    if any(not isinstance(machine.get(key), str) or not machine[key] for key in ("host_os", "host_arch")):
        raise CorpusError(f"{label}: incomplete machine provenance")
    if any(value is not None and not isinstance(value, str) for value in (machine["cpu_model"], machine["kernel"])):
        raise CorpusError(f"{label}: invalid optional machine text provenance")
    if machine["page_size"] is not None and (type(machine["page_size"]) is not int
            or not 1 <= machine["page_size"] <= MASK64):
        raise CorpusError(f"{label}: invalid optional machine page size")


def compare_result(result, raw, expected, build, machine, label):
    if not isinstance(result, dict) or set(result) != RESULT_FIELDS:
        raise CorpusError(f"{label}: result fields differ")
    if type(result["schema_version"]) is not int or result["schema_version"] != 2:
        raise CorpusError(f"{label}: result schema_version must be the supported u32 value 2")
    wanted = {"case_id": expected["case_id"], "input_sha256": sha256(raw), "profile": PROFILE,
              "backend": "interpreter", "build": build, "machine": machine}
    if any(result.get(key) != value for key, value in wanted.items()):
        raise CorpusError(f"{label}: result identity differs from oracle")
    validate_provenance(result["build"], result["machine"], label)

    outcome = result["outcome"]
    if (not isinstance(outcome, dict) or set(outcome) != {"kind"}
            or not isinstance(outcome.get("kind"), str) or outcome != expected["outcome"]):
        raise CorpusError(f"{label}: outcome fields or value differ")
    retirement = result["instructions_executed"]
    if (not isinstance(retirement, dict) or set(retirement) != {"status", "value"}
            or retirement.get("status") != "available" or not isinstance(retirement.get("value"), str)
            or retirement != expected["instructions_executed"]):
        raise CorpusError(f"{label}: retirement observation fields or value differ")

    snap = result["snapshot"]
    if (not isinstance(snap, dict) or set(snap) != SNAPSHOT_FIELDS
            or snap.get("status") != "available" or not isinstance(snap.get("value"), dict)):
        raise CorpusError(f"{label}: complete snapshot unavailable or malformed")
    actual = snap["value"]
    if set(actual) != SNAPSHOT_VALUE_FIELDS:
        raise CorpusError(f"{label}: snapshot value fields differ")
    state = expected["state"]
    observed_state = actual.get("state")
    if not isinstance(observed_state, dict) or set(observed_state) != STATE_FIELDS:
        raise CorpusError(f"{label}: architectural state fields differ")
    if (observed_state != {key: state[key] for key in STATE_FIELDS}
            or actual.get("rflags_defined") != state["rflags_defined"]):
        raise CorpusError(f"{label}: complete GPR/RIP/RFLAGS differs")
    if actual["minss_memory_rax_controls"] is not None or actual["addss_memory_rax_controls"] is not None:
        raise CorpusError(f"{label}: unrelated optional snapshot controls are present")
    memory = actual.get("memory")
    if not isinstance(memory, list) or len(memory) != len(expected["memory"]):
        raise CorpusError(f"{label}: complete memory snapshot is malformed")
    for region in memory:
        if not isinstance(region, dict) or set(region) != MEMORY_FIELDS or any(not isinstance(region.get(key), str) for key in MEMORY_FIELDS):
            raise CorpusError(f"{label}: memory-region fields or types differ")
    if memory != expected["memory"]:
        raise CorpusError(f"{label}: code, data, or canary bytes differ")
    exception = actual["exception"]
    if (not isinstance(exception, dict) or set(exception) != {"status", "value"}
            or exception.get("status") != "available" or exception.get("value") is not None):
        raise CorpusError(f"{label}: exception observation fields or value differ")


def missing_attempts(ids, rows):
    by_id = {x.get("case_id"): x for x in rows if isinstance(x, dict)}
    out = []
    for identifier in ids:
        row = by_id.get(identifier, {})
        for repeat in range(REPEATS):
            ps, rs = row.get("processes", []), row.get("repeats", [])
            if repeat >= len(ps) or ps[repeat].get("returncode") != 0 or repeat >= len(rs) or "failure" in rs[repeat]: out.append({"case_id": identifier, "repeat": repeat + 1})
    return out


def expected_ids(manifest): return [x["case_id"] for x in manifest["case_digests"]]


def process_environment(): return {key: os.environ.get(key) for key in PROCESS_ENV_KEYS}


def output_text(value):
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def command_identity(executable):
    executable = executable.resolve(strict=True)
    revision = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    osname = {"darwin": "macos"}.get(platform.system().lower(), platform.system().lower())
    arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(platform.machine().lower(), platform.machine().lower())
    return {"executable_sha256": sha256(executable.read_bytes()), "revision": revision}, {"host_os": osname, "host_arch": arch}


def write_checkpoint(path, directory, manifest, rows, failures, identity, machine, build, runner_machine, status="incomplete"):
    ids = expected_ids(manifest)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": status, "backend": "interpreter", "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "manifest_sha256": sha256((directory / "manifest.json").read_bytes()), "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS,
        "execution_identity": identity, "execution_machine": machine, "runner_build": build, "runner_machine": runner_machine,
        "results": rows, "failures": failures, "missing_attempts": missing_attempts(ids, rows)}))


def write_initial_incomplete_summary(path):
    ids = ([control_id(i) for i in range(CONTROL_COUNT)]
           + [generated_id(si, seq) for si in range(len(SEEDS)) for seq in range(CASES_PER_SEED)])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": "incomplete", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE, "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "results": [],
        "failures": ["run is in progress or preflight did not complete"],
        "missing_attempts": missing_attempts(ids, [])}))


def run_interpreter(directory, result_path, executable):
    write_initial_incomplete_summary(result_path)
    manifest = verify_corpus(directory)
    identity, machine = command_identity(executable)
    expected = {x["case_id"]: x for x in load_json(directory / "expected.json")["cases"]}
    rows, failures = [], []
    build = runner_machine = None
    env = process_environment()
    child_env = {k: v for k, v in env.items() if v is not None}
    for item in manifest["case_digests"]:
        identifier = item["case_id"]
        path = (directory / f"{identifier}.case.json").resolve(strict=True)
        raw = path.read_bytes()
        row = {"case_id": identifier, "fixture_sha256": sha256(raw), "repeats": [], "processes": []}
        rows.append(row)
        for repeat in range(REPEATS):
            cmd = [str(executable.resolve(strict=True)), "--fixture", str(path), "--engine", "interpreter", "--json"]
            try:
                p = subprocess.run(cmd, capture_output=True, text=True, check=False, timeout=RUNNER_TIMEOUT_SECONDS, cwd=str(ROOT.resolve()), env=child_env)
                proc = {"returncode": p.returncode, "stdout": p.stdout, "stderr": p.stderr, "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None}
                row["processes"].append(proc)
                if p.returncode: raise CorpusError(f"runner exited {p.returncode}: {p.stderr.strip()}")
                observation = json.loads(p.stdout, object_pairs_hook=strict_object)
                if not isinstance(observation, dict):
                    raise CorpusError("runner output must be one JSON object")
                b, m = observation.get("build"), observation.get("machine")
                validate_provenance(b, m, "interpreter result")
                if build is None: build, runner_machine = b, m
                if b != build or m != runner_machine: raise CorpusError("runner provenance changed during run")
                compare_result(observation, raw, expected[identifier], build, runner_machine, f"{identifier} repeat {repeat+1}")
                row["repeats"].append(observation)
            except KeyboardInterrupt as e:
                row["processes"].append({"returncode": None, "stdout": "", "stderr": "interrupted", "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": "interrupted", "returncode": None})
                failures.append(f"{identifier} repeat {repeat+1}: interrupted")
                write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
                raise CorpusError(f"interpreter run interrupted; checkpoint saved: {result_path}") from e
            except subprocess.TimeoutExpired as e:
                message = str(e)
                row["processes"].append({"returncode": None, "stdout": output_text(e.stdout),
                    "stderr": output_text(e.stderr), "command": cmd, "cwd": str(ROOT.resolve()),
                    "environment": env, "timeout_diagnosis": {"kind": "timeout_expired",
                    "timeout_seconds": RUNNER_TIMEOUT_SECONDS, "message": message}})
                failures.append(f"{identifier} repeat {repeat+1}: timed out after {RUNNER_TIMEOUT_SECONDS}s: {message}")
                row["repeats"].append({"failure": message, "returncode": None})
            except (OSError, subprocess.SubprocessError, json.JSONDecodeError, CorpusError) as e:
                if len(row["processes"]) == repeat: row["processes"].append({"returncode": None, "stdout": "", "stderr": str(e), "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": str(e), "returncode": row["processes"][-1]["returncode"]})
                failures.append(f"{identifier} repeat {repeat+1}: {e}")
        if (len(row["repeats"]) == REPEATS
                and all(isinstance(x, dict) and "failure" not in x for x in row["repeats"])
                and row["repeats"][0] != row["repeats"][1]):
            failures.append(f"{identifier}: repeat results differ")
        write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
    status = "failed" if failures else "passed"
    write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine, status)
    if failures: raise CorpusError(f"{len(failures)} interpreter failures; saved: {result_path}")
    return load_json(result_path)


SAVED_RESULT_FIELDS = {"schema_version", "status", "backend", "native_x86", "profile", "manifest_sha256",
                       "case_count", "repeats_per_case", "execution_identity", "execution_machine",
                       "runner_build", "runner_machine", "results", "failures", "missing_attempts"}
SAVED_CASE_FIELDS = {"case_id", "fixture_sha256", "repeats", "processes"}


def verify_results(directory, result_path, executable):
    manifest = verify_corpus(directory)
    result = load_json(result_path)
    if not isinstance(result, dict) or set(result) != SAVED_RESULT_FIELDS:
        raise CorpusError("saved evidence fields differ")
    if type(result.get("schema_version")) is not int or result["schema_version"] != 1:
        raise CorpusError("saved evidence schema_version must be the supported u32 value 1")
    if (result.get("schema_version") != 1 or result.get("status") != "passed" or result.get("backend") != "interpreter"
            or result.get("native_x86") != "not_run_form_not_admitted" or result.get("profile") != PROFILE
            or result.get("failures") != [] or result.get("missing_attempts") != []):
        raise CorpusError("saved evidence is not a successful local interpreter record")
    if result.get("manifest_sha256") != sha256((directory / "manifest.json").read_bytes()) or result.get("case_count") != TOTAL_COUNT or result.get("repeats_per_case") != REPEATS: raise CorpusError("saved result/corpus identity differs")
    build, machine = result.get("runner_build"), result.get("runner_machine")
    validate_provenance(build, machine, "saved results")
    identity, execution_machine = result.get("execution_identity"), result.get("execution_machine")
    selected = executable.resolve(strict=True)
    if (not isinstance(identity, dict) or set(identity) != {"executable_sha256", "revision"}
            or identity.get("executable_sha256") != sha256(selected.read_bytes())
            or identity.get("revision") != build.get("revision")):
        raise CorpusError("saved runner/source provenance differs")
    if execution_machine != {k: machine[k] for k in ("host_os", "host_arch")}: raise CorpusError("saved host provenance differs")
    rows = result.get("results")
    if (not isinstance(rows, list) or any(not isinstance(row, dict) or set(row) != SAVED_CASE_FIELDS for row in rows)
            or [row["case_id"] for row in rows] != expected_ids(manifest)):
        raise CorpusError("saved result order/count/fields differ")
    exp = {x["case_id"]: x for x in load_json(directory / "expected.json")["cases"]}
    env0 = None
    for row in rows:
        ident = row["case_id"]; raw = (directory / f"{ident}.case.json").read_bytes()
        if row.get("fixture_sha256") != sha256(raw) or len(row.get("repeats", [])) != REPEATS or len(row.get("processes", [])) != REPEATS: raise CorpusError(f"saved {ident}: fixture/repeat count differs")
        for i, (observed, proc) in enumerate(zip(row["repeats"], row["processes"]), 1):
            if (not isinstance(proc, dict) or set(proc) != {"returncode", "stdout", "stderr", "command", "cwd", "environment", "timeout_diagnosis"}
                    or type(proc.get("returncode")) is not int or proc["returncode"] != 0
                    or not isinstance(proc.get("stdout"), str) or not isinstance(proc.get("stderr"), str)
                    or proc["timeout_diagnosis"] is not None):
                raise CorpusError(f"saved {ident} repeat {i}: invalid process evidence")
            path = (directory / f"{ident}.case.json").resolve(strict=True)
            if proc["command"] != [str(selected), "--fixture", str(path), "--engine", "interpreter", "--json"] or proc["cwd"] != str(ROOT.resolve()): raise CorpusError(f"saved {ident} repeat {i}: replay argv/cwd differs")
            env = proc["environment"]
            if (not isinstance(env, dict) or set(env) != set(PROCESS_ENV_KEYS)
                    or any(v is not None and not isinstance(v, str) for v in env.values())
                    or (env0 is not None and env != env0)):
                raise CorpusError(f"saved {ident} repeat {i}: environment provenance differs")
            env0 = env
            try: parsed = json.loads(proc["stdout"], object_pairs_hook=strict_object)
            except (json.JSONDecodeError, CorpusError) as e: raise CorpusError(f"saved {ident} repeat {i}: invalid frozen stdout") from e
            compare_result(parsed, raw, exp[ident], build, machine, f"saved {ident} repeat {i} stdout")
            if parsed != observed: raise CorpusError(f"saved {ident} repeat {i}: stdout/result differs")
            compare_result(observed, raw, exp[ident], build, machine, f"saved {ident} repeat {i}")
        if row["repeats"][0] != row["repeats"][1]: raise CorpusError(f"saved {ident}: repeats disagree")


def write_native_blocked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": "blocked", "profile": PROFILE,
        "native_x86": "not_run_form_not_admitted", "blocked_reason": "S2-42y denies native execution; the local-only bare XADD memory form is not admitted to the native backend.", "backend_execution": "not_started"}))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_mutually_exclusive_group(required=True)
    for flag in ("generate", "verify", "run-interpreter", "verify-results", "native"): actions.add_argument("--" + flag, action="store_true")
    parser.add_argument("--corpus", type=Path, default=CORPUS_DIR)
    parser.add_argument("--results", type=Path)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner")
    args = parser.parse_args(argv)
    try:
        if args.generate:
            write_file_set(args.corpus, materialize()); print(f"generated {TOTAL_COUNT} S2-42y fixtures")
        elif args.verify:
            m = verify_corpus(args.corpus); print(f"verified {TOTAL_COUNT} S2-42y fixtures; digest {m['corpus_digest']}")
        elif args.run_interpreter:
            path = args.results or RESULTS_PATH
            try: run_interpreter(args.corpus, path, args.runner)
            except (CorpusError, OSError, subprocess.SubprocessError, json.JSONDecodeError) as e:
                try: saved = load_json(path)
                except Exception: saved = {}
                saved.update({"schema_version": 1, "status": "failed", "backend": "interpreter", "native_x86": "not_run_form_not_admitted", "profile": PROFILE, "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS, "error": str(e)})
                saved.setdefault("failures", []).append(str(e)); path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(json_bytes(saved)); raise
            print(f"passed {TOTAL_COUNT} cases × {REPEATS} local interpreter runs; native_x86=not_run_form_not_admitted")
        elif args.verify_results:
            verify_results(args.corpus, args.results or RESULTS_PATH, args.runner); print("verified saved local interpreter results")
        else:
            write_native_blocked(args.results or NATIVE_PATH); print("native execution blocked; backend_execution=not_started"); return 1
    except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError) as e:
        print(f"S2-42y failed closed: {e}", file=sys.stderr); return 1
    return 0


if __name__ == "__main__": raise SystemExit(main())
