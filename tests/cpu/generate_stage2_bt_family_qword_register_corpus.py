#!/usr/bin/env python3
"""Generate and strictly replay the local-only S2-42af qword BT-family corpus."""
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
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42af_bt_family_qword_register_controls"
CORPUS_DIR = ROOT / "work/stage2/s2-42af-bt-family-qword-register-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42af-bt-family-qword-register-corpus/interpreter-results.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42af-bt-family-qword-register-v1"
RIP = 0x00100000
FORMS = {
    "bt": ("bt", bytes.fromhex("480fa3c8")),
    "bts": ("bts", bytes.fromhex("480fabc8")),
    "btr": ("btr", bytes.fromhex("480fb3c8")),
    "btc": ("btc", bytes.fromhex("480fbbc8")),
}
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED_PER_FORM = 256
CONTROL_PER_FORM = 16
GENERATED_PER_FORM = len(SEEDS) * CASES_PER_SEED_PER_FORM
TOTAL_COUNT = len(FORMS) * (CONTROL_PER_FORM + GENERATED_PER_FORM)
REPEATS = 2
MASK64 = 0xFFFFFFFFFFFFFFFF
RFLAGS_DEFINED = 0x0CD7
RFLAGS_MASK = 0x0443
MANIFEST_FIELDS = {"manifest_version", "generator_version", "profile", "forms", "seeds", "cases_per_seed_per_form", "controls_per_form", "generated_per_form", "case_count", "case_digests", "control_manifest_sha256", "expected_sha256", "corpus_digest"}
CONTROL_MANIFEST_SHA256 = "2232841123e8c8e111f5f63caab372872825283e9e3d529859d9c95e749f1191"
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12
CONTROL_INPUTS = (
    (0, 0), (1, 0), (0, 63), (1 << 63, 63),
    (0, 64), (1 << 0, 64), (0, 127), (1 << 63, 127),
    (0xAAAAAAAAAAAAAAAA, 0xFFFFFFFFFFFFFFFF),
    (0x5555555555555555, 0x8000000000000000),
    (0x7FFFFFFFFFFFFFFF, 0x1000000000000042),
    (MASK64, 0xFFFFFFFFFFFFFFC0),
    (0x0123456789ABCDEF, 1), (0xFEDCBA9876543210, 65),
    (0x8000000000000001, 0xFFFFFFFFFFFFFFBF),
    (0x0F0F0F0F0F0F0F0F, 0x123456789ABCDEF8),
)
FLAG_PROBE_BITS = (0x000, 0x8D5, 0x045, 0x892, 0x455, 0xC80, 0x0D3, 0x80D,
    0x100, 0x220, 0x400, 0x2, 0x600, 0x812, 0x0C3, 0x4D0)


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def control_id(form: str, index: int) -> str:
    return f"s2_42af_{form}_c{index:02d}"


def generated_id(form: str, seed_index: int, sequence: int) -> str:
    return f"s2_42af_{form}_g{seed_index}_{sequence:03d}"


def fixture(identifier: str, form: str, gprs: list[int], flags: int,
            source: str, seed: int | None = None) -> dict[str, Any]:
    if form not in FORMS or len(gprs) != 16 or any(type(x) is not int or not 0 <= x <= MASK64 for x in gprs):
        raise CorpusError("input is outside the exact S2-42af qword BT-family register profile")
    if type(flags) is not int or flags & ~RFLAGS_DEFINED or not flags & 2:
        raise CorpusError("initial RFLAGS contain reserved bits or omit fixed bit 1")
    operation, code = FORMS[form]
    spec = (f"Intel SDM revision 093, Vol. 2A {operation.upper()}: exact Long64 bytes {code.hex(' ').upper()}, "
            "qword register form BT/BTS/BTR/BTC RAX,RCX. RCX selects bit RCX modulo 64. CF receives "
            "the old selected bit; ZF is preserved; OF/SF/AF/PF are undefined. BT preserves RAX; BTS "
            "sets the selected bit, BTR clears it, and BTC complements it.")
    return {"schema_version": 2, "case_id": identifier, "profile": PROFILE,
        "provenance": {"kind": "project_fixture", "source": source, "specification": spec,
            "specification_version": "Intel SDM revision 093, Vol. 2A BT/BTC/BTR/BTS; project S2-15c/S2-42af"},
        "seed": None if seed is None else hx(seed), "converted_from_v1_sha256": None,
        "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gprs], "rip": hx(RIP), "rflags": hx(flags)},
        "memory": [{"region_id": "code", "guest_address": hx(RIP), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)}],
        "stop_rip": hx(RIP + len(code)), "instruction_budget": 1, "wall_timeout_ms": 2000,
        "rflags_mask": hx(RFLAGS_MASK), "expected_exit": "stop_address"}


