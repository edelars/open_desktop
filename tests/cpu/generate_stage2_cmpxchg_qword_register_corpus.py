#!/usr/bin/env python3
"""Build and strictly replay the local-only S2-42v register CMPXCHG corpus."""
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
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42v_cmpxchg_qword_register_controls"
CONTROL_EXPECTED = CONTROL_DIR / "expected.json"
CONTROL_MANIFEST = CONTROL_DIR / "controls_manifest.json"
CORPUS_DIR = ROOT / "work/stage2/s2-42v-cmpxchg-register-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42v-cmpxchg-register-corpus/interpreter-results.json"
NATIVE_PATH = ROOT / "work/stage2/s2-42v-cmpxchg-register-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42v-cmpxchg-register-v1"
RIP = 0x00100000
CODE_D9 = bytes.fromhex("480fb1d9")  # CMPXCHG RCX, RBX
CODE_C9 = bytes.fromhex("480fb1c9")  # CMPXCHG RCX, RCX
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
CONTROL_COUNT = 16
GENERATED_COUNT = len(SEEDS) * CASES_PER_SEED
TOTAL_COUNT = CONTROL_COUNT + GENERATED_COUNT
REPEATS = 2
RFLAGS_DEFINED = 0x0CD7
ARITHMETIC_FLAGS = 0x08D5
MASK64 = 0xFFFFFFFFFFFFFFFF
MANIFEST_FIELDS = {"manifest_version", "generator_version", "profile", "seeds", "cases_per_seed", "control_count", "generated_case_count", "case_count", "case_digests", "control_manifest_sha256", "expected_sha256", "corpus_digest"}
CONTROL_MANIFEST_SHA256 = "08a38fc357aa38322de037299161ba9e517dbafeaa661df268219cac1be2edfa"
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def control_id(index: int) -> str:
    return f"s2_42v_cmpxchg_qword_register_c{index:02d}"


def generated_id(seed_index: int, sequence: int) -> str:
    return f"s2_42v_cmpxchg_qword_register_g{seed_index}_{sequence:03d}"


