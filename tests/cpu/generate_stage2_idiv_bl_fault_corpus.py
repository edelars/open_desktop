#!/usr/bin/env python3
"""Generate, run, and strictly replay the local S2-42ad IDIV BL #DE corpus."""
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
CONTROL_DIR = ROOT / "tests/fixtures/x86/stage2/s2_42ad_idiv_bl_fault_controls"
CORPUS_DIR = ROOT / "work/stage2/s2-42ad-idiv-bl-fault-corpus/corpus"
RESULTS_PATH = ROOT / "work/stage2/s2-42ad-idiv-bl-fault-corpus/interpreter-results.json"
NATIVE_PATH = ROOT / "work/stage2/s2-42ad-idiv-bl-fault-corpus/native-blocked.json"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42ad-idiv-bl-fault-v1"
CODE_ADDRESS = 0x00100000
CODE = bytes.fromhex("f6fb")  # IDIV BL
RFLAGS_DEFINED = 0x0CD7
BASE_FLAGS = 0x08D7
MASK64 = 0xFFFFFFFFFFFFFFFF
SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344, 0xA4093822299F31D0, 0x082EFA98EC4E6C89)
CASES_PER_SEED = 256
GENERATED_COUNT = len(SEEDS) * CASES_PER_SEED
CONTROL_AX_BL = ((0x0000, 0), (0x0100, 0), (0xFFFF, 0), (0x00FF, 0),
                 (0x8000, 0xFF), (0x7FFF, 1), (0x8000, 1), (0x7FFF, 0xFF))
CONTROL_COUNT = len(CONTROL_AX_BL)
TOTAL_COUNT = GENERATED_COUNT + CONTROL_COUNT
REPEATS = 2
PROCESS_ENV_KEYS = ("PATH", "LANG", "LC_ALL", "RUST_LOG", "RUST_BACKTRACE")
RUNNER_TIMEOUT_SECONDS = 12
MANIFEST_FIELDS = {"manifest_version", "generator_version", "profile", "seeds", "cases_per_seed", "control_count", "generated_case_count", "case_count", "case_digests", "control_manifest_sha256", "expected_sha256", "corpus_digest"}
CONTROL_MANIFEST_SHA256 = "24bda9c454e217facd8f48ff4cbea12c9561c9e431d5d5ef7913a3f3afa1d5b9"


def hx(value: int) -> str:
    return f"0x{value & MASK64:016x}"


def load_json(path: Path) -> Any:
    if path.is_symlink() or not path.is_file():
        raise CorpusError(f"required regular file is missing or unsafe: {path}")
    return json.loads(path.read_bytes(), object_pairs_hook=strict_object)


def signed(value: int, width: int) -> int:
    sign = 1 << (width - 1)
    return value - (1 << width) if value & sign else value


def truncating_quotient(dividend: int, divisor: int) -> int:
    """Signed integer division rounded toward zero, without float conversion."""
    magnitude = abs(dividend) // abs(divisor)
    return -magnitude if (dividend < 0) != (divisor < 0) else magnitude


def is_idiv_fault(ax: int, bl: int) -> bool:
    """Independent integer oracle for signed AX / signed BL byte IDIV #DE."""
    if type(ax) is not int or type(bl) is not int or not 0 <= ax <= 0xFFFF or not 0 <= bl <= 0xFF:
        return False
    dividend, divisor = signed(ax, 16), signed(bl, 8)
    if divisor == 0:
        return True
    quotient = truncating_quotient(dividend, divisor)
    return not -128 <= quotient <= 127


def control_id(index: int) -> str:
    return f"s2_42ad_idiv_bl_fault_c{index:02d}"


def generated_id(seed_index: int, sequence: int) -> str:
    return f"s2_42ad_idiv_bl_fault_g{seed_index}_{sequence:03d}"


