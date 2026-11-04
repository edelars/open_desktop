#!/usr/bin/env python3
"""Build and strictly replay the local-only S2-42z near JZ/JNZ rel32 corpus."""
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
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42z_jcc_rel32_controls"
CONTROL_EXPECTED = CONTROL_DIR / "expected.json"
CONTROL_MANIFEST = CONTROL_DIR / "controls_manifest.json"
CORPUS_DIR = ROOT / "work/stage2/s2-42z-jcc-rel32-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42z-jcc-rel32-corpus/interpreter-results.json"
NATIVE_PATH = ROOT / "work/stage2/s2-42z-jcc-rel32-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42z-jcc-rel32-v1"
CODE_ADDRESS = 0x0000000100000000
RIP = CODE_ADDRESS
SEEDS = (0x4200000000000001, 0x4200000000000002, 0x4200000000000003, 0x4200000000000004)
CASES_PER_SEED = 256
OPCODES = (0x84, 0x85)  # 0F 84 JZ; 0F 85 JNZ
DISPLACEMENTS = (-(1 << 31), -(1 << 31) + 1, -129, -1, 1, 127, 128, (1 << 31) - 1)
CONTROL_COUNT = len(OPCODES) * 2 * len(DISPLACEMENTS)
GENERATED_COUNT = len(SEEDS) * CASES_PER_SEED
TOTAL_COUNT = CONTROL_COUNT + GENERATED_COUNT
REPEATS = 2
MASK64 = 0xFFFFFFFFFFFFFFFF
RFLAGS_DEFINED = 0x0CD7
MANIFEST_FIELDS = {"manifest_version", "generator_version", "profile", "code_address", "opcodes", "displacements", "seeds", "cases_per_seed", "control_count", "generated_case_count", "case_count", "case_digests", "control_manifest_sha256", "expected_sha256", "corpus_digest"}
OBSERVATION_FIELDS = {"schema_version", "case_id", "input_sha256", "profile", "backend", "outcome", "instructions_executed", "build", "machine", "snapshot"}
SNAPSHOT_FIELDS = {"status", "value"}
SNAPSHOT_VALUE_FIELDS = {"state", "rflags_defined", "memory", "exception", "addss_memory_rax_controls", "minss_memory_rax_controls"}
SUMMARY_FIELDS = {"schema_version", "status", "backend", "native_x86", "profile", "manifest_sha256", "case_count", "repeats_per_case", "execution_identity", "execution_machine", "runner_build", "runner_machine", "results", "failures", "missing_attempts"}
CASE_RESULT_FIELDS = {"case_id", "fixture_sha256", "repeats", "processes"}
BUILD_FIELDS = {"revision", "compiler", "target", "profile"}
MACHINE_FIELDS = {"host_os", "host_arch", "cpu_model", "kernel", "page_size"}
CONTROL_MANIFEST_SHA256 = "cf6e94d8e4435cb6f313191307507f7053b55c0e6b1aeb37f773e1c4248305e4"
CONTROL_EXPECTED_SHA256 = "4484831504241ef1f5c8f2a7b4db6ee41275ff348f5cf1da791d75fd2fe8e65c"
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def control_id(opcode: int, zf: int, displacement: int) -> str:
    return f"s2_42z_jcc_rel32_{opcode:02x}_zf{zf}_{displacement & 0xffffffff:08x}"


def control_ids() -> list[str]:
    return [control_id(opcode, zf, displacement) for opcode in OPCODES
            for zf in (0, 1) for displacement in DISPLACEMENTS]


def generated_id(seed_index: int, sequence: int) -> str:
    return f"s2_42z_jcc_rel32_g{seed_index}_{sequence:03d}"