def control_inputs() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    for form in FORMS:
        for i, (destination, offset) in enumerate(CONTROL_INPUTS):
            gprs = [0x1122334455667788, 0x8877665544332211, 0xDEADBEEFFEDCBA98,
                0xAABBCCDDEEFF0011, 0x5566778899AABBCC, 0x8877665544332211,
                0x0000000000200004, 0x0000000000200010, 0xAABBCCDDEEFF0011,
                0xFEDCBA9876543210, 0x0123456789ABCDEF, 0x9988776655443322,
                0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, MASK64]
            gprs[0], gprs[1] = destination, offset
            flags = 2 | (FLAG_PROBE_BITS[i] & (RFLAGS_DEFINED & ~2))
            item = fixture(control_id(form, i), form, gprs, flags,
                f"Pinned S2-42af {form} control: RAX={destination:#018x}, RCX={offset:#018x}.")
            files[f"{item['case_id']}.case.json"] = json_bytes(item)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    result, unique = [], {form: set() for form in FORMS}
    for fi, form in enumerate(FORMS):
        for si, seed in enumerate(SEEDS):
            for seq in range(CASES_PER_SEED_PER_FORM):
                rng = random.Random(seed ^ (fi << 52) ^ (seq * 0x9E3779B97F4A7C15))
                gprs = [rng.getrandbits(64) for _ in range(16)]
                offset = seq % 64
                mode = (seq // 64) % 4
                if mode == 1:
                    offset |= 0x40
                elif mode == 2:
                    offset |= 0x8000000000000000
                elif mode == 3:
                    offset |= 0x1000000000000000
                selected_bit = offset & 63
                destination = gprs[0] & ~(1 << selected_bit)
                if seq & 1:
                    destination |= 1 << selected_bit
                gprs[0], gprs[1] = destination, offset
                gprs[15] = 0x42AF000000000000 | (fi << 40) | (si << 32) | seq
                flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
                item = fixture(generated_id(form, si, seq), form, gprs, flags,
                    f"Deterministic S2-42af input seed={hx(seed)}, form={form}, sequence={seq}.", seed)
                initial = item["initial_state"]
                unique[form].add((tuple(initial["gpr"]), initial["rflags"], item["memory"][0]["initial_bytes_hex"]))
                result.append(item)
    if any(len(unique[f]) != GENERATED_PER_FORM for f in FORMS):
        raise CorpusError("generated architectural input states are not unique per exact form")
    return result


def expected_snapshot(item: dict[str, Any]) -> dict[str, Any]:
    code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
    form = next((key for key, (_, exact) in FORMS.items() if code == exact), None)
    if form is None or not item["case_id"].startswith(f"s2_42af_{form}_"):
        raise CorpusError("oracle rejects bytes outside the four exact S2-42af BT-family encodings")
    operation, _ = FORMS[form]
    initial = item["initial_state"]
    gprs = [int(x, 16) for x in initial["gpr"]]
    destination, offset = gprs[0], gprs[1]
    index = offset & 63
    bit = (destination >> index) & 1
    selected = 1 << index
    if operation == "bts":
        gprs[0] = destination | selected
    elif operation == "btr":
        gprs[0] = destination & ~selected & MASK64
    elif operation == "btc":
        gprs[0] = destination ^ selected
    old_flags = int(initial["rflags"], 16)
    flags = (old_flags & ~1) | bit
    return {"case_id": item["case_id"],
        "state": {"mode": "long64", "gpr": [hx(v) for v in gprs],
            "rip": hx(RIP + len(code)), "rflags": hx(flags), "rflags_defined": hx(RFLAGS_MASK)},
        "memory": [{"region_id": "code", "guest_address": hx(RIP), "bytes_hex": code.hex()}],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": hx(1)}, "exception": None}


def load_controls():
    manifest_path, expected_path = CONTROL_DIR / "controls_manifest.json", CONTROL_DIR / "expected.json"
    if CONTROL_DIR.is_symlink() or not CONTROL_DIR.is_dir(): raise CorpusError("S2-42af pinned controls missing or unsafe")
    if any(p.is_symlink() or not p.is_file() for p in (manifest_path, expected_path)):
        raise CorpusError("S2-42af static control documents are missing or unsafe")
    manifest_raw, expected_raw = manifest_path.read_bytes(), expected_path.read_bytes()
    if CONTROL_MANIFEST_SHA256 == "TO_BE_PINNED" or sha256(manifest_raw) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned S2-42af control manifest digest differs")
    manifest = json.loads(manifest_raw, object_pairs_hook=strict_object)
    expected_doc = json.loads(expected_raw, object_pairs_hook=strict_object)
    fixed = control_inputs()
    if (not isinstance(manifest, dict) or set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
        or type(manifest.get("schema_version")) is not int or manifest["schema_version"] != 1
        or manifest.get("profile") != PROFILE or manifest.get("control_count") != len(fixed)
        or sha256(expected_raw) != manifest.get("expected_sha256")
        or not isinstance(expected_doc, dict) or set(expected_doc) != {"profile", "cases"}
        or expected_doc.get("profile") != PROFILE or not isinstance(expected_doc.get("cases"), list)
        or len(expected_doc["cases"]) != len(fixed)):
        raise CorpusError("S2-42af frozen controls manifest/expected shape differs")
    by_id = {x.get("case_id"): x for x in expected_doc["cases"] if isinstance(x, dict)}
    if list(by_id) != [control_id(form, i) for form in FORMS for i in range(CONTROL_PER_FORM)]:
        raise CorpusError("S2-42af frozen expected snapshots are duplicated, missing, or unordered")
    controls, names = [], {"controls_manifest.json", "expected.json"}
    for index, (name, raw) in enumerate(fixed.items()):
        ident = name.removesuffix(".case.json"); names.add(name); frozen = by_id[ident]
        row = manifest["fixtures"][index]; item = json.loads(raw, object_pairs_hook=strict_object)
        actual_path = CONTROL_DIR / name
        if (not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "expected_sha256"}
            or row.get("case_id") != ident or sha256(raw) != row.get("fixture_sha256")
            or frozen != expected_snapshot(item) or sha256(json_bytes(frozen)) != row.get("expected_sha256")
            or actual_path.is_symlink() or not actual_path.is_file() or actual_path.read_bytes() != raw):
            raise CorpusError(f"S2-42af pinned control changed: {ident}")
        controls.append((ident, raw, frozen))
    entries = list(CONTROL_DIR.iterdir())
    if any(p.is_symlink() or not p.is_file() for p in entries) or {p.name for p in entries} != names:
        raise CorpusError("S2-42af control directory has missing, extra, or unsafe entries")
    return controls, manifest_raw


def all_cases():
    controls, control_raw = load_controls()
    rows = [(ident, raw, "handwritten", expected) for ident, raw, expected in controls]
    for item in generated_fixtures(): rows.append((item["case_id"], json_bytes(item), "generated", expected_snapshot(item)))
    return rows, control_raw


def materialize() -> dict[str, bytes]:
    rows, _ = all_cases(); files, digests, snapshots = {}, [], []
    for ident, raw, kind, expected in rows:
        files[f"{ident}.case.json"] = raw; snapshots.append(expected)
        digests.append({"case_id": ident, "kind": kind, "fixture_sha256": sha256(raw), "expected_sha256": sha256(json_bytes(expected))})
    expected_raw = json_bytes({"profile": PROFILE, "cases": snapshots})
    files["expected.json"] = expected_raw
    files["manifest.json"] = json_bytes({"manifest_version": 1, "generator_version": GENERATOR_VERSION,
        "profile": PROFILE, "forms": list(FORMS), "seeds": [hx(x) for x in SEEDS],
        "cases_per_seed_per_form": CASES_PER_SEED_PER_FORM, "controls_per_form": CONTROL_PER_FORM,
        "generated_per_form": GENERATED_PER_FORM, "case_count": TOTAL_COUNT, "case_digests": digests,
        "control_manifest_sha256": CONTROL_MANIFEST_SHA256, "expected_sha256": sha256(expected_raw),
        "corpus_digest": sha256("\n".join(x["fixture_sha256"] for x in digests).encode())})
    return files


def verify_corpus(directory: Path):
    if directory.is_symlink() or not directory.is_dir(): raise CorpusError("S2-42af corpus directory missing or unsafe")
    files = materialize(); verify_file_set(directory, files); manifest = load_json(directory / "manifest.json")
    if set(manifest) != MANIFEST_FIELDS or manifest.get("case_count") != TOTAL_COUNT or len(manifest.get("case_digests", [])) != TOTAL_COUNT:
        raise CorpusError("S2-42af manifest shape or accounting differs")
    return manifest


RESULT_FIELDS = {"schema_version", "case_id", "input_sha256", "profile", "backend", "outcome", "instructions_executed", "snapshot", "build", "machine"}
BUILD_FIELDS = {"revision", "target", "profile", "compiler"}
MACHINE_FIELDS = {"host_os", "host_arch", "cpu_model", "kernel", "page_size"}
STATE_FIELDS = {"mode", "gpr", "rip", "rflags"}
SNAPSHOT_FIELDS = {"status", "value"}
SNAPSHOT_VALUE_FIELDS = {"state", "rflags_defined", "memory", "minss_memory_rax_controls", "addss_memory_rax_controls", "exception"}


def validate_provenance(build, machine, label):
    if not isinstance(build, dict) or set(build) != BUILD_FIELDS or any(not isinstance(v, str) or not v for v in build.values()): raise CorpusError(f"{label}: incomplete build provenance")
    if not isinstance(machine, dict) or set(machine) != MACHINE_FIELDS: raise CorpusError(f"{label}: machine provenance fields differ")
    if any(not isinstance(machine.get(k), str) or not machine[k] for k in ("host_os", "host_arch")): raise CorpusError(f"{label}: incomplete machine provenance")
    if any(v is not None and not isinstance(v, str) for v in (machine["cpu_model"], machine["kernel"])): raise CorpusError(f"{label}: invalid machine text provenance")
    if machine["page_size"] is not None and (type(machine["page_size"]) is not int or machine["page_size"] <= 0): raise CorpusError(f"{label}: invalid page size provenance")


def compare_result(result, raw, expected, build, machine, label):
    if not isinstance(result, dict) or set(result) != RESULT_FIELDS or type(result.get("schema_version")) is not int or result["schema_version"] != 2: raise CorpusError(f"{label}: result shape or schema differs")
    wanted = {"case_id": expected["case_id"], "input_sha256": sha256(raw), "profile": PROFILE, "backend": "interpreter", "build": build, "machine": machine}
    if any(result.get(k) != v for k, v in wanted.items()): raise CorpusError(f"{label}: identity/provenance differs")
    validate_provenance(result["build"], result["machine"], label)
    if result.get("outcome") != expected["outcome"] or result.get("instructions_executed") != expected["instructions_executed"]: raise CorpusError(f"{label}: exit or retirement differs")
    snap = result.get("snapshot")
    if not isinstance(snap, dict) or set(snap) != SNAPSHOT_FIELDS or snap.get("status") != "available" or not isinstance(snap.get("value"), dict): raise CorpusError(f"{label}: full snapshot unavailable or malformed")
    value = snap["value"]
    if set(value) != SNAPSHOT_VALUE_FIELDS: raise CorpusError(f"{label}: snapshot fields differ")
    state = value.get("state")
    if not isinstance(state, dict) or set(state) != STATE_FIELDS:
        raise CorpusError(f"{label}: architectural state shape differs")
    if any(state.get(key) != expected["state"][key] for key in ("mode", "gpr", "rip")):
        raise CorpusError(f"{label}: GPR/RIP state differs")
    try:
        actual_flags = int(state["rflags"], 16)
        expected_flags = int(expected["state"]["rflags"], 16)
    except (TypeError, ValueError) as e:
        raise CorpusError(f"{label}: RFLAGS is malformed") from e
    if actual_flags & RFLAGS_MASK != expected_flags & RFLAGS_MASK:
        raise CorpusError(f"{label}: defined RFLAGS bits differ")
    if value.get("rflags_defined") != expected["state"]["rflags_defined"]: raise CorpusError(f"{label}: RFLAGS definedness differs")
    if value.get("memory") != expected["memory"] or value.get("minss_memory_rax_controls") is not None or value.get("addss_memory_rax_controls") is not None: raise CorpusError(f"{label}: code/memory or optional controls differ")
    if value.get("exception") != {"status": "available", "value": None}: raise CorpusError(f"{label}: exception observation differs")


def expected_ids(manifest): return [x["case_id"] for x in manifest["case_digests"]]


def all_case_ids():
    return ([control_id(form, i) for form in FORMS for i in range(CONTROL_PER_FORM)]
        + [generated_id(form, si, seq) for form in FORMS for si in range(len(SEEDS))
           for seq in range(CASES_PER_SEED_PER_FORM)])


def process_environment(): return {key: os.environ.get(key) for key in PROCESS_ENV_KEYS}
def output_text(value): return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else value or ""


def command_identity(executable):
    executable = executable.resolve(strict=True)
    revision = current_revision()
    osname = {"darwin": "macos"}.get(platform.system().lower(), platform.system().lower())
    arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(platform.machine().lower(), platform.machine().lower())
    return {"executable_sha256": sha256(executable.read_bytes()), "revision": revision}, {"host_os": osname, "host_arch": arch}


def current_revision():
    return subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()


def missing_attempts(ids, rows):
    by_id = {x.get("case_id"): x for x in rows if isinstance(x, dict)}; result = []
    for ident in ids:
        row = by_id.get(ident, {})
        for repeat in range(REPEATS):
            ps, rs = row.get("processes", []), row.get("repeats", [])
            if repeat >= len(ps) or ps[repeat].get("returncode") != 0 or repeat >= len(rs) or "failure" in rs[repeat]: result.append({"case_id": ident, "repeat": repeat + 1})
    return result


def write_checkpoint(path, directory, manifest, rows, failures, identity, machine, build, runner_machine, status="incomplete"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": status, "backend": "interpreter", "native_x86": "not_run/not_selected", "profile": PROFILE,
        "manifest_sha256": sha256((directory / "manifest.json").read_bytes()), "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS,
        "execution_identity": identity, "execution_machine": machine, "runner_build": build, "runner_machine": runner_machine,
        "results": rows, "failures": failures, "missing_attempts": missing_attempts(expected_ids(manifest), rows)}))