def make_fixture(identifier: str, ax: int, bl: int, gprs: list[int], flags: int,
                 source: str, seed: int | None = None) -> dict[str, Any]:
    if not is_idiv_fault(ax, bl):
        raise CorpusError("fault corpus admits only BL=0 or signed AX/BL outside [-128,127]")
    if len(gprs) != 16 or any(type(value) is not int or not 0 <= value <= MASK64 for value in gprs):
        raise CorpusError("input is outside the exact S2-42ad IDIV BL profile")
    if gprs[0] & 0xFFFF != ax or gprs[3] & 0xFF != bl:
        raise CorpusError("AX/BL input does not match the encoded fixture state")
    if flags & ~RFLAGS_DEFINED or flags & 2 == 0:
        raise CorpusError("initial RFLAGS contain reserved bits or omit fixed bit 1")
    return {
        "schema_version": 2, "case_id": identifier, "profile": PROFILE,
        "provenance": {"kind": "project_fixture", "source": source,
            "specification": "Intel SDM revision 093, Vol. 2A IDIV: byte IDIV divides signed AX by signed r/m8, truncates toward zero, and raises #DE when the divisor is zero or the quotient is outside [-128,127]. Exact bytes F6 FB encode IDIV BL.",
            "specification_version": "Intel SDM Vol.2A revision 093 (253667-093US); project S2-42ad"},
        "seed": None if seed is None else hx(seed), "converted_from_v1_sha256": None,
        "initial_state": {"mode": "long64", "gpr": [hx(value) for value in gprs], "rip": hx(CODE_ADDRESS), "rflags": hx(flags)},
        "memory": [{"region_id": "code", "guest_address": hx(CODE_ADDRESS), "length": len(CODE),
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": CODE.hex(), "comparison_mask_hex": "ff" * len(CODE)}],
        "stop_rip": None, "instruction_budget": 1, "wall_timeout_ms": 2000,
        "rflags_mask": hx(RFLAGS_DEFINED), "expected_exit": "exception",
    }


def base_gprs(ax: int, bl: int) -> list[int]:
    values = [0x1122334455667788, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
              0x0000000000200000, 0x0000000000300080, 0x0000000000300080,
              0x0000000000200004, 0x0000000000200010, 0x0123456789ABCDEF,
              0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
              0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
              MASK64]
    values[0] = (values[0] & ~0xFFFF) | ax
    values[3] = (values[3] & ~0xFF) | bl
    return values


def control_inputs() -> dict[str, bytes]:
    result = {}
    for index, (ax, bl) in enumerate(CONTROL_AX_BL):
        item = make_fixture(control_id(index), ax, bl, base_gprs(ax, bl), BASE_FLAGS,
                            "Handwritten S2-42ad F6 FB bytes, AX/BL values, and pinned #DE boundary identity.")
        result[f"{item['case_id']}.case.json"] = json_bytes(item)
    return result


def materialize_controls() -> dict[str, bytes]:
    inputs = control_inputs()
    cases = []
    fixtures = []
    for index in range(CONTROL_COUNT):
        identifier = control_id(index)
        raw = inputs[f"{identifier}.case.json"]
        expected = expected_snapshot(json.loads(raw, object_pairs_hook=strict_object))
        cases.append(expected)
        fixtures.append({"case_id": identifier, "fixture_sha256": sha256(raw),
                         "expected_sha256": sha256(json_bytes(expected))})
    expected_raw = json_bytes({"profile": PROFILE, "cases": cases})
    manifest = json_bytes({"schema_version": 1, "profile": PROFILE, "control_count": CONTROL_COUNT,
                           "fixtures": fixtures, "expected_sha256": sha256(expected_raw)})
    return {**inputs, "expected.json": expected_raw, "controls_manifest.json": manifest}


def generated_fixtures() -> list[dict[str, Any]]:
    output, unique = [], set()
    for seed_index, seed in enumerate(SEEDS):
        for sequence in range(CASES_PER_SEED):
            rng = random.Random(seed ^ ((sequence + 1) * 0x9E3779B97F4A7C15))
            if sequence < CASES_PER_SEED // 2:
                ax, bl = rng.getrandbits(16), 0
            else:
                while True:
                    dividend = rng.randint(-0x8000, 0x7FFF)
                    divisor = rng.choice(tuple(range(-128, 0)) + tuple(range(1, 128)))
                    if not -128 <= truncating_quotient(dividend, divisor) <= 127:
                        ax, bl = dividend & 0xFFFF, divisor & 0xFF
                        break
            gprs = [rng.getrandbits(64) for _ in range(16)]
            gprs[0] = (gprs[0] & ~0xFFFF) | ax
            gprs[3] = (gprs[3] & ~0xFF) | bl
            gprs[15] = 0x42AD000000000000 | (seed_index << 32) | sequence
            flags = 2 | (rng.getrandbits(64) & (RFLAGS_DEFINED & ~2))
            item = make_fixture(generated_id(seed_index, sequence), ax, bl, gprs, flags,
                f"Deterministic S2-42ad corpus seed={hx(seed)}, sequence={sequence}; fault class selected by integer-only oracle.", seed)
            initial = item["initial_state"]
            key = (int(initial["gpr"][0], 16) & 0xFFFF, int(initial["gpr"][3], 16) & 0xFF,
                   tuple(initial["gpr"]), initial["rflags"])
            unique.add(key)
            output.append(item)
    if len(output) != GENERATED_COUNT or len(unique) != GENERATED_COUNT:
        raise CorpusError("generated architectural input states are not unique")
    if sum((int(x["initial_state"]["gpr"][3], 16) & 0xFF) == 0 for x in output) != GENERATED_COUNT // 2:
        raise CorpusError("generated fault classes are not split evenly")
    return output


def expected_snapshot(item: dict[str, Any]) -> dict[str, Any]:
    code = bytes.fromhex(item["memory"][0]["initial_bytes_hex"])
    initial = item["initial_state"]
    gprs = [int(value, 16) for value in initial["gpr"]]
    ax, bl = gprs[0] & 0xFFFF, gprs[3] & 0xFF
    if code != CODE or not is_idiv_fault(ax, bl):
        raise CorpusError("independent oracle rejects non-faulting input: only BL=0 or signed AX/BL overflow is admitted")
    return {"case_id": item["case_id"],
            "state": {"mode": "long64", "gpr": [hx(value) for value in gprs],
                      "rip": hx(CODE_ADDRESS), "rflags": hx(int(initial["rflags"], 16)),
                      "rflags_defined": hx(RFLAGS_DEFINED)},
            "memory": [{"region_id": "code", "guest_address": hx(CODE_ADDRESS), "bytes_hex": CODE.hex()}],
            "outcome": {"kind": "exception"},
            "instructions_executed": {"status": "available", "value": hx(0)},
            "exception": {"status": "available", "value": {"vector": 0,
                "error_code": {"status": "unavailable", "reason": "exception has no architectural error code"},
                "faulting_rip": hx(CODE_ADDRESS),
                "fault_address": {"status": "unavailable", "reason": "exception has no reported linear address"}}}}


def load_controls() -> tuple[list[tuple[str, bytes, dict]], bytes]:
    manifest_path, expected_path = CONTROL_DIR / "controls_manifest.json", CONTROL_DIR / "expected.json"
    manifest_raw, expected_raw = manifest_path.read_bytes(), expected_path.read_bytes()
    if CONTROL_MANIFEST_SHA256 == "TO_BE_PINNED" or sha256(manifest_raw) != CONTROL_MANIFEST_SHA256:
        raise CorpusError("pinned control manifest digest differs")
    manifest = json.loads(manifest_raw, object_pairs_hook=strict_object)
    frozen = json.loads(expected_raw, object_pairs_hook=strict_object)
    if (set(manifest) != {"schema_version", "profile", "control_count", "fixtures", "expected_sha256"}
            or manifest.get("schema_version") != 1 or manifest.get("profile") != PROFILE
            or manifest.get("control_count") != CONTROL_COUNT or len(manifest.get("fixtures", [])) != CONTROL_COUNT
            or manifest.get("expected_sha256") != sha256(expected_raw)
            or set(frozen) != {"profile", "cases"} or frozen.get("profile") != PROFILE
            or len(frozen.get("cases", [])) != CONTROL_COUNT):
        raise CorpusError("static control bundle has an invalid strict shape")
    expected_by_id = {row["case_id"]: row for row in frozen["cases"]}
    if list(expected_by_id) != [control_id(i) for i in range(CONTROL_COUNT)]:
        raise CorpusError("pinned control expected cases are duplicated or out of order")
    fixed, controls, names = control_inputs(), [], {"controls_manifest.json", "expected.json"}
    for index in range(CONTROL_COUNT):
        identifier, name = control_id(index), f"{control_id(index)}.case.json"
        raw = (CONTROL_DIR / name).read_bytes()
        expected = expected_by_id[identifier]
        row = manifest["fixtures"][index]
        if (raw != fixed[name] or row != {"case_id": identifier, "fixture_sha256": sha256(raw),
                "expected_sha256": sha256(json_bytes(expected))}
                or expected != expected_snapshot(json.loads(raw, object_pairs_hook=strict_object))):
            raise CorpusError(f"pinned control differs: {identifier}")
        controls.append((identifier, raw, expected)); names.add(name)
    entries = list(CONTROL_DIR.iterdir())
    if any(path.is_symlink() or not path.is_file() for path in entries) or {path.name for path in entries} != names:
        raise CorpusError("control directory has missing, extra, or symlink entries")
    return controls, manifest_raw


def all_cases() -> tuple[list[tuple[str, bytes, str, dict]], bytes]:
    controls, control_manifest = load_controls()
    rows = [(identifier, raw, "handwritten", expected) for identifier, raw, expected in controls]
    for item in generated_fixtures():
        raw = json_bytes(item)
        rows.append((item["case_id"], raw, "generated", expected_snapshot(item)))
    return rows, control_manifest


def materialize() -> dict[str, bytes]:
    rows, control_manifest = all_cases()
    files, digests, snapshots = {}, [], []
    for identifier, raw, kind, expected in rows:
        files[f"{identifier}.case.json"] = raw
        snapshots.append(expected)
        digests.append({"case_id": identifier, "kind": kind, "fixture_sha256": sha256(raw),
                        "expected_sha256": sha256(json_bytes(expected))})
    expected_raw = json_bytes({"profile": PROFILE, "cases": snapshots})
    files["expected.json"] = expected_raw
    files["manifest.json"] = json_bytes({"manifest_version": 1, "generator_version": GENERATOR_VERSION,
        "profile": PROFILE, "seeds": [hx(seed) for seed in SEEDS], "cases_per_seed": CASES_PER_SEED,
        "control_count": CONTROL_COUNT, "generated_case_count": GENERATED_COUNT, "case_count": TOTAL_COUNT,
        "case_digests": digests, "control_manifest_sha256": sha256(control_manifest),
        "expected_sha256": sha256(expected_raw),
        "corpus_digest": sha256("\n".join(row["fixture_sha256"] for row in digests).encode())})
    return files


def verify_corpus(directory: Path) -> dict:
    if directory.is_symlink() or not directory.is_dir():
        raise CorpusError("corpus directory missing or unsafe")
    files = materialize()
    verify_file_set(directory, files)
    manifest = load_json(directory / "manifest.json")
    if (set(manifest) != MANIFEST_FIELDS or manifest.get("case_count") != TOTAL_COUNT
            or len(manifest.get("case_digests", [])) != TOTAL_COUNT):
        raise CorpusError("manifest shape or case accounting differs")
    return manifest


def validate_provenance(build: Any, machine: Any, label: str) -> None:
    if not isinstance(build, dict) or any(not isinstance(build.get(key), str) or not build[key]
            for key in ("revision", "compiler", "target", "profile")):
        raise CorpusError(f"{label}: incomplete build provenance")
    if not isinstance(machine, dict) or any(not isinstance(machine.get(key), str) or not machine[key]
            for key in ("host_os", "host_arch")):
        raise CorpusError(f"{label}: incomplete machine provenance")


def compare_result(result: Any, raw: bytes, expected: dict, build: dict, machine: dict, label: str) -> None:
    result_fields = {"schema_version", "case_id", "input_sha256", "profile", "backend", "outcome",
                     "instructions_executed", "snapshot", "build", "machine"}
    if (not isinstance(result, dict) or set(result) != result_fields
            or type(result.get("schema_version")) is not int):
        raise CorpusError(f"{label}: invalid result object/schema")
    wanted = {"schema_version": 2, "case_id": expected["case_id"], "input_sha256": sha256(raw),
              "profile": PROFILE, "backend": "interpreter", "outcome": expected["outcome"],
              "instructions_executed": expected["instructions_executed"], "build": build, "machine": machine}
    if any(result.get(key) != value for key, value in wanted.items()):
        raise CorpusError(f"{label}: result identity/outcome/retirement/provenance differs")
    validate_provenance(result.get("build"), result.get("machine"), label)
    snapshot = result.get("snapshot")
    if (not isinstance(snapshot, dict) or set(snapshot) != {"status", "value"}
            or snapshot.get("status") != "available" or not isinstance(snapshot.get("value"), dict)):
        raise CorpusError(f"{label}: complete snapshot unavailable")
    actual, state = snapshot["value"], expected["state"]
    if set(actual) != {"state", "rflags_defined", "memory", "minss_memory_rax_controls",
                      "addss_memory_rax_controls", "exception"}:
        raise CorpusError(f"{label}: snapshot fields differ from schema-v2 contract")
    if actual["minss_memory_rax_controls"] is not None or actual["addss_memory_rax_controls"] is not None:
        raise CorpusError(f"{label}: unexpected instruction-control snapshot")
    if not isinstance(actual.get("state"), dict) or set(actual["state"]) != {"mode", "gpr", "rip", "rflags"}:
        raise CorpusError(f"{label}: architectural state fields are incomplete or unknown")
    exception = actual.get("exception")
    if not isinstance(exception, dict) or set(exception) != {"status", "value"}:
        raise CorpusError(f"{label}: exception observation fields are incomplete or unknown")
    exception_value = exception.get("value")
    if exception.get("status") != "available" or not isinstance(exception_value, dict) or set(exception_value) != {
            "vector", "error_code", "faulting_rip", "fault_address"}:
        raise CorpusError(f"{label}: exception metadata fields are incomplete or unknown")
    if type(exception_value.get("vector")) is not int or exception_value["vector"] != 0:
        raise CorpusError(f"{label}: #DE vector must be integer 0")
    for field in ("error_code", "fault_address"):
        unavailable = exception_value[field]
        if (not isinstance(unavailable, dict) or set(unavailable) != {"status", "reason"}
                or unavailable.get("status") != "unavailable" or not isinstance(unavailable.get("reason"), str)
                or not unavailable["reason"]):
            raise CorpusError(f"{label}: {field} must be explicitly unavailable")
    if (actual.get("state") != {key: state[key] for key in ("mode", "gpr", "rip", "rflags")}
            or actual.get("rflags_defined") != state["rflags_defined"]):
        raise CorpusError(f"{label}: full architectural state/definedness differs")
    if actual.get("memory") != expected["memory"]:
        raise CorpusError(f"{label}: full code memory differs")
    if actual.get("exception") != expected["exception"]:
        raise CorpusError(f"{label}: #DE metadata differs")


def expected_ids(manifest: dict) -> list[str]:
    return [row["case_id"] for row in manifest["case_digests"]]


def missing_attempts(ids: list[str], rows: list[dict]) -> list[dict]:
    by_id = {row.get("case_id"): row for row in rows if isinstance(row, dict)}
    missing = []
    for identifier in ids:
        row = by_id.get(identifier, {})
        for repeat in range(REPEATS):
            processes, results = row.get("processes", []), row.get("repeats", [])
            if (repeat >= len(processes) or not isinstance(processes[repeat], dict)
                    or processes[repeat].get("returncode") != 0 or repeat >= len(results)
                    or not isinstance(results[repeat], dict) or "failure" in results[repeat]):
                missing.append({"case_id": identifier, "repeat": repeat + 1})
    return missing


def output_text(value: Any) -> str:
    return value.decode("utf-8", errors="replace") if isinstance(value, bytes) else (value or "")


def process_environment() -> dict:
    return {key: os.environ.get(key) for key in PROCESS_ENV_KEYS}


def command_identity(executable: Path) -> tuple[dict, dict]:
    selected = executable.resolve(strict=True)
    revision = source_revision()
    os_name = {"darwin": "macos"}.get(platform.system().lower(), platform.system().lower())
    arch = {"arm64": "aarch64", "amd64": "x86_64"}.get(platform.machine().lower(), platform.machine().lower())
    return {"executable_sha256": sha256(selected.read_bytes()), "revision": revision}, {"host_os": os_name, "host_arch": arch}


def source_revision() -> str:
    return subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True,
                          capture_output=True, text=True).stdout.strip()


def write_checkpoint(path: Path, directory: Path, manifest: dict, rows: list, failures: list,
                    identity: dict, machine: dict, build: dict | None, runner_machine: dict | None,
                    status: str = "incomplete") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": status, "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE,
        "manifest_sha256": sha256((directory / "manifest.json").read_bytes()), "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "execution_identity": identity, "execution_machine": machine,
        "runner_build": build, "runner_machine": runner_machine, "results": rows,
        "failures": failures, "missing_attempts": missing_attempts(expected_ids(manifest), rows)}))