def fixture(identifier: str, opcode: int, zf: int, displacement: int, gprs: list[int], flags: int,
            source: str, seed: int | None = None) -> dict[str, Any]:
    if (opcode not in OPCODES or zf not in (0, 1) or displacement not in DISPLACEMENTS
            or len(gprs) != 16 or any(type(x) is not int or not 0 <= x <= MASK64 for x in gprs)):
        raise CorpusError("input is outside the exact S2-42z near JZ/JNZ rel32 profile")
    if flags & ~RFLAGS_DEFINED or not flags & 2 or ((flags >> 6) & 1) != zf:
        raise CorpusError("initial RFLAGS contain reserved bits or do not match fixed bit 1/ZF cell")
    code = bytes((0x0f, opcode)) + (displacement & 0xffffffff).to_bytes(4, "little")
    taken = (opcode == 0x84 and zf == 1) or (opcode == 0x85 and zf == 0)
    stop_rip = RIP + 6 + displacement if taken else RIP + 6
    if not 0 <= stop_rip < (1 << 47):
        raise CorpusError("selected rel32 target is not canonical in the fixed lower-half window")
    return {"schema_version": 2, "case_id": identifier, "profile": PROFILE,
            "provenance": {"kind": "project_fixture", "source": source,
            "specification": "Intel SDM revision 093, Vol. 2A Jcc: near 0F 84/85 rel32 sign-extends the signed 32-bit displacement and adds it to the address after the six-byte instruction; JZ tests ZF=1 and JNZ tests ZF=0.",
            "specification_version": "Intel SDM revision 093; project S2-16a"},
            "seed": None if seed is None else hx(seed), "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gprs], "rip": hx(RIP), "rflags": hx(flags)},
            "memory": [{"region_id": "code", "guest_address": hx(CODE_ADDRESS), "length": len(code),
                        "permissions": {"read": False, "write": False, "execute": True},
                        "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)}],
            "stop_rip": hx(stop_rip), "instruction_budget": 1, "wall_timeout_ms": 2000,
            "rflags_mask": hx(RFLAGS_DEFINED), "expected_exit": "stop_address"}


def control_inputs() -> dict[str, bytes]:
    files = {}
    for opcode in OPCODES:
        for zf in (0, 1):
            for displacement in DISPLACEMENTS:
                gprs = [0x0123456789ABCDEF, 0x1122334455667788, 0xDEADBEEFFEDCBA98,
                        0xAABBCCDDEEFF0011, 0x5566778899AABBCC, 0x8877665544332211,
                        0x0000000000200004, 0x0000000000200010, 0xAABBCCDDEEFF0011,
                        0xFEDCBA9876543210, 0x0123456789ABCDEF, 0x9988776655443322,
                        0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, MASK64]
                flags = 2 | (zf << 6) | (0x8d5 & ~(1 << 6))
                item = fixture(control_id(opcode, zf, displacement), opcode, zf, displacement, gprs, flags,
                               f"Handwritten S2-42z control: opcode=0x{opcode:02x}, ZF={zf}, rel32={displacement}.")
                files[f"{item['case_id']}.case.json"] = json_bytes(item)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    result, unique = [], set()
    for si, seed in enumerate(SEEDS):
        for seq in range(CASES_PER_SEED):
            rng = random.Random(seed ^ ((seq + 1) * 0x9E3779B97F4A7C15))
            gprs = [rng.getrandbits(64) for _ in range(16)]
            gprs[15] = 0x427A000000000000 | (si << 32) | seq
            # 32 opcode × ZF × displacement cells, repeated eight times per seed.
            opcode_index = (seq // (2 * len(DISPLACEMENTS))) % len(OPCODES)
            zf = (seq // len(DISPLACEMENTS)) % 2
            displacement = DISPLACEMENTS[seq % len(DISPLACEMENTS)]
            flags = 2 | (zf << 6) | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~(2 | (1 << 6))))
            item = fixture(generated_id(si, seq), OPCODES[opcode_index], zf, displacement, gprs, flags,
                           f"Deterministic S2-42z input seed={hx(seed)}, sequence={seq}.", seed)
            state = item["initial_state"]
            unique.add((item["memory"][0]["initial_bytes_hex"], tuple(state["gpr"]), state["rip"], state["rflags"]))
            result.append(item)
    if len(unique) != GENERATED_COUNT:
        raise CorpusError("generated architectural input states are not unique")
    return result


def expected_snapshot(item: dict[str, Any]) -> dict[str, Any]:
    """Independent integer oracle for literal opcode, ZF and signed rel32."""
    code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
    if len(code) != 6 or code[0] != 0x0f or code[1] not in OPCODES:
        raise CorpusError("oracle rejects bytes outside exact near JZ/JNZ rel32 forms")
    initial = item["initial_state"]
    flags = int(initial["rflags"], 16)
    unsigned = int.from_bytes(code[2:6], "little")
    displacement = unsigned - (1 << 32) if unsigned & (1 << 31) else unsigned
    zf = bool(flags & (1 << 6))
    taken = (code[1] == 0x84 and zf) or (code[1] == 0x85 and not zf)
    entry = int(initial["rip"], 16)
    target = (entry + 6 + displacement) & MASK64
    if target >= (1 << 47):
        raise CorpusError("oracle target is not canonical in the admitted lower-half window")
    rip = target if taken else entry + 6
    if item.get("stop_rip") != hx(rip):
        raise CorpusError("fixture stop_rip differs from independent rel32 branch calculation")
    return {"case_id": item["case_id"],
            "state": {"mode": "long64", "gpr": list(initial["gpr"]), "rip": hx(rip),
                      "rflags": hx(flags), "rflags_defined": hx(RFLAGS_DEFINED)},
            "memory": [{"region_id": "code", "guest_address": hx(CODE_ADDRESS), "bytes_hex": code.hex()}],
            "outcome": {"kind": "stop_address"},
            "instructions_executed": {"status": "available", "value": hx(1)}, "exception": None}


def load_controls():
    if not CONTROL_DIR.is_dir() or CONTROL_DIR.is_symlink(): raise CorpusError("S2-42z control directory missing or unsafe")
    if any(path.is_symlink() or not path.is_file() for path in (CONTROL_MANIFEST, CONTROL_EXPECTED)):
        raise CorpusError("S2-42z static control documents are missing or unsafe")
    if (CONTROL_MANIFEST_SHA256 == "TO_BE_PINNED"
            or sha256(CONTROL_MANIFEST.read_bytes()) != CONTROL_MANIFEST_SHA256
            or CONTROL_EXPECTED_SHA256 == "TO_BE_PINNED"
            or sha256(CONTROL_EXPECTED.read_bytes()) != CONTROL_EXPECTED_SHA256):
        raise CorpusError("pinned S2-42z control manifest digest differs")
    manifest_raw = CONTROL_MANIFEST.read_bytes()
    manifest = json.loads(manifest_raw, object_pairs_hook=strict_object)
    expected_raw = CONTROL_EXPECTED.read_bytes()
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
            or type(manifest.get("schema_version")) is not int or manifest["schema_version"] != 1
            or manifest.get("profile") != PROFILE
            or type(manifest.get("control_count")) is not int or manifest["control_count"] != CONTROL_COUNT
            or not isinstance(manifest.get("fixtures"), list)
            or len(manifest["fixtures"]) != CONTROL_COUNT or sha256(expected_raw) != manifest.get("expected_sha256")):
        raise CorpusError("S2-42z static control snapshots or manifest are invalid")
    expected_doc = json.loads(expected_raw, object_pairs_hook=strict_object)
    if not isinstance(expected_doc, dict) or set(expected_doc) != {"profile", "cases"} or expected_doc.get("profile") != PROFILE or not isinstance(expected_doc.get("cases"), list) or len(expected_doc["cases"]) != CONTROL_COUNT:
        raise CorpusError("S2-42z static expected-state document has invalid shape")
    expected_by_id = {x["case_id"]: x for x in expected_doc["cases"]}
    ids = control_ids()
    if len(expected_by_id) != CONTROL_COUNT or list(expected_by_id) != ids:
        raise CorpusError("S2-42z frozen expected states are duplicated, missing, or out of order")
    controls, fixed = [], control_inputs()
    names = {"controls_manifest.json", "expected.json"}
    for i, identifier in enumerate(ids):
        name = f"{identifier}.case.json"
        names.add(name)
        row = manifest["fixtures"][i]
        raw = (CONTROL_DIR / name).read_bytes()
        expected = expected_by_id[identifier]
        if (not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "expected_sha256"}
                or row.get("case_id") != identifier or raw != fixed[name]
                or sha256(raw) != row.get("fixture_sha256") or sha256(json_bytes(expected)) != row.get("expected_sha256")
                or expected != expected_snapshot(json.loads(raw, object_pairs_hook=strict_object))):
            raise CorpusError(f"S2-42z frozen control changed: {identifier}")
        controls.append((identifier, raw, expected))
    entries = list(CONTROL_DIR.iterdir())
    if any(x.is_symlink() or not x.is_file() for x in entries) or {x.name for x in entries} != names:
        raise CorpusError("S2-42z controls have missing, extra, or symlink entries")
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
        "code_address": hx(CODE_ADDRESS), "opcodes": [f"0x{x:02x}" for x in OPCODES],
        "displacements": list(DISPLACEMENTS),
        "seeds": [hx(x) for x in SEEDS], "cases_per_seed": CASES_PER_SEED, "control_count": CONTROL_COUNT,
        "generated_case_count": GENERATED_COUNT, "case_count": TOTAL_COUNT, "case_digests": digests,
        "control_manifest_sha256": CONTROL_MANIFEST_SHA256, "expected_sha256": sha256(expected_raw),
        "corpus_digest": sha256("\n".join(x["fixture_sha256"] for x in digests).encode())})
    return files


