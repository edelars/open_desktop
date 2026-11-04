#!/usr/bin/env python3
"""Generate and strictly replay the local-only S2-42aa BSWAP register corpus."""
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
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42aa_bswap_register_controls"
CORPUS_DIR = ROOT / "work/stage2/s2-42aa-bswap-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42aa-bswap-corpus/interpreter-results.json"
NATIVE_PATH = ROOT / "work/stage2/s2-42aa-bswap-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42aa-bswap-register-v1"
RIP = 0x00100000
WIDTHS = (32, 64)
CODES = {32: bytes.fromhex("0fc8"), 64: bytes.fromhex("480fc8")}
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED_PER_WIDTH = 256
CONTROL_PER_WIDTH = 16
GENERATED_PER_WIDTH = len(SEEDS) * CASES_PER_SEED_PER_WIDTH
TOTAL_COUNT = len(WIDTHS) * (CONTROL_PER_WIDTH + GENERATED_PER_WIDTH)
REPEATS = 2
MASK32 = 0xFFFFFFFF
MASK64 = 0xFFFFFFFFFFFFFFFF
RFLAGS_DEFINED = 0x0CD7
MANIFEST_FIELDS = {"manifest_version", "generator_version", "profile", "widths", "seeds", "cases_per_seed_per_width", "controls_per_width", "generated_per_width", "case_count", "case_digests", "control_manifest_sha256", "expected_sha256", "corpus_digest"}
CONTROL_MANIFEST_SHA256 = "20990706fd0accc5d952fff1b27a9efbd7ed673328b5d5e71e7f6013e90cfcab"
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12
CONTROL_VALUES_32 = (0, 1, 0x01020304, 0x10203040, 0x7FFFFFFF, 0x80000000,
    MASK32, 0x000000FF, 0xFF000000, 0xAABBCCDD, 0x11223344, 0xDEADBEEF,
    0x7F00FF80, 0x00FF0080, 0x80000001, 0x01000080)
CONTROL_VALUES_64 = (0, 1, 0x0102030405060708, 0x1020304050607080,
    0x7FFFFFFFFFFFFFFF, 0x8000000000000000, MASK64, 0x00000000000000FF,
    0xFF00000000000000, 0xAABBCCDDEEFF0011, 0x1122334455667788,
    0xDEADBEEFCAFEBABE, 0x7F00FF8000FF007F, 0x00FF008000FF0080,
    0x8000000000000001, 0x0100008001000080)


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def control_id(width: int, index: int) -> str:
    return f"s2_42aa_bswap_{width}_c{index:02d}"


def generated_id(width: int, seed_index: int, sequence: int) -> str:
    return f"s2_42aa_bswap_{width}_g{seed_index}_{sequence:03d}"


def fixture(identifier: str, width: int, gprs: list[int], flags: int,
            source: str, seed: int | None = None) -> dict[str, Any]:
    if width not in WIDTHS or len(gprs) != 16 or any(type(x) is not int or not 0 <= x <= MASK64 for x in gprs):
        raise CorpusError("input is outside the exact S2-42aa BSWAP register profile")
    if type(flags) is not int or flags & ~RFLAGS_DEFINED or not flags & 2:
        raise CorpusError("initial RFLAGS contain reserved bits or omit fixed bit 1")
    code = CODES[width]
    spec = ("Intel SDM revision 093, Vol. 2A BSWAP: opcode 0F C8+rd reverses bytes in a 32-bit GPR; "
            "REX.W + 0F C8 reverses bytes in a 64-bit GPR. A 32-bit GPR write zero-extends to 64 bits. "
            "BSWAP preserves flags.")
    return {"schema_version": 2, "case_id": identifier, "profile": PROFILE,
        "provenance": {"kind": "project_fixture", "source": source, "specification": spec,
            "specification_version": "Intel SDM revision 093, Vol. 2A BSWAP; Vol. 1 §3.4.1.1; project S2-15b/S2-42aa"},
        "seed": None if seed is None else hx(seed), "converted_from_v1_sha256": None,
        "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gprs], "rip": hx(RIP), "rflags": hx(flags)},
        "memory": [{"region_id": "code", "guest_address": hx(RIP), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)}],
        "stop_rip": hx(RIP + len(code)), "instruction_budget": 1, "wall_timeout_ms": 2000,
        "rflags_mask": hx(RFLAGS_DEFINED), "expected_exit": "stop_address"}