def run_interpreter(directory: Path, result_path: Path, executable: Path) -> dict:
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_bytes(json_bytes({"schema_version": 1, "status": "incomplete", "backend": "interpreter",
        "native_x86": "not_run_form_not_admitted", "profile": PROFILE, "case_count": TOTAL_COUNT,
        "repeats_per_case": REPEATS, "results": [], "failures": ["preflight incomplete"],
        "missing_attempts": missing_attempts([control_id(i) for i in range(CONTROL_COUNT)] +
            [generated_id(si, seq) for si in range(len(SEEDS)) for seq in range(CASES_PER_SEED)], [])}))
    manifest = verify_corpus(directory)
    identity, machine = command_identity(executable)
    expected = {row["case_id"]: row for row in load_json(directory / "expected.json")["cases"]}
    rows, failures, build, runner_machine = [], [], None, None
    env = process_environment()
    child_env = {key: value for key, value in env.items() if value is not None}
    for digest in manifest["case_digests"]:
        identifier = digest["case_id"]
        fixture_path = (directory / f"{identifier}.case.json").resolve(strict=True)
        raw = fixture_path.read_bytes()
        row = {"case_id": identifier, "fixture_sha256": sha256(raw), "repeats": [], "processes": []}
        rows.append(row)
        for repeat in range(REPEATS):
            command = [str(executable.resolve(strict=True)), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"]
            try:
                proc = subprocess.run(command, capture_output=True, text=True, check=False,
                    timeout=RUNNER_TIMEOUT_SECONDS, cwd=str(ROOT.resolve()), env=child_env)
                process = {"returncode": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr,
                           "command": command, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None}
                row["processes"].append(process)
                if proc.returncode != 0:
                    raise CorpusError(f"runner exited {proc.returncode}: {proc.stderr.strip()}")
                observation = json.loads(proc.stdout, object_pairs_hook=strict_object)
                if not isinstance(observation, dict):
                    raise CorpusError("runner output must be one JSON object")
                b, m = observation.get("build"), observation.get("machine")
                validate_provenance(b, m, "runner observation")
                if b["revision"] != identity["revision"] or {k: m[k] for k in ("host_os", "host_arch")} != machine:
                    raise CorpusError("runner source or host provenance differs")
                if build is None:
                    build, runner_machine = b, m
                if b != build or m != runner_machine:
                    raise CorpusError("runner provenance changed during run")
                compare_result(observation, raw, expected[identifier], build, runner_machine, f"{identifier} repeat {repeat + 1}")
                row["repeats"].append(observation)
            except KeyboardInterrupt as error:
                row["processes"].append({"returncode": None, "stdout": "", "stderr": "interrupted", "command": command,
                    "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": "interrupted", "returncode": None})
                failures.append(f"{identifier} repeat {repeat + 1}: interrupted")
                write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
                raise CorpusError(f"interpreter run interrupted; checkpoint saved: {result_path}") from error
            except subprocess.TimeoutExpired as error:
                message = str(error)
                row["processes"].append({"returncode": None, "stdout": output_text(error.stdout),
                    "stderr": output_text(error.stderr), "command": command, "cwd": str(ROOT.resolve()),
                    "environment": env, "timeout_diagnosis": {"kind": "timeout_expired",
                    "timeout_seconds": RUNNER_TIMEOUT_SECONDS, "message": message}})
                row["repeats"].append({"failure": message, "returncode": None})
                failures.append(f"{identifier} repeat {repeat + 1}: timed out: {message}")
            except (OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError, CorpusError) as error:
                if len(row["processes"]) == repeat:
                    stdout = output_text(error.object) if isinstance(error, UnicodeError) else ""
                    row["processes"].append({"returncode": None, "stdout": stdout, "stderr": str(error),
                        "command": command, "cwd": str(ROOT.resolve()), "environment": env, "timeout_diagnosis": None})
                row["repeats"].append({"failure": str(error), "returncode": row["processes"][-1]["returncode"]})
                failures.append(f"{identifier} repeat {repeat + 1}: {error}")
        if (len(row["repeats"]) == REPEATS and all("failure" not in item for item in row["repeats"])
                and row["repeats"][0] != row["repeats"][1]):
            failures.append(f"{identifier}: repeat results differ")
        write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine)
    status = "failed" if failures else "passed"
    write_checkpoint(result_path, directory, manifest, rows, failures, identity, machine, build, runner_machine, status)
    if failures:
        raise CorpusError(f"{len(failures)} interpreter failures; saved {result_path}")
    return load_json(result_path)


