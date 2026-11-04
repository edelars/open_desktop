#!/usr/bin/env python3
"""Generate and strictly replay the local-only S2-42ac CDQE/CQO corpus."""
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
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42ac_sign_extension_controls"
CORPUS_DIR = ROOT / "work/stage2/s2-42ac-sign-extension-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42ac-sign-extension-corpus/interpreter-results.json"
NATIVE_PATH = ROOT / "work/stage2/s2-42ac-sign-extension-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42ac-sign-extension-v1"
RIP = 0x00100000
ENCODINGS = {"cdqe": bytes.fromhex("4898"), "cqo": bytes.fromhex("4899")}
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
CONTROL_INPUTS = {
    "cdqe": (0, 1, 0x7FFFFFFF, 0x80000000, 0xFFFFFFFF, 0x1234567880000000,
             0x800000007FFFFFFF, 0xFEDCBA9876543210, 0x8000000080000000,
             0x000000007FFFFFFF, 0x0000000080000000, 0xFFFFFFFF80000001,
             0x5555555580000000, 0xAAAAAAAA7FFFFFFF, 0xFFFFFFFF00000000,
             0x0000000100000000),
    "cqo": (0, 1, 0x7FFFFFFFFFFFFFFF, 0x8000000000000000, 0xFFFFFFFFFFFFFFFF,
            0x0123456789ABCDEF, 0xAAAAAAAAAAAAAAAA, 0x5555555555555555,
            0x7FFFFFFFFFFFFFFE, 0x8000000000000001, 0x0000000080000000,
            0xFFFFFFFF7FFFFFFF, 0x8000000080000000, 0x00000000FFFFFFFF,
            0xFEDCBA9800000000, 0x0123456780000000),
}
CONTROL_COUNT = sum(map(len, CONTROL_INPUTS.values()))
GENERATED_COUNT = len(ENCODINGS) * len(SEEDS) * CASES_PER_SEED
TOTAL_COUNT = CONTROL_COUNT + GENERATED_COUNT
REPEATS = 2
MASK32 = 0xFFFFFFFF
MASK64 = 0xFFFFFFFFFFFFFFFF
RFLAGS_DEFINED = 0x0CD7
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12
CONTROL_MANIFEST_SHA256 = "8737471b11977eec1608ab7c42dde1fb93505d7190309fe4129bb26dcb8360d2"
MANIFEST_FIELDS = {"manifest_version", "generator_version", "profile", "seeds", "cases_per_seed",
                   "control_count", "generated_case_count", "case_count", "case_digests",
                   "control_manifest_sha256", "expected_sha256", "corpus_digest"}


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def fixture(identifier: str, opname: str, gprs: list[int], flags: int, source: str,
            seed: int | None = None) -> dict[str, Any]:
    code = ENCODINGS[opname]
    if len(gprs) != 16 or any(type(x) is not int or not 0 <= x <= MASK64 for x in gprs):
        raise CorpusError("input is outside the exact S2-42ac GPR profile")
    if type(flags) is not int or flags & ~RFLAGS_DEFINED or not flags & 2:
        raise CorpusError("initial RFLAGS contain reserved bits or omit fixed bit 1")
    return {"schema_version": 2, "case_id": identifier, "profile": PROFILE,
            "provenance": {"kind": "project_fixture", "source": source,
                "specification": ("Intel SDM revision 093, Vol. 2A: exact long64 bytes "
                    f"{code.hex(' ').upper()} ({opname.upper()}). CDQE sign-extends EAX to RAX; "
                    "CQO fills RDX with the sign bit of RAX. These exact opcode forms leave flags unchanged."),
                "specification_version": "Intel SDM revision 093; project S2-42ac"},
            "seed": None if seed is None else hx(seed), "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gprs],
                              "rip": hx(RIP), "rflags": hx(flags)},
            "memory": [{"region_id": "code", "guest_address": hx(RIP), "length": len(code),
                        "permissions": {"read": False, "write": False, "execute": True},
                        "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)}],
            "stop_rip": hx(RIP + len(code)), "instruction_budget": 1,
            "wall_timeout_ms": 2000, "rflags_mask": hx(RFLAGS_DEFINED),
            "expected_exit": "stop_address"}


def control_id(opname: str, index: int) -> str:
    return f"s2_42ac_{opname}_c{index:02d}"


def generated_id(opname: str, seed_index: int, sequence: int) -> str:
    return f"s2_42ac_{opname}_g{seed_index}_{sequence:03d}"


def control_inputs() -> dict[str, bytes]:
    files = {}
    for opname, values in CONTROL_INPUTS.items():
        for i, value in enumerate(values):
            gprs = [((j + 1) * 0x102030405060708) & MASK64 for j in range(16)]
            gprs[0] = value
            gprs[2] = value ^ 0xA5A5A5A55A5A5A5A
            flags = (2 | (0x8D5 if i & 1 else 0x44)) & RFLAGS_DEFINED
            ident = control_id(opname, i)
            item = fixture(ident, opname, gprs, flags,
                "Pinned S2-42ac control: exact opcode bytes and architectural input are defined in generator source.")
            files[f"{ident}.case.json"] = json_bytes(item)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    cases, unique = [], set()
    for opname in ENCODINGS:
        for si, seed in enumerate(SEEDS):
            for seq in range(CASES_PER_SEED):
                rng = random.Random(seed ^ (seq * 0x9E3779B97F4A7C15) ^ (0x42AC << (si + 1))
                                    ^ (0xCD if opname == "cdqe" else 0xC0))
                gprs = [rng.getrandbits(64) for _ in range(16)]
                # Ensure both extension outcomes and arbitrary preexisting destination halves.
                if seq % 4 == 0:
                    gprs[0] = ((rng.getrandbits(32) | (0x80000000 if (seq // 4) & 1 else 0)) << 32) | rng.getrandbits(32)
                flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
                ident = generated_id(opname, si, seq)
                item = fixture(ident, opname, gprs, flags,
                    f"Deterministic S2-42ac input seed=0x{seed:016x}, sequence={seq}; integer-only oracle.", seed)
                key = (opname, tuple(item["initial_state"]["gpr"]), item["initial_state"]["rflags"])
                unique.add(key)
                cases.append(item)
    if len(unique) != GENERATED_COUNT:
        raise CorpusError("generated instruction/input tuples are not unique")
    return cases


def expected_snapshot(item: dict[str, Any]) -> dict[str, Any]:
    """Independent bit-vector oracle; imports no decoder, emulator, or CPU helper."""
    code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
    opname = next((name for name, encoding in ENCODINGS.items() if code == encoding), None)
    if opname is None or item.get("profile") != PROFILE:
        raise CorpusError("oracle rejects bytes outside exact S2-42ac CDQE/CQO forms")
    initial = item["initial_state"]
    gprs = [int(x, 16) for x in initial["gpr"]]
    if opname == "cdqe":
        low = gprs[0] & MASK32
        gprs[0] = low | (0xFFFFFFFF00000000 if low & 0x80000000 else 0)
    else:
        gprs[2] = MASK64 if gprs[0] & (1 << 63) else 0
    flags = int(initial["rflags"], 16)
    return {"case_id": item["case_id"], "state": {"mode": "long64", "gpr": [hx(x) for x in gprs],
            "rip": hx(RIP + 2), "rflags": hx(flags), "rflags_defined": hx(RFLAGS_DEFINED)},
            "memory": [{"region_id": "code", "guest_address": hx(RIP), "bytes_hex": code.hex()}],
            "outcome": {"kind": "stop_address"},
            "instructions_executed": {"status": "available", "value": hx(1)}, "exception": None}


def load_controls() -> tuple[list[tuple[str, bytes, dict[str, Any]]], bytes]:
    manifest_path, expected_path = CONTROL_DIR / "controls_manifest.json", CONTROL_DIR / "expected.json"
    if CONTROL_DIR.is_symlink() or not CONTROL_DIR.is_dir():
        raise CorpusError("S2-42ac control directory missing or unsafe")
    raw_manifest, raw_expected = manifest_path.read_bytes(), expected_path.read_bytes()
    if CONTROL_MANIFEST_SHA256 == "TO_BE_PINNED" or sha256(raw_manifest) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned S2-42ac control manifest digest differs")
    manifest = json.loads(raw_manifest, object_pairs_hook=strict_object)
    expected_doc = json.loads(raw_expected, object_pairs_hook=strict_object)
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
            or type(manifest.get("schema_version")) is not int or manifest.get("schema_version") != 1
            or manifest.get("profile") != PROFILE or type(manifest.get("control_count")) is not int
            or manifest.get("control_count") != CONTROL_COUNT or not isinstance(manifest.get("fixtures"), list)
            or len(manifest["fixtures"]) != CONTROL_COUNT or not isinstance(expected_doc, dict)
            or sha256(raw_expected) != manifest.get("expected_sha256")
            or set(expected_doc) != {"profile", "cases"} or expected_doc.get("profile") != PROFILE
            or not isinstance(expected_doc.get("cases"), list) or len(expected_doc["cases"]) != CONTROL_COUNT):
        raise CorpusError("S2-42ac frozen controls or expected snapshots have invalid shape")
    if any(not isinstance(x, dict) or not isinstance(x.get("case_id"), str) for x in expected_doc["cases"]):
        raise CorpusError("S2-42ac expected snapshots have invalid identifiers")
    expected_by_id = {x["case_id"]: x for x in expected_doc["cases"]}
    ordered_ids = [control_id(op, i) for op in ENCODINGS for i in range(len(CONTROL_INPUTS[op]))]
    if list(expected_by_id) != ordered_ids:
        raise CorpusError("S2-42ac expected controls are duplicated, missing, or out of order")
    fixed = control_inputs()
    rows, names = [], {"controls_manifest.json", "expected.json"}
    i = 0
    for op in ENCODINGS:
        for j in range(len(CONTROL_INPUTS[op])):
            ident = control_id(op, j)
            name = f"{ident}.case.json"; names.add(name)
            raw = (CONTROL_DIR / name).read_bytes()
            row = manifest["fixtures"][i]; expected = expected_by_id[ident]
            parsed = json.loads(raw, object_pairs_hook=strict_object)
            if (raw != fixed[name] or row != {"case_id": ident, "fixture_sha256": sha256(raw),
                    "expected_sha256": sha256(json_bytes(expected))} or expected != expected_snapshot(parsed)):
                raise CorpusError(f"S2-42ac pinned control changed: {ident}")
            rows.append((ident, raw, expected)); i += 1
    entries = list(CONTROL_DIR.iterdir())
    if {x.name for x in entries} != names or any(x.is_symlink() or not x.is_file() for x in entries):
        raise CorpusError("S2-42ac controls have missing, extra, or unsafe entries")
    return rows, raw_manifest


def all_cases():
    controls, control_raw = load_controls()
    rows = [(ident, raw, "handwritten", exp) for ident, raw, exp in controls]
    for item in generated_fixtures():
        raw = json_bytes(item)
        rows.append((item["case_id"], raw, "generated", expected_snapshot(item)))
    return rows, control_raw


def materialize() -> dict[str, bytes]:
    rows, _ = all_cases(); files = {}; digests = []; snapshots = []
    for ident, raw, kind, expected in rows:
        files[f"{ident}.case.json"] = raw
        snapshots.append(expected)
        digests.append({"case_id": ident, "kind": kind, "fixture_sha256": sha256(raw),
                        "expected_sha256": sha256(json_bytes(expected))})
    expected_raw = json_bytes({"profile": PROFILE, "cases": snapshots})
    files["expected.json"] = expected_raw
    files["manifest.json"] = json_bytes({"manifest_version": 1, "generator_version": GENERATOR_VERSION,
        "profile": PROFILE, "seeds": [f"{x:016x}" for x in SEEDS], "cases_per_seed": CASES_PER_SEED,
        "control_count": CONTROL_COUNT, "generated_case_count": GENERATED_COUNT, "case_count": TOTAL_COUNT,
        "case_digests": digests, "control_manifest_sha256": CONTROL_MANIFEST_SHA256,
        "expected_sha256": sha256(expected_raw),
        "corpus_digest": sha256("\n".join(x["fixture_sha256"] for x in digests).encode())})
    return files


def verify_corpus(directory: Path):
    if directory.is_symlink() or not directory.is_dir(): raise CorpusError("corpus directory missing or unsafe")
    verify_file_set(directory, materialize())
    manifest = load_json(directory / "manifest.json")
    if set(manifest) != MANIFEST_FIELDS or manifest["case_count"] != TOTAL_COUNT or len(manifest["case_digests"]) != TOTAL_COUNT:
        raise CorpusError("manifest shape or accounting differs")
    return manifest


def validate_provenance(build, machine, label):
    if not isinstance(build, dict) or any(not isinstance(build.get(k), str) or not build[k] for k in ("revision", "compiler", "target", "profile")):
        raise CorpusError(f"{label}: incomplete build provenance")
    if not isinstance(machine, dict) or any(not isinstance(machine.get(k), str) or not machine[k] for k in ("host_os", "host_arch")):
        raise CorpusError(f"{label}: incomplete machine provenance")


def compare_result(result, raw, expected, build, machine, label):
    if (not isinstance(result, dict) or set(result) != {"schema_version", "case_id", "input_sha256", "profile",
            "backend", "outcome", "instructions_executed", "build", "machine", "snapshot"}
            or type(result.get("schema_version")) is not int):
        raise CorpusError(f"{label}: result/schema is invalid")
    wanted = {"schema_version": 2, "case_id": expected["case_id"], "input_sha256": sha256(raw),
              "profile": PROFILE, "backend": "interpreter", "outcome": expected["outcome"],
              "instructions_executed": expected["instructions_executed"], "build": build, "machine": machine}
    if any(result.get(k) != v for k, v in wanted.items()): raise CorpusError(f"{label}: identity or outcome differs")
    validate_provenance(result.get("build"), result.get("machine"), label)
    snap = result.get("snapshot")
    if (not isinstance(snap, dict) or set(snap) != {"status", "value"} or snap.get("status") != "available"
            or not isinstance(snap.get("value"), dict)):
        raise CorpusError(f"{label}: complete snapshot unavailable")
    actual = snap["value"]; state = expected["state"]
    if (set(actual) != {"state", "rflags_defined", "memory", "exception",
                        "minss_memory_rax_controls", "addss_memory_rax_controls"}
            or not isinstance(actual.get("state"), dict)):
        raise CorpusError(f"{label}: snapshot shape differs")
    if set(actual["state"]) != {"mode", "gpr", "rip", "rflags"}:
        raise CorpusError(f"{label}: CPU state shape differs")
    if actual.get("state") != {k: state[k] for k in ("mode", "gpr", "rip", "rflags")} or actual.get("rflags_defined") != state["rflags_defined"]:
        raise CorpusError(f"{label}: GPRs, flags/definedness, or RIP differs")
    if actual.get("memory") != expected["memory"]: raise CorpusError(f"{label}: code memory differs")
    if actual["minss_memory_rax_controls"] is not None or actual["addss_memory_rax_controls"] is not None:
        raise CorpusError(f"{label}: unrelated SIMD memory controls are nonempty")
    if actual.get("exception") != {"status": "available", "value": None}: raise CorpusError(f"{label}: exception observation differs")


def expected_ids(manifest): return [x["case_id"] for x in manifest["case_digests"]]


def missing_attempts(ids, rows):
    byid = {x.get("case_id"): x for x in rows if isinstance(x, dict)}; out = []
    for ident in ids:
        row = byid.get(ident, {})
        for repeat in range(REPEATS):
            ps, rs = row.get("processes", []), row.get("repeats", [])
            if repeat >= len(ps) or not isinstance(ps[repeat], dict) or ps[repeat].get("returncode") != 0 or repeat >= len(rs) or "failure" in rs[repeat]:
                out.append({"case_id": ident, "repeat": repeat + 1})
    return out


def process_environment(): return {key: os.environ.get(key) for key in PROCESS_ENV_KEYS}
def output_text(value): return value.decode("utf-8", "replace") if isinstance(value, bytes) else (value or "")


def command_identity(executable):
    executable = executable.resolve(strict=True)
    revision = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
    osname = {"darwin": "macos"}.get(platform.system().lower(), platform.system().lower())
    arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(platform.machine().lower(), platform.machine().lower())
    return {"executable_sha256": sha256(executable.read_bytes()), "revision": revision}, {"host_os": osname, "host_arch": arch}


def write_checkpoint(path, directory, manifest, rows, failures, identity, machine, build, runner_machine, status="incomplete"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": status, "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "manifest_sha256": sha256((directory / "manifest.json").read_bytes()), "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "execution_identity": identity, "execution_machine": machine,
        "runner_build": build, "runner_machine": runner_machine, "results": rows, "failures": failures,
        "missing_attempts": missing_attempts(expected_ids(manifest), rows)}))


def run_interpreter(directory, result_path, executable):
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_bytes(json_bytes({"schema_version": 1, "status": "incomplete", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE, "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "results": [], "failures": ["run in progress or preflight incomplete"],
        "missing_attempts": missing_attempts([x["case_id"] for x in materialize_manifest_ids()], [])}))
    manifest = verify_corpus(directory); identity, machine = command_identity(executable)
    expected = {x["case_id"]: x for x in load_json(directory / "expected.json")["cases"]}
    rows, failures, build, runner_machine = [], [], None, None
    env = process_environment(); child_env = {k: v for k, v in env.items() if v is not None}
    for entry in manifest["case_digests"]:
        ident = entry["case_id"]; fixture_path = (directory / f"{ident}.case.json").resolve(strict=True); raw = fixture_path.read_bytes()
        row = {"case_id": ident, "fixture_sha256": sha256(raw), "repeats": [], "processes": []}; rows.append(row)
        for repeat in range(REPEATS):
            cmd = [str(executable.resolve(strict=True)), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"]
            try:
                p = subprocess.run(cmd, capture_output=True, text=True, check=False, timeout=RUNNER_TIMEOUT_SECONDS,
                                   cwd=str(ROOT.resolve()), env=child_env)
                proc = {"returncode": p.returncode, "stdout": p.stdout, "stderr": p.stderr, "command": cmd,
                        "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None}; row["processes"].append(proc)
                if p.returncode: raise CorpusError(f"runner exited {p.returncode}: {p.stderr.strip()}")
                obs = json.loads(p.stdout, object_pairs_hook=strict_object)
                if not isinstance(obs, dict): raise CorpusError("runner output must be one JSON object")
                b, m = obs.get("build"), obs.get("machine"); validate_provenance(b, m, "interpreter result")
                if b["revision"] != identity["revision"] or {k: m[k] for k in ("host_os", "host_arch")} != machine:
                    raise CorpusError("runner source revision or host provenance differs")
                if build is None: build, runner_machine = b, m
                if b != build or m != runner_machine: raise CorpusError("runner provenance changed during run")
                compare_result(obs, raw, expected[ident], build, runner_machine, f"{ident} repeat {repeat+1}")
                row["repeats"].append(obs)
            except KeyboardInterrupt as e:
                row["processes"].append({"returncode": None, "stdout": "", "stderr": "interrupted", "command": cmd,
                    "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": "interrupted", "returncode": None}); failures.append(f"{ident} repeat {repeat+1}: interrupted")
                write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
                raise CorpusError(f"interpreter run interrupted; checkpoint saved: {result_path}") from e
            except subprocess.TimeoutExpired as e:
                msg = str(e); row["processes"].append({"returncode": None, "stdout": output_text(e.stdout), "stderr": output_text(e.stderr),
                    "command": cmd, "cwd": str(ROOT.resolve()), "environment": env,
                    "timeout_diagnosis": {"kind": "timeout_expired", "timeout_seconds": RUNNER_TIMEOUT_SECONDS, "message": msg}})
                row["repeats"].append({"failure": msg, "returncode": None}); failures.append(f"{ident} repeat {repeat+1}: timed out: {msg}")
            except (OSError, subprocess.SubprocessError, json.JSONDecodeError, CorpusError) as e:
                if len(row["processes"]) == repeat:
                    row["processes"].append({"returncode": None, "stdout": "", "stderr": str(e), "command": cmd,
                        "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": str(e), "returncode": row["processes"][-1]["returncode"]})
                failures.append(f"{ident} repeat {repeat+1}: {e}")
        if len(row["repeats"]) == 2 and all("failure" not in x for x in row["repeats"]) and row["repeats"][0] != row["repeats"][1]:
            failures.append(f"{ident}: repeat results differ")
        write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
    status = "failed" if failures else "passed"
    write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine, status)
    if failures: raise CorpusError(f"{len(failures)} interpreter failures; saved {result_path}")
    return load_json(result_path)


def materialize_manifest_ids():
    return [{"case_id": x[0]} for x in all_cases()[0]]


def verify_results(directory, result_path, executable):
    manifest = verify_corpus(directory); result = load_json(result_path)
    result_fields = {"schema_version", "status", "backend", "native_x86", "profile", "manifest_sha256",
                     "case_count", "repeats_per_case", "execution_identity", "execution_machine",
                     "runner_build", "runner_machine", "results", "failures", "missing_attempts"}
    if (not isinstance(result, dict) or set(result) != result_fields
            or type(result.get("schema_version")) is not int or result.get("schema_version") != 1
            or result.get("status") != "passed" or result.get("backend") != "interpreter"
            or result.get("native_x86") != "not_run_form_not_admitted" or result.get("profile") != PROFILE
            or result.get("failures") != [] or result.get("missing_attempts") != []
            or result.get("manifest_sha256") != sha256((directory / "manifest.json").read_bytes())
            or type(result.get("case_count")) is not int or result["case_count"] != TOTAL_COUNT
            or type(result.get("repeats_per_case")) is not int or result["repeats_per_case"] != REPEATS):
        raise CorpusError("saved evidence does not match successful corpus identity")
    build, machine = result.get("runner_build"), result.get("runner_machine"); validate_provenance(build, machine, "saved results")
    identity = result.get("execution_identity"); selected = executable.resolve(strict=True)
    current_identity, current_machine = command_identity(selected)
    if (not isinstance(identity, dict) or set(identity) != {"executable_sha256", "revision"}
            or identity != current_identity or identity.get("revision") != build["revision"]):
        raise CorpusError("saved runner/source provenance differs")
    if (result.get("execution_machine") != {k: machine[k] for k in ("host_os", "host_arch")}
            or current_machine != result.get("execution_machine")):
        raise CorpusError("saved machine provenance differs")
    rows = result.get("results")
    if (not isinstance(rows, list) or any(not isinstance(x, dict) or set(x) != {"case_id", "fixture_sha256", "repeats", "processes"} for x in rows)
            or [x.get("case_id") for x in rows] != expected_ids(manifest)):
        raise CorpusError("saved result order/count differs")
    expected = {x["case_id"]: x for x in load_json(directory / "expected.json")["cases"]}; env0 = None
    for row in rows:
        ident = row["case_id"]; raw = (directory / f"{ident}.case.json").read_bytes(); repeats, processes = row.get("repeats"), row.get("processes")
        if row.get("fixture_sha256") != sha256(raw) or not isinstance(repeats, list) or not isinstance(processes, list) or len(repeats) != 2 or len(processes) != 2:
            raise CorpusError(f"saved {ident}: fixture/repeat count differs")
        for i, (obs, proc) in enumerate(zip(repeats, processes), 1):
            if (not isinstance(proc, dict) or set(proc) != {"returncode", "stdout", "stderr", "command", "cwd", "environment", "timeout_diagnosis"}
                    or type(proc.get("returncode")) is not int or proc["returncode"] != 0
                    or not isinstance(proc.get("stdout"), str) or not isinstance(proc.get("stderr"), str)
                    or proc.get("timeout_diagnosis") is not None):
                raise CorpusError(f"saved {ident} repeat {i}: invalid process record")
            path = str((directory / f"{ident}.case.json").resolve(strict=True))
            if proc.get("command") != [str(selected), "--fixture", path, "--engine", "interpreter", "--json"] or proc.get("cwd") != str(ROOT.resolve()):
                raise CorpusError(f"saved {ident} repeat {i}: replay command/cwd differs")
            env = proc.get("environment")
            if (not isinstance(env, dict) or set(env) != set(PROCESS_ENV_KEYS)
                    or any(v is not None and not isinstance(v, str) for v in env.values())
                    or (env0 is not None and env != env0)):
                raise CorpusError("saved environment provenance differs")
            env0 = env
            if json.loads(proc["stdout"], object_pairs_hook=strict_object) != obs: raise CorpusError("saved stdout/result differs")
            compare_result(obs, raw, expected[ident], build, machine, f"saved {ident} repeat {i}")
        if repeats[0] != repeats[1]: raise CorpusError(f"saved {ident}: repeats disagree")


def write_native_blocked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": "blocked", "profile": PROFILE,
        "native_x86": "not_run_form_not_admitted", "blocked_reason": "S2-42ac exact CDQE/CQO encodings are not admitted to the native backend.",
        "backend_execution": "not_started"}))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    for action in ("generate", "verify", "run-interpreter", "verify-results", "native"):
        group.add_argument("--" + action, action="store_true")
    parser.add_argument("--corpus", type=Path, default=CORPUS_DIR); parser.add_argument("--results", type=Path)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner")
    args = parser.parse_args(argv)
    try:
        if args.generate:
            write_file_set(args.corpus, materialize()); print(f"generated {TOTAL_COUNT} S2-42ac fixtures")
        elif args.verify:
            m = verify_corpus(args.corpus); print(f"verified {TOTAL_COUNT} fixtures; digest {m['corpus_digest']}")
        elif args.run_interpreter:
            path = args.results or RESULTS_PATH
            try: run_interpreter(args.corpus, path, args.runner)
            except (CorpusError, OSError, subprocess.SubprocessError, json.JSONDecodeError) as e:
                try: saved = load_json(path)
                except Exception: saved = {}
                saved.update({"schema_version": 1, "status": "failed", "backend": "interpreter", "native_x86": "not_run_form_not_admitted",
                    "profile": PROFILE, "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS, "error": str(e)})
                saved.setdefault("failures", []).append(str(e)); path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(json_bytes(saved)); raise
            print(f"passed {TOTAL_COUNT} cases × 2 interpreter runs")
        elif args.verify_results:
            verify_results(args.corpus, args.results or RESULTS_PATH, args.runner); print("verified saved interpreter evidence")
        else:
            write_native_blocked(args.results or NATIVE_PATH); print("native denied before backend execution"); return 1
    except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError) as e:
        print(f"S2-42ac failed closed: {e}", file=sys.stderr); return 1
    return 0


if __name__ == "__main__": raise SystemExit(main())