def control_inputs() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    for width in WIDTHS:
        values = CONTROL_VALUES_32 if width == 32 else CONTROL_VALUES_64
        for i, value in enumerate(values):
            high = ((0xA5C30000 ^ (i * 0x0103070B)) & MASK32) if i else MASK32
            value = ((high << 32) | value) if width == 32 else value
            gprs = [value, 0x1122334455667788, 0xDEADBEEFFEDCBA98,
                0xAABBCCDDEEFF0011, 0x5566778899AABBCC, 0x8877665544332211,
                0x0000000000200004, 0x0000000000200010, 0xAABBCCDDEEFF0011,
                0xFEDCBA9876543210, 0x0123456789ABCDEF, 0x9988776655443322,
                0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, MASK64]
            gprs[1] ^= i * 0x0101010101010101
            flags = 2 | ((i * 0x95) & (RFLAGS_DEFINED & ~2))
            item = fixture(control_id(width, i), width, gprs, flags,
                f"Handwritten S2-42aa exact encoding, width={width}, register value and boundary identity.")
            files[f"{item['case_id']}.case.json"] = json_bytes(item)
    return files


def generated_fixtures() -> list[dict[str, Any]]:
    result, unique, destination_values = [], {32: set(), 64: set()}, {32: set(), 64: set()}
    for width in WIDTHS:
        for si, seed in enumerate(SEEDS):
            for seq in range(CASES_PER_SEED_PER_WIDTH):
                rng = random.Random(seed ^ (width << 48) ^ ((seq + 1) * 0x9E3779B97F4A7C15))
                gprs = [rng.getrandbits(64) for _ in range(16)]
                if width == 32:
                    low = rng.getrandbits(32)
                    while low in destination_values[width]: low = rng.getrandbits(32)
                    destination_values[width].add(low)
                    gprs[0] = (rng.getrandbits(32) << 32) | low
                else:
                    while gprs[0] in destination_values[width]: gprs[0] = rng.getrandbits(64)
                    destination_values[width].add(gprs[0])
                gprs[15] = 0x4274000000000000 | (width << 40) | (si << 32) | seq
                flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
                item = fixture(generated_id(width, si, seq), width, gprs, flags,
                    f"Deterministic S2-42aa input seed={hx(seed)}, width={width}, sequence={seq}.", seed)
                initial = item["initial_state"]
                unique[width].add((tuple(initial["gpr"]), initial["rflags"], item["memory"][0]["initial_bytes_hex"]))
                result.append(item)
    if any(len(unique[w]) != GENERATED_PER_WIDTH or len(destination_values[w]) != GENERATED_PER_WIDTH for w in WIDTHS):
        raise CorpusError("generated architectural input states are not unique per width")
    return result


def reverse_bytes(value: int, width: int) -> int:
    """Independent mathematical byte-reversal oracle; imports no emulator code."""
    size = width // 8
    raw = (value & ((1 << width) - 1)).to_bytes(size, "little")
    return int.from_bytes(raw[::-1], "little")