def verify_results(directory: Path, result_path: Path, executable: Path) -> None:
    manifest = verify_corpus(directory)
    result = load_json(result_path)
    result_fields = {"schema_version", "status", "backend", "native_x86", "profile", "manifest_sha256",
                     "case_count", "repeats_per_case", "execution_identity", "execution_machine",
                     "runner_build", "runner_machine", "results", "failures", "missing_attempts"}
    if (not isinstance(result, dict) or set(result) != result_fields
            or type(result.get("schema_version")) is not int or result.get("schema_version") != 1
            or result.get("status") != "passed" or result.get("backend") != "interpreter"
            or result.get("native_x86") != "not_run_form_not_admitted" or result.get("profile") != PROFILE
            or result.get("failures") != [] or result.get("missing_attempts") != []):
        raise CorpusError("saved evidence is not a successful local interpreter record")
    if (result.get("manifest_sha256") != sha256((directory / "manifest.json").read_bytes())
            or result.get("case_count") != TOTAL_COUNT or result.get("repeats_per_case") != REPEATS):
        raise CorpusError("saved results/corpus identity differs")
    build, runner_machine = result.get("runner_build"), result.get("runner_machine")
    validate_provenance(build, runner_machine, "saved evidence")
    identity, machine = result.get("execution_identity"), result.get("execution_machine")
    executable = executable.resolve(strict=True)
    if (not isinstance(identity, dict) or set(identity) != {"executable_sha256", "revision"}
            or identity.get("executable_sha256") != sha256(executable.read_bytes())
            or identity.get("revision") != build["revision"] or identity.get("revision") != source_revision()
            or machine != {key: runner_machine[key] for key in ("host_os", "host_arch")}):
        raise CorpusError("saved executable/source/host provenance differs")
    rows = result.get("results")
    if (not isinstance(rows, list) or any(not isinstance(row, dict) for row in rows)
            or [row.get("case_id") for row in rows] != expected_ids(manifest)):
        raise CorpusError("saved result order/count differs")
    expected = {row["case_id"]: row for row in load_json(directory / "expected.json")["cases"]}
    env0 = None
    for row in rows:
        if not isinstance(row, dict) or set(row) != {"case_id", "fixture_sha256", "repeats", "processes"}:
            raise CorpusError("saved result row has missing or unrecognized fields")
        identifier = row["case_id"]
        fixture_path = (directory / f"{identifier}.case.json").resolve(strict=True)
        raw = fixture_path.read_bytes()
        repeats, processes = row.get("repeats"), row.get("processes")
        if (row.get("fixture_sha256") != sha256(raw) or not isinstance(repeats, list)
                or not isinstance(processes, list) or len(repeats) != REPEATS or len(processes) != REPEATS):
            raise CorpusError(f"saved {identifier}: input hash/repeat count differs")
        for index, (observed, process) in enumerate(zip(repeats, processes), 1):
            if (not isinstance(process, dict) or set(process) != {"returncode", "stdout", "stderr", "command", "cwd", "environment", "timeout_diagnosis"}
                    or type(process.get("returncode")) is not int or process["returncode"] != 0
                    or not isinstance(process.get("stdout"), str) or not isinstance(process.get("stderr"), str)
                    or process.get("timeout_diagnosis") is not None):
                raise CorpusError(f"saved {identifier} repeat {index}: invalid process evidence")
            if (process["command"] != [str(executable), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"]
                    or process["cwd"] != str(ROOT.resolve())):
                raise CorpusError(f"saved {identifier} repeat {index}: exact replay command/cwd differs")
            env = process.get("environment")
            if (not isinstance(env, dict) or set(env) != set(PROCESS_ENV_KEYS)
                    or any(value is not None and not isinstance(value, str) for value in env.values())
                    or (env0 is not None and env != env0)):
                raise CorpusError(f"saved {identifier} repeat {index}: environment provenance differs")
            env0 = env
            parsed = json.loads(process["stdout"], object_pairs_hook=strict_object)
            if parsed != observed:
                raise CorpusError(f"saved {identifier} repeat {index}: frozen stdout/result differs")
            compare_result(observed, raw, expected[identifier], build, runner_machine, f"saved {identifier} repeat {index}")
        if repeats[0] != repeats[1]:
            raise CorpusError(f"saved {identifier}: repeats differ")


def write_native_blocked(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes({"schema_version": 1, "status": "blocked", "profile": PROFILE,
        "native_x86": "not_run_form_not_admitted", "blocked_reason": "S2-42ad explicitly blocks native execution because IDIV BL fault delivery is not admitted; no remote host is contacted.",
        "backend_execution": "not_started"}))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_mutually_exclusive_group(required=True)
    for action in ("generate-controls", "generate", "verify", "run-interpreter", "verify-results", "native"):
        actions.add_argument("--" + action, action="store_true")
    parser.add_argument("--corpus", type=Path, default=CORPUS_DIR)
    parser.add_argument("--results", type=Path)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner")
    args = parser.parse_args(argv)
    try:
        if args.generate_controls:
            write_file_set(CONTROL_DIR, materialize_controls())
            print(f"wrote {CONTROL_COUNT} S2-42ad pinned controls")
        elif args.generate:
            write_file_set(args.corpus, materialize()); print(f"generated {TOTAL_COUNT} S2-42ad fixtures")
        elif args.verify:
            manifest = verify_corpus(args.corpus); print(f"verified {TOTAL_COUNT} cases; digest {manifest['corpus_digest']}")
        elif args.run_interpreter:
            run_interpreter(args.corpus, args.results or RESULTS_PATH, args.runner)
            print(f"passed {TOTAL_COUNT} cases × {REPEATS} local interpreter runs; native_x86=not_run_form_not_admitted")
        elif args.verify_results:
            verify_results(args.corpus, args.results or RESULTS_PATH, args.runner); print("verified saved interpreter replay")
        else:
            write_native_blocked(args.results or NATIVE_PATH)
            print("native suite blocked before backend invocation; no remote host contacted")
            return 1
    except (CorpusError, OSError, subprocess.SubprocessError, UnicodeError, json.JSONDecodeError) as error:
        print(f"S2-42ad failed closed: {error}", file=sys.stderr); return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