def verify_corpus(directory: Path):
    if directory.is_symlink() or not directory.is_dir(): raise CorpusError("S2-42z corpus directory missing or unsafe")
    files = materialize()
    verify_file_set(directory, files)
    manifest = load_json(directory / "manifest.json")
    if set(manifest) != MANIFEST_FIELDS or manifest["case_count"] != TOTAL_COUNT or len(manifest["case_digests"]) != TOTAL_COUNT:
        raise CorpusError("S2-42z manifest shape or accounting differs")
    return manifest


def validate_provenance(build, machine, label):
    if (not isinstance(build, dict) or set(build) != BUILD_FIELDS
            or any(not isinstance(build.get(k), str) or not build[k] for k in BUILD_FIELDS)):
        raise CorpusError(f"{label}: build provenance schema or values differ")
    if (not isinstance(machine, dict) or set(machine) != MACHINE_FIELDS
            or any(not isinstance(machine.get(k), str) or not machine[k] for k in ("host_os", "host_arch"))
            or any(machine.get(k) is not None and not isinstance(machine[k], str)
                   for k in ("cpu_model", "kernel"))
            or (machine.get("page_size") is not None
                and (type(machine["page_size"]) is not int or machine["page_size"] <= 0))):
        raise CorpusError(f"{label}: machine provenance schema or values differ")