def expected_snapshot(item: dict[str, Any]) -> dict[str, Any]:
    code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
    width = 64 if code == CODES[64] else 32 if code == CODES[32] else 0
    if width not in WIDTHS:
        raise CorpusError("oracle rejects bytes outside exact S2-15b BSWAP EAX/RAX encodings")
    initial = item["initial_state"]
    gprs = [int(x, 16) for x in initial["gpr"]]
    value = reverse_bytes(gprs[0], width)
    gprs[0] = value if width == 64 else value & MASK32
    flags = int(initial["rflags"], 16)
    return {"case_id": item["case_id"],
        "state": {"mode": "long64", "gpr": [hx(v) for v in gprs],
            "rip": hx(RIP + len(code)), "rflags": hx(flags), "rflags_defined": hx(RFLAGS_DEFINED)},
        "memory": [{"region_id": "code", "guest_address": hx(RIP), "bytes_hex": code.hex()}],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": hx(1)}, "exception": None}


def load_controls():
    manifest_path, expected_path = CONTROL_DIR / "controls_manifest.json", CONTROL_DIR / "expected.json"
    if CONTROL_DIR.is_symlink() or not CONTROL_DIR.is_dir(): raise CorpusError("S2-42aa pinned controls missing or unsafe")
    if any(p.is_symlink() or not p.is_file() for p in (manifest_path, expected_path)):
        raise CorpusError("S2-42aa static control documents are missing or unsafe")
    manifest_raw, expected_raw = manifest_path.read_bytes(), expected_path.read_bytes()
    if CONTROL_MANIFEST_SHA256 == "TO_BE_PINNED" or sha256(manifest_raw) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned S2-42aa control manifest digest differs")
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
        raise CorpusError("S2-42aa frozen controls manifest/expected shape differs")
    by_id = {x.get("case_id"): x for x in expected_doc["cases"] if isinstance(x, dict)}
    if list(by_id) != [control_id(w, i) for w in WIDTHS for i in range(CONTROL_PER_WIDTH)]:
        raise CorpusError("S2-42aa frozen expected snapshots are duplicated, missing, or unordered")
    controls, names = [], {"controls_manifest.json", "expected.json"}
    for index, (name, raw) in enumerate(fixed.items()):
        ident = name.removesuffix(".case.json"); names.add(name); frozen = by_id[ident]
        row = manifest["fixtures"][index]; item = json.loads(raw, object_pairs_hook=strict_object)
        actual_path = CONTROL_DIR / name
        if (not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "expected_sha256"}
            or row.get("case_id") != ident or sha256(raw) != row.get("fixture_sha256")
            or frozen != expected_snapshot(item) or sha256(json_bytes(frozen)) != row.get("expected_sha256")
            or actual_path.is_symlink() or not actual_path.is_file() or actual_path.read_bytes() != raw):
            raise CorpusError(f"S2-42aa pinned control changed: {ident}")
        controls.append((ident, raw, frozen))
    entries = list(CONTROL_DIR.iterdir())
    if any(p.is_symlink() or not p.is_file() for p in entries) or {p.name for p in entries} != names:
        raise CorpusError("S2-42aa control directory has missing, extra, or unsafe entries")
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
        "profile": PROFILE, "widths": list(WIDTHS), "seeds": [hx(x) for x in SEEDS],
        "cases_per_seed_per_width": CASES_PER_SEED_PER_WIDTH, "controls_per_width": CONTROL_PER_WIDTH,
        "generated_per_width": GENERATED_PER_WIDTH, "case_count": TOTAL_COUNT, "case_digests": digests,
        "control_manifest_sha256": CONTROL_MANIFEST_SHA256, "expected_sha256": sha256(expected_raw),
        "corpus_digest": sha256("\n".join(x["fixture_sha256"] for x in digests).encode())})
    return files


def verify_corpus(directory: Path):
    if directory.is_symlink() or not directory.is_dir(): raise CorpusError("S2-42aa corpus directory missing or unsafe")
    files = materialize(); verify_file_set(directory, files); manifest = load_json(directory / "manifest.json")
    if set(manifest) != MANIFEST_FIELDS or manifest.get("case_count") != TOTAL_COUNT or len(manifest.get("case_digests", [])) != TOTAL_COUNT:
        raise CorpusError("S2-42aa manifest shape or accounting differs")
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
    if value.get("state") != {k: expected["state"][k] for k in STATE_FIELDS} or value.get("rflags_defined") != expected["state"]["rflags_defined"]: raise CorpusError(f"{label}: full GPR/RIP/RFLAGS snapshot differs")
    if value.get("memory") != expected["memory"] or value.get("minss_memory_rax_controls") is not None or value.get("addss_memory_rax_controls") is not None: raise CorpusError(f"{label}: code/memory or optional controls differ")
    if value.get("exception") != {"status": "available", "value": None}: raise CorpusError(f"{label}: exception observation differs")