def run_interpreter(directory, result_path, executable):
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_bytes(json_bytes({"schema_version": 1, "status": "incomplete", "backend": "interpreter", "native_x86": "not_run/not_selected", "profile": PROFILE, "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS, "results": [], "failures": ["run started or preflight incomplete"], "missing_attempts": missing_attempts(all_case_ids(), [])}))
    manifest = verify_corpus(directory); identity, machine = command_identity(executable)
    expected = {x["case_id"]: x for x in load_json(directory / "expected.json")["cases"]}
    rows, failures, build, runner_machine = [], [], None, None
    env = process_environment(); child_env = {k: v for k, v in env.items() if v is not None}
    for ident in expected_ids(manifest):
        path = (directory / f"{ident}.case.json").resolve(strict=True); raw = path.read_bytes()
        row = {"case_id": ident, "fixture_sha256": sha256(raw), "repeats": [], "processes": []}; rows.append(row)
        for repeat in range(REPEATS):
            cmd = [str(executable.resolve(strict=True)), "--fixture", str(path), "--engine", "interpreter", "--json"]
            process_recorded = False
            try:
                proc = subprocess.run(cmd, capture_output=True, text=False, check=False, timeout=RUNNER_TIMEOUT_SECONDS, cwd=str(ROOT.resolve()), env=child_env)
                stdout_bytes = proc.stdout or b""
                stderr_bytes = proc.stderr or b""
                try:
                    stdout = stdout_bytes if isinstance(stdout_bytes, str) else stdout_bytes.decode("utf-8")
                    stderr = stderr_bytes if isinstance(stderr_bytes, str) else stderr_bytes.decode("utf-8")
                except UnicodeError as e:
                    diagnostic = (f"captured subprocess output was not valid UTF-8: {e}; "
                        f"stdout_hex={stdout_bytes.encode('utf-8').hex() if isinstance(stdout_bytes, str) else stdout_bytes.hex()}; "
                        f"stderr_hex={stderr_bytes.encode('utf-8').hex() if isinstance(stderr_bytes, str) else stderr_bytes.hex()}")
                    record = {"returncode": proc.returncode, "stdout": output_text(stdout_bytes),
                        "stderr": output_text(stderr_bytes) + ("\n" if stderr_bytes else "") + diagnostic,
                        "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None}
                    row["processes"].append(record); process_recorded = True
                    raise CorpusError(diagnostic) from e
                record = {"returncode": proc.returncode, "stdout": stdout, "stderr": stderr, "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None}; row["processes"].append(record); process_recorded = True
                if proc.returncode: raise CorpusError(f"runner exited {proc.returncode}: {stderr.strip()}")
                observation = json.loads(stdout, object_pairs_hook=strict_object)
                if not isinstance(observation, dict): raise CorpusError("runner output must be one JSON object")
                b, m = observation.get("build"), observation.get("machine"); validate_provenance(b, m, "interpreter result")
                if build is None: build, runner_machine = b, m
                if b != build or m != runner_machine: raise CorpusError("runner provenance changed during run")
                compare_result(observation, raw, expected[ident], build, runner_machine, f"{ident} repeat {repeat+1}")
                row["repeats"].append(observation)
            except KeyboardInterrupt as e:
                if not process_recorded: row["processes"].append({"returncode": None, "stdout": "", "stderr": "interrupted", "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": "interrupted", "returncode": None}); failures.append(f"{ident} repeat {repeat+1}: interrupted")
                write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
                raise CorpusError(f"interpreter interrupted; checkpoint saved: {result_path}") from e
            except subprocess.TimeoutExpired as e:
                msg = str(e); row["processes"].append({"returncode": None, "stdout": output_text(e.stdout), "stderr": output_text(e.stderr), "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": {"kind": "timeout_expired", "timeout_seconds": RUNNER_TIMEOUT_SECONDS, "message": msg}})
                row["repeats"].append({"failure": msg, "returncode": None}); failures.append(f"{ident} repeat {repeat+1}: timed out")
            except (OSError, subprocess.SubprocessError, json.JSONDecodeError, CorpusError) as e:
                if not process_recorded: row["processes"].append({"returncode": None, "stdout": "", "stderr": str(e), "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": str(e), "returncode": row["processes"][-1]["returncode"]}); failures.append(f"{ident} repeat {repeat+1}: {e}")
        if len(row["repeats"]) == REPEATS and all("failure" not in x for x in row["repeats"]) and row["repeats"][0] != row["repeats"][1]: failures.append(f"{ident}: repeats differ")
        write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
    write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine, "failed" if failures else "passed")
    if failures: raise CorpusError(f"{len(failures)} local interpreter failures; saved {result_path}")
    return load_json(result_path)


SAVED_RESULT_FIELDS = {"schema_version", "status", "backend", "native_x86", "profile", "manifest_sha256", "case_count", "repeats_per_case", "execution_identity", "execution_machine", "runner_build", "runner_machine", "results", "failures", "missing_attempts"}


def verify_results(directory, result_path, executable):
    manifest = verify_corpus(directory); result = load_json(result_path)
    if not isinstance(result, dict) or set(result) != SAVED_RESULT_FIELDS: raise CorpusError("saved evidence fields differ")
    if (type(result.get("schema_version")) is not int or result["schema_version"] != 1 or result.get("status") != "passed"
        or result.get("backend") != "interpreter" or result.get("native_x86") != "not_run/not_selected" or result.get("profile") != PROFILE
        or result.get("failures") != [] or result.get("missing_attempts") != [] or result.get("manifest_sha256") != sha256((directory / "manifest.json").read_bytes())
        or type(result.get("case_count")) is not int or result["case_count"] != TOTAL_COUNT
        or type(result.get("repeats_per_case")) is not int or result["repeats_per_case"] != REPEATS): raise CorpusError("saved evidence is not a complete success for this corpus")
    build, machine = result.get("runner_build"), result.get("runner_machine"); validate_provenance(build, machine, "saved result")
    identity, selected = result.get("execution_identity"), executable.resolve(strict=True)
    revision = identity.get("revision") if isinstance(identity, dict) else None
    revision_is_hash = isinstance(revision, str) and len(revision) == 40 and all(c in "0123456789abcdef" for c in revision)
    if (not isinstance(identity, dict) or set(identity) != {"executable_sha256", "revision"}
        or identity["executable_sha256"] != sha256(selected.read_bytes()) or not revision_is_hash
        or revision != current_revision()
        or build["revision"] not in ("unavailable", revision)):
        raise CorpusError("saved runner/source provenance differs")
    if result.get("execution_machine") != {k: machine[k] for k in ("host_os", "host_arch")}: raise CorpusError("saved host provenance differs")
    rows = result.get("results")
    if not isinstance(rows, list) or len(rows) != TOTAL_COUNT or any(not isinstance(r, dict) for r in rows) or [r.get("case_id") for r in rows] != expected_ids(manifest): raise CorpusError("saved case order/count differs")
    expected = {x["case_id"]: x for x in load_json(directory / "expected.json")["cases"]}; env0 = None
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "repeats", "processes"}: raise CorpusError("saved result row fields differ")
        ident = row["case_id"]; raw = (directory / f"{ident}.case.json").read_bytes()
        if (row["fixture_sha256"] != sha256(raw) or not isinstance(row["repeats"], list)
            or not isinstance(row["processes"], list) or len(row["repeats"]) != REPEATS
            or len(row["processes"]) != REPEATS): raise CorpusError(f"{ident}: saved digest/repeat count differs")
        fixture_path = str((directory / f"{ident}.case.json").resolve(strict=True))
        for i, (observed, proc) in enumerate(zip(row["repeats"], row["processes"]), 1):
            if (not isinstance(proc, dict) or set(proc) != {"returncode", "stdout", "stderr", "command", "cwd", "environment", "timeout_diagnosis"}
                or type(proc.get("returncode")) is not int or proc["returncode"] != 0
                or not isinstance(proc.get("stdout"), str) or not isinstance(proc.get("stderr"), str)
                or not isinstance(proc.get("command"), list) or not all(isinstance(arg, str) for arg in proc["command"])
                or not isinstance(proc.get("cwd"), str) or proc["timeout_diagnosis"] is not None): raise CorpusError(f"{ident} repeat {i}: malformed process evidence")
            if proc["command"] != [str(selected), "--fixture", fixture_path, "--engine", "interpreter", "--json"] or proc["cwd"] != str(ROOT.resolve()): raise CorpusError(f"{ident} repeat {i}: command/cwd provenance differs")
            env = proc["environment"]
            if not isinstance(env, dict) or set(env) != set(PROCESS_ENV_KEYS) or any(v is not None and not isinstance(v, str) for v in env.values()) or (env0 is not None and env != env0): raise CorpusError(f"{ident} repeat {i}: environment provenance differs")
            env0 = env; parsed = json.loads(proc["stdout"], object_pairs_hook=strict_object)
            compare_result(parsed, raw, expected[ident], build, machine, f"saved {ident} repeat {i} stdout")
            if parsed != observed: raise CorpusError(f"{ident} repeat {i}: frozen stdout differs from result")
            compare_result(observed, raw, expected[ident], build, machine, f"saved {ident} repeat {i}")
        if row["repeats"][0] != row["repeats"][1]: raise CorpusError(f"{ident}: repeat instability")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__); actions = parser.add_mutually_exclusive_group(required=True)
    for flag in ("generate", "verify", "run-interpreter", "verify-results"): actions.add_argument("--" + flag, action="store_true")
    parser.add_argument("--corpus", type=Path, default=CORPUS_DIR); parser.add_argument("--results", type=Path)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner"); args = parser.parse_args(argv)
    try:
        if args.generate: write_file_set(args.corpus, materialize()); print(f"generated {TOTAL_COUNT} cases ({GENERATED_PER_FORM} generated per exact form)")
        elif args.verify: m = verify_corpus(args.corpus); print(f"verified {TOTAL_COUNT} cases; digest {m['corpus_digest']}")
        elif args.run_interpreter: run_interpreter(args.corpus, args.results or RESULTS_PATH, args.runner); print(f"passed {TOTAL_COUNT} cases × {REPEATS} local interpreter runs; native=not_run/not_selected")
        elif args.verify_results: verify_results(args.corpus, args.results or RESULTS_PATH, args.runner); print("verified saved, provenance-bound local results")
    except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError) as e:
        print(f"S2-42af failed closed: {e}", file=sys.stderr); return 1
    return 0


if __name__ == "__main__": raise SystemExit(main())