def compare_result(result, raw, expected, build, machine, label):
    if not isinstance(result, dict) or set(result) != OBSERVATION_FIELDS:
        raise CorpusError(f"{label}: result schema differs")
    if type(result.get("schema_version")) is not int:
        raise CorpusError(f"{label}: schema_version must be an integer")
    wanted = {"schema_version": 2, "case_id": expected["case_id"], "input_sha256": sha256(raw), "profile": PROFILE,
              "backend": "interpreter", "outcome": expected["outcome"], "instructions_executed": expected["instructions_executed"], "build": build, "machine": machine}
    if any(result.get(k) != v for k, v in wanted.items()): raise CorpusError(f"{label}: result identity differs from oracle")
    validate_provenance(result.get("build"), result.get("machine"), label)
    snap = result.get("snapshot")
    if (not isinstance(snap, dict) or set(snap) != SNAPSHOT_FIELDS
            or snap.get("status") != "available" or not isinstance(snap.get("value"), dict)):
        raise CorpusError(f"{label}: complete snapshot unavailable or schema differs")
    actual = snap["value"]
    if (set(actual) != SNAPSHOT_VALUE_FIELDS
            or actual.get("addss_memory_rax_controls") is not None
            or actual.get("minss_memory_rax_controls") is not None):
        raise CorpusError(f"{label}: snapshot contains an unexpected schema or unrelated observation")
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
            if (repeat >= len(ps) or not isinstance(ps[repeat], dict)
                    or type(ps[repeat].get("returncode")) is not int
                    or ps[repeat]["returncode"] != 0 or repeat >= len(rs)
                    or "failure" in rs[repeat]): out.append({"case_id": identifier, "repeat": repeat + 1})
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
    ids = (control_ids()
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
                if b["revision"] != identity["revision"]:
                    raise CorpusError("runner build revision differs from executing source revision")
                if {k: m[k] for k in ("host_os", "host_arch")} != machine:
                    raise CorpusError("runner host provenance differs from executing machine")
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
    if not isinstance(result, dict) or set(result) != SUMMARY_FIELDS:
        raise CorpusError("saved result schema differs")
    if (type(result.get("schema_version")) is not int or result["schema_version"] != 1
            or result.get("status") != "passed" or result.get("backend") != "interpreter"
            or result.get("native_x86") != "not_run_form_not_admitted" or result.get("profile") != PROFILE
            or result.get("failures") != [] or result.get("missing_attempts") != []):
        raise CorpusError("saved evidence is not a successful local interpreter record")
    if (result.get("manifest_sha256") != sha256((directory / "manifest.json").read_bytes())
            or type(result.get("case_count")) is not int or result["case_count"] != TOTAL_COUNT
            or type(result.get("repeats_per_case")) is not int or result["repeats_per_case"] != REPEATS):
        raise CorpusError("saved result/corpus identity differs")
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
    if (not isinstance(rows, list) or any(not isinstance(x, dict) for x in rows)
            or [x.get("case_id") for x in rows] != expected_ids(manifest)):
        raise CorpusError("saved result order/count differs")
    exp = {x["case_id"]: x for x in load_json(directory / "expected.json")["cases"]}
    env0 = None
    for row in rows:
        if set(row) != CASE_RESULT_FIELDS:
            raise CorpusError("saved result row schema differs")
        ident = row["case_id"]; raw = (directory / f"{ident}.case.json").read_bytes()
        repeats, processes = row.get("repeats"), row.get("processes")
        if (row.get("fixture_sha256") != sha256(raw) or not isinstance(repeats, list)
                or not isinstance(processes, list) or len(repeats) != REPEATS
                or len(processes) != REPEATS):
            raise CorpusError(f"saved {ident}: fixture/repeat count differs")
        for i, (observed, proc) in enumerate(zip(repeats, processes), 1):
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
        "native_x86": "not_run_form_not_admitted", "blocked_reason": "S2-42z denies native execution for this local-only near JZ/JNZ rel32 corpus; native execution is not admitted.", "backend_execution": "not_started"}))


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
            write_file_set(args.corpus, materialize()); print(f"generated {TOTAL_COUNT} S2-42z fixtures")
        elif args.verify:
            m = verify_corpus(args.corpus); print(f"verified {TOTAL_COUNT} S2-42z fixtures; digest {m['corpus_digest']}")
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
        print(f"S2-42z failed closed: {e}", file=sys.stderr); return 1
    return 0


if __name__ == "__main__": raise SystemExit(main())