def fixture(identifier: str, code: bytes, gprs: list[int], flags: int, source: str,
            seed: int | None = None) -> dict[str, Any]:
    if code not in (CODE_D9, CODE_C9) or len(gprs) != 16 or any(type(x) is not int or not 0 <= x <= MASK64 for x in gprs):
        raise CorpusError("input is outside the exact S2-42v register CMPXCHG profile")
    if flags & ~RFLAGS_DEFINED or not flags & 2:
        raise CorpusError("initial RFLAGS contain reserved bits or omit fixed bit 1")
    return {"schema_version": 2, "case_id": identifier, "profile": PROFILE,
            "provenance": {"kind": "project_fixture", "source": source,
            "specification": "Intel SDM revision 093, Vol. 2A CMPXCHG: REX.W + 0F B1 /r compares RAX with RCX; for bytes 48 0F B1 D9 equality writes RBX to RCX, while alias bytes 48 0F B1 C9 leave RCX unchanged; mismatch copies RCX to RAX; arithmetic flags reflect RAX-RCX. S2-42v admits only these two encodings.",
                           "specification_version": "Intel SDM revision 093; project S2-42v"},
            "seed": None if seed is None else hx(seed), "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gprs], "rip": hx(RIP), "rflags": hx(flags)},
            "memory": [{"region_id": "code", "guest_address": hx(RIP), "length": len(code),
                        "permissions": {"read": False, "write": False, "execute": True},
                        "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)}],
            "stop_rip": hx(RIP + len(code)), "instruction_budget": 1, "wall_timeout_ms": 2000,
            "rflags_mask": hx(RFLAGS_DEFINED), "expected_exit": "stop_address"}


# Inputs chosen by hand to exercise equality/mismatch and each subtraction flag boundary.
CONTROL_INPUTS = (
    (CODE_D9, 0, 0), (CODE_C9, 1, 0),
    (CODE_D9, 0, 1), (CODE_D9, 1, 0),
    (CODE_D9, 0x0F, 1), (CODE_D9, 0x10, 1),
    (CODE_D9, 0x7FFFFFFFFFFFFFFF, MASK64),
    (CODE_D9, 0x8000000000000000, 1),
    (CODE_D9, MASK64, 0x7FFFFFFFFFFFFFFF),
    (CODE_D9, 0x8000000000000000, 0x7FFFFFFFFFFFFFFF),
    (CODE_D9, 0x7FFFFFFFFFFFFFFF, 0x8000000000000000),
    (CODE_D9, 0x0100000000000000, 0),
    (CODE_D9, 0x1000000000000000, 0),
    (CODE_D9, 0x7FFFFFFFFFFFFFFE, 0x7FFFFFFFFFFFFFFF),
    (CODE_D9, 0x8000000000000001, 0x8000000000000000),
    (CODE_C9, 0xFFFFFFFFFFFFFFFF, 0xFFFFFFFFFFFFFFFF),
)


def control_inputs() -> dict[str, bytes]:
    files = {}
    for i, (code, accumulator, destination) in enumerate(CONTROL_INPUTS):
        gprs = [0x0123456789ABCDEF, 0x1122334455667788, 0xDEADBEEFFEDCBA98,
                0xAABBCCDDEEFF0011, 0x5566778899AABBCC, 0x8877665544332211,
                0x0000000000200004, 0x0000000000200010, 0xAABBCCDDEEFF0011,
                0xFEDCBA9876543210, 0x0123456789ABCDEF, 0x9988776655443322,
                0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, MASK64]
        gprs[0], gprs[1] = accumulator, destination
        item = fixture(control_id(i), code, gprs, RFLAGS_DEFINED,
                       "Handwritten S2-42v byte encoding, input registers, and control identity.")
        files[f"{item['case_id']}.case.json"] = json_bytes(item)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    result, unique = [], set()
    for si, seed in enumerate(SEEDS):
        for seq in range(CASES_PER_SEED):
            rng = random.Random(seed ^ ((seq + 1) * 0x9E3779B97F4A7C15))
            # Cycle through equality and directed subtraction boundaries while varying all other GPRs.
            mode = seq % 16
            if mode < 8:
                accumulator = destination = rng.getrandbits(64)
            elif mode == 8: accumulator, destination = 0, 1
            elif mode == 9: accumulator, destination = 1, 0
            elif mode == 10: accumulator, destination = 0x0F, 1
            elif mode == 11: accumulator, destination = 0x10, 1
            elif mode == 12: accumulator, destination = 0x7FFFFFFFFFFFFFFF, MASK64
            elif mode == 13: accumulator, destination = 0x8000000000000000, 1
            elif mode == 14: accumulator, destination = 0x8000000000000000, 0x7FFFFFFFFFFFFFFF
            else: accumulator, destination = 0x7FFFFFFFFFFFFFFF, 0x8000000000000000
            code = CODE_D9 if (seq + si) % 2 == 0 else CODE_C9
            gprs = [rng.getrandbits(64) for _ in range(16)]
            gprs[0], gprs[1] = accumulator, destination
            gprs[15] = 0x4274000000000000 | (si << 32) | seq
            flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
            item = fixture(generated_id(si, seq), code, gprs, flags,
                           f"Deterministic S2-42v corpus input seed={hx(seed)}, sequence={seq}; serialization uses the integer-only expected-state oracle.", seed)
            initial = item["initial_state"]
            unique.add((item["memory"][0]["initial_bytes_hex"], tuple(initial["gpr"]), initial["rflags"]))
            result.append(item)
    if len(unique) != GENERATED_COUNT:
        raise CorpusError("generated architectural input states are not unique")
    return result


def arithmetic_flags(a: int, b: int) -> int:
    # Independent mathematical subtraction, with x86 CF=borrow.
    result = (a - b) & MASK64
    cf = int(a < b)
    pf = int((result & 0xff).bit_count() % 2 == 0)
    af = int(((a ^ b ^ result) & 0x10) != 0)
    zf, sf = int(result == 0), (result >> 63) & 1
    of = int((((a ^ b) & (a ^ result)) >> 63) & 1)
    return cf | (pf << 2) | (af << 4) | (zf << 6) | (sf << 7) | (of << 11)


def expected_snapshot(item: dict[str, Any]) -> dict[str, Any]:
    """Standalone Python integer oracle; no decoder or emulator helper is imported."""
    code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
    if code not in (CODE_D9, CODE_C9):
        raise CorpusError("oracle rejects bytes outside the two exact S2-21c encodings")
    initial = item["initial_state"]
    gprs = [int(x, 16) for x in initial["gpr"]]
    a, dest, src = gprs[0], gprs[1], gprs[3]
    equal = a == dest
    gprs[1] = src if equal and code == CODE_D9 else (dest if not equal else dest)
    if not equal:
        gprs[0] = dest
    flags = int(initial["rflags"], 16)
    new_flags = (flags & ~ARITHMETIC_FLAGS) | arithmetic_flags(a, dest)
    return {"case_id": item["case_id"],
            "state": {"mode": "long64", "gpr": [hx(v) for v in gprs],
                      "rip": hx(RIP + len(code)), "rflags": hx(new_flags),
                      "rflags_defined": hx(RFLAGS_DEFINED)},
            "memory": [{"region_id": "code", "guest_address": hx(RIP), "bytes_hex": code.hex()}],
            "outcome": {"kind": "stop_address"},
            "instructions_executed": {"status": "available", "value": hx(1)}, "exception": None}


def load_controls():
    if not CONTROL_DIR.is_dir() or CONTROL_DIR.is_symlink(): raise CorpusError("S2-42v control directory missing or unsafe")
    if any(path.is_symlink() or not path.is_file() for path in (CONTROL_MANIFEST, CONTROL_EXPECTED)):
        raise CorpusError("S2-42v static control documents are missing or unsafe")
    if CONTROL_MANIFEST_SHA256 == "TO_BE_PINNED" or sha256(CONTROL_MANIFEST.read_bytes()) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned S2-42v control manifest digest differs")
    manifest_raw = CONTROL_MANIFEST.read_bytes()
    manifest = json.loads(manifest_raw, object_pairs_hook=strict_object)
    expected_raw = CONTROL_EXPECTED.read_bytes()
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
            or manifest.get("schema_version") != 1 or manifest.get("profile") != PROFILE
            or manifest.get("control_count") != CONTROL_COUNT or not isinstance(manifest.get("fixtures"), list)
            or len(manifest["fixtures"]) != CONTROL_COUNT or sha256(expected_raw) != manifest.get("expected_sha256")):
        raise CorpusError("S2-42v static control snapshots or manifest are invalid")
    expected_doc = json.loads(expected_raw, object_pairs_hook=strict_object)
    if not isinstance(expected_doc, dict) or set(expected_doc) != {"profile", "cases"} or expected_doc.get("profile") != PROFILE or not isinstance(expected_doc.get("cases"), list) or len(expected_doc["cases"]) != CONTROL_COUNT:
        raise CorpusError("S2-42v static expected-state document has invalid shape")
    expected_by_id = {x["case_id"]: x for x in expected_doc["cases"]}
    if len(expected_by_id) != CONTROL_COUNT or list(expected_by_id) != [control_id(i) for i in range(CONTROL_COUNT)]:
        raise CorpusError("S2-42v frozen expected states are duplicated, missing, or out of order")
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
            raise CorpusError(f"S2-42v frozen control changed: {identifier}")
        controls.append((identifier, raw, expected))
    entries = list(CONTROL_DIR.iterdir())
    if any(x.is_symlink() or not x.is_file() for x in entries) or {x.name for x in entries} != names:
        raise CorpusError("S2-42v controls have missing, extra, or symlink entries")
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
    if directory.is_symlink() or not directory.is_dir(): raise CorpusError("S2-42v corpus directory missing or unsafe")
    files = materialize()
    verify_file_set(directory, files)
    manifest = load_json(directory / "manifest.json")
    if set(manifest) != MANIFEST_FIELDS or manifest["case_count"] != TOTAL_COUNT or len(manifest["case_digests"]) != TOTAL_COUNT:
        raise CorpusError("S2-42v manifest shape or accounting differs")
    return manifest


def validate_provenance(build, machine, label):
    if not isinstance(build, dict) or any(not isinstance(build.get(k), str) or not build[k] for k in ("revision", "compiler", "target", "profile")): raise CorpusError(f"{label}: incomplete build provenance")
    if not isinstance(machine, dict) or any(not isinstance(machine.get(k), str) or not machine[k] for k in ("host_os", "host_arch")): raise CorpusError(f"{label}: incomplete machine provenance")


def compare_result(result, raw, expected, build, machine, label):
    if not isinstance(result, dict): raise CorpusError(f"{label}: result is not an object")
    wanted = {"schema_version": 2, "case_id": expected["case_id"], "input_sha256": sha256(raw), "profile": PROFILE,
              "backend": "interpreter", "outcome": expected["outcome"], "instructions_executed": expected["instructions_executed"], "build": build, "machine": machine}
    if any(result.get(k) != v for k, v in wanted.items()): raise CorpusError(f"{label}: result identity differs from oracle")
    validate_provenance(result.get("build"), result.get("machine"), label)
    snap = result.get("snapshot")
    if not isinstance(snap, dict) or snap.get("status") != "available" or not isinstance(snap.get("value"), dict): raise CorpusError(f"{label}: complete snapshot unavailable")
    actual = snap["value"]
    state = expected["state"]
    if actual.get("state") != {k: state[k] for k in ("mode", "gpr", "rip", "rflags")} or actual.get("rflags_defined") != state["rflags_defined"]: raise CorpusError(f"{label}: complete GPR/RIP/RFLAGS differs")
    if actual.get("memory") != expected["memory"]: raise CorpusError(f"{label}: code bytes differ")
    if actual.get("exception") != {"status": "available", "value": None}: raise CorpusError(f"{label}: exception observation missing or nonempty")


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


def verify_results(directory, result_path, executable):
    manifest = verify_corpus(directory)
    result = load_json(result_path)
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
    if not isinstance(rows, list) or [x.get("case_id") for x in rows] != expected_ids(manifest): raise CorpusError("saved result order/count differs")
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
            if parsed != observed: raise CorpusError(f"saved {ident} repeat {i}: stdout/result differs")
            compare_result(observed, raw, exp[ident], build, machine, f"saved {ident} repeat {i}")
        if row["repeats"][0] != row["repeats"][1]: raise CorpusError(f"saved {ident}: repeats disagree")


def write_native_blocked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": "blocked", "profile": PROFILE,
        "native_x86": "not_run_form_not_admitted", "blocked_reason": "S2-42v denies native execution; these local-only CMPXCHG register encodings are not admitted to the native backend.", "backend_execution": "not_started"}))


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
            write_file_set(args.corpus, materialize()); print(f"generated {TOTAL_COUNT} S2-42v fixtures")
        elif args.verify:
            m = verify_corpus(args.corpus); print(f"verified {TOTAL_COUNT} S2-42v fixtures; digest {m['corpus_digest']}")
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
        print(f"S2-42v failed closed: {e}", file=sys.stderr); return 1
    return 0


if __name__ == "__main__": raise SystemExit(main())