def expected_ids(manifest): return [x["case_id"] for x in manifest["case_digests"]]


def all_case_ids():
    return ([control_id(width, i) for width in WIDTHS for i in range(CONTROL_PER_WIDTH)]
        + [generated_id(width, si, seq) for width in WIDTHS for si in range(len(SEEDS))
           for seq in range(CASES_PER_SEED_PER_WIDTH)])


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
    path.write_bytes(json_bytes({"schema_version": 1, "status": status, "backend": "interpreter", "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "manifest_sha256": sha256((directory / "manifest.json").read_bytes()), "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS,
        "execution_identity": identity, "execution_machine": machine, "runner_build": build, "runner_machine": runner_machine,
        "results": rows, "failures": failures, "missing_attempts": missing_attempts(expected_ids(manifest), rows)}))


def run_interpreter(directory, result_path, executable):
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_bytes(json_bytes({"schema_version": 1, "status": "incomplete", "backend": "interpreter", "native_x86": "not_run_form_not_admitted", "profile": PROFILE, "case_count": TOTAL_COUNT, "repeats_per_case": REPEATS, "results": [], "failures": ["run started or preflight incomplete"], "missing_attempts": missing_attempts(all_case_ids(), [])}))
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
                proc = subprocess.run(cmd, capture_output=True, text=True, check=False, timeout=RUNNER_TIMEOUT_SECONDS, cwd=str(ROOT.resolve()), env=child_env)
                record = {"returncode": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr, "command": cmd, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None}; row["processes"].append(record); process_recorded = True
                if proc.returncode: raise CorpusError(f"runner exited {proc.returncode}: {proc.stderr.strip()}")
                observation = json.loads(proc.stdout, object_pairs_hook=strict_object)
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
        or result.get("backend") != "interpreter" or result.get("native_x86") != "not_run_form_not_admitted" or result.get("profile") != PROFILE
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


def write_native_blocked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": "blocked", "profile": PROFILE,
        "native_x86": "not_run_form_not_admitted", "blocked_reason": "S2-42aa does not admit native execution or physical-hardware acceptance; no native execution was attempted.", "backend_execution": "not_started"}))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__); actions = parser.add_mutually_exclusive_group(required=True)
    for flag in ("generate", "verify", "run-interpreter", "verify-results", "native"): actions.add_argument("--" + flag, action="store_true")
    parser.add_argument("--corpus", type=Path, default=CORPUS_DIR); parser.add_argument("--results", type=Path)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner"); args = parser.parse_args(argv)
    try:
        if args.generate: write_file_set(args.corpus, materialize()); print(f"generated {TOTAL_COUNT} cases ({GENERATED_PER_WIDTH} per width)")
        elif args.verify: m = verify_corpus(args.corpus); print(f"verified {TOTAL_COUNT} cases; digest {m['corpus_digest']}")
        elif args.run_interpreter: run_interpreter(args.corpus, args.results or RESULTS_PATH, args.runner); print(f"passed {TOTAL_COUNT} cases × {REPEATS} local interpreter runs; native=not_run_form_not_admitted")
        elif args.verify_results: verify_results(args.corpus, args.results or RESULTS_PATH, args.runner); print("verified saved, provenance-bound local results")
        else: write_native_blocked(args.results or NATIVE_PATH); print("native blocked; backend_execution=not_started"); return 1
    except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError) as e:
        print(f"S2-42aa failed closed: {e}", file=sys.stderr); return 1
    return 0


if __name__ == "__main__": raise SystemExit(main())
