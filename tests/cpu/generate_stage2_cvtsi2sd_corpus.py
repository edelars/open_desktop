#!/usr/bin/env python3
"""Generate and execute the interpreter-only S2-42m CVTSI2SD corpus."""

from __future__ import annotations

import argparse
import copy
import json
import platform
import random
import subprocess
import sys
from pathlib import Path
from typing import Any

from stage2_corpus_support import CorpusError, json_bytes, sha256, strict_object, verify_file_set, write_file_set


ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = ROOT / "tests/fixtures/x86/stage2/s2_37n_cvtsi2sd_r32_zero.case.json"
DEFAULT_CORPUS = ROOT / "work/stage2/s2-42m-cvtsi2sd/corpus"
DEFAULT_RESULTS = ROOT / "work/stage2/s2-42m-cvtsi2sd/results.json"
PROFILE = "s2-long-cvtsi2sd-r32-exact-v1"
CODE = "f20f2ac0"
ENTRY = 0x100000
MXCSR_MASK = 0xFFBF
SEEDS = (0x42_0000_0001, 0x42_0000_0002, 0x42_0000_0003, 0x42_0000_0004)
CASES_PER_SEED = 256
REPEATS = 2
HANDWRITTEN_BITS = {
    0: 0x0000000000000000,
    1: 0x3FF0000000000000,
    -1: 0xBFF0000000000000,
    -(1 << 31): 0xC1E0000000000000,
    (1 << 31) - 1: 0x41DFFFFFFFC00000,
    (1 << 24) - 1: 0x416FFFFFE0000000,
    -((1 << 24) - 1): 0xC16FFFFFE0000000,
    (1 << 24) + 1: 0x4170000010000000,
    -((1 << 24) + 1): 0xC170000010000000,
}


def binary64_bits(value: int) -> int:
    """Exact integer-to-binary64 encoding for every signed 32-bit integer."""
    if not -(1 << 31) <= value < (1 << 31):
        raise CorpusError("CVTSI2SD input is outside signed i32")
    if value == 0:
        return 0
    sign = 1 if value < 0 else 0
    magnitude = -value if value < 0 else value
    top_bit = magnitude.bit_length() - 1
    exponent = 1023 + top_bit
    if top_bit <= 52:
        significand = magnitude << (52 - top_bit)
    else:
        significand = magnitude >> (top_bit - 52)
    fraction = significand & ((1 << 52) - 1)
    return (sign << 63) | (exponent << 52) | fraction


def boundary_values() -> tuple[int, ...]:
    values = {0, 1, -1, -(1 << 31), (1 << 31) - 1}
    for bit in range(1, 32):
        power = 1 << bit
        for value in (power - 1, power, power + 1, -power - 1, -power, -power + 1):
            if -(1 << 31) <= value < (1 << 31):
                values.add(value)
    for magnitude in ((1 << 24) - 1, (1 << 24) + 1):
        values.update((magnitude, -magnitude))
    return tuple(sorted(values))


def controls(seed_index: int, sequence: int) -> tuple[int, bool]:
    rng = random.Random((SEEDS[seed_index] << 16) ^ sequence)
    rc = sequence & 3
    sticky = rng.randrange(1 << 6)
    masks = rng.randrange(1 << 6) << 7
    ftz = (rng.randrange(2) << 15)
    mxcsr = (rc << 13) | sticky | masks | ftz
    assert mxcsr & ~MXCSR_MASK == 0 and not (mxcsr & (1 << 6))
    return mxcsr, bool((sequence // 2 + seed_index) & 1)


def random_inputs(seed_index: int) -> list[int]:
    rng = random.Random(SEEDS[seed_index])
    bounds = boundary_values()
    result: list[int] = []
    for index in range(CASES_PER_SEED):
        if index % 4 == 0:
            value = bounds[(seed_index * 31 + index // 4) % len(bounds)]
        elif index % 4 == 1:
            value = rng.randint(-(1 << 31), (1 << 31) - 1)
        elif index % 4 == 2:
            value = rng.choice(bounds)
        else:
            value = rng.getrandbits(32)
            if value & 0x80000000:
                value -= 1 << 32
        result.append(value)
    if len(set(result)) != len(result):
        # Deterministically re-roll collisions without losing boundary density.
        seen: set[int] = set()
        for index, value in enumerate(result):
            if value in seen:
                candidate = rng.randint(-(1 << 31), (1 << 31) - 1)
                while candidate in seen:
                    candidate = rng.randint(-(1 << 31), (1 << 31) - 1)
                result[index] = candidate
            seen.add(result[index])
    return result


def case_id(seed: int, sequence: int) -> str:
    return f"s2_42m_cvtsi2sd_{seed:016x}_{sequence:04x}"


def make_fixture(template: dict[str, Any], identifier: str, seed: int | None,
                 value: int, mxcsr: int, osxmmexcpt: bool, handwritten: bool) -> dict[str, Any]:
    fixture = copy.deepcopy(template)
    fixture["case_id"] = identifier
    fixture["seed"] = None if handwritten else f"0x{seed:016x}"
    fixture["provenance"] = {
        "kind": "handwritten_control" if handwritten else "generated_project_fixture",
        "source": ("S2-42m fixed control: independently specified signed i32 input and exact integer bit oracle."
                   if handwritten else f"S2-42m deterministic corpus seed=0x{seed:016x}, sequence={int(identifier.rsplit('_', 1)[1], 16)}."),
        "specification": "Intel 64 and IA-32 Architectures Software Developer’s Manual, Rev. 093, Vol. 2A, CVTSI2SD.",
        "specification_version": "Intel SDM revision 093; S2-42m.",
    }
    state = fixture["initial_state"]
    state["gpr"][0] = f"0x{(0x12345678 << 32) | (value & 0xffffffff):016x}"
    state["sse"]["mxcsr"] = f"0x{mxcsr:016x}"
    controls_value = fixture["cvtsi2sd_r32_controls"]
    controls_value["cr4_osxmmexcpt"] = osxmmexcpt
    bits = HANDWRITTEN_BITS[value] if handwritten else binary64_bits(value)
    fixture["expected_bits"] = f"0x{bits:016x}"
    fixture["input_i32"] = value
    fixture["handwritten_control"] = handwritten
    return fixture


def corpus_files() -> dict[str, bytes]:
    template = json.loads(TEMPLATE.read_text(encoding="utf-8"))
    fixtures: list[tuple[str, dict[str, Any]]] = []
    for seed_index, seed in enumerate(SEEDS):
        for sequence, value in enumerate(random_inputs(seed_index)):
            mxcsr, osxmmexcpt = controls(seed_index, sequence)
            identifier = case_id(seed, sequence)
            fixtures.append((identifier, make_fixture(template, identifier, seed, value, mxcsr, osxmmexcpt, False)))

    # Literal cases intentionally do not derive their inputs from the matrix PRNG.
    fixed = (0, 1, -1, -(1 << 31), (1 << 31) - 1,
             (1 << 24) - 1, -((1 << 24) - 1), (1 << 24) + 1, -((1 << 24) + 1))
    for index, value in enumerate(fixed):
        mxcsr = ((index & 3) << 13) | ((0x15 + index) & 0x3f) | (((0x2a ^ index) & 0x3f) << 7) | ((index & 1) << 15)
        fixtures.append((f"s2_42m_cvtsi2sd_hand_{index:02d}", make_fixture(
            template, f"s2_42m_cvtsi2sd_hand_{index:02d}", None, value, mxcsr, bool(index & 1), True)))

    identifiers = [identifier for identifier, _ in fixtures]
    if len(identifiers) != len(set(identifiers)) or len(fixtures) < 1024:
        raise CorpusError("generated fixture identifiers/count are invalid")
    expected: dict[str, dict[str, Any]] = {}
    files: dict[str, bytes] = {}
    for identifier, fixture in fixtures:
        # expected state is computed solely from the fixture inputs and independent integer encoder.
        before = fixture["initial_state"]
        after = copy.deepcopy(before)
        after["rip"] = f"0x{ENTRY + 4:016x}"
        xmm0 = after["sse"]["xmm"][0]
        bits = int(fixture["expected_bits"], 16)
        low = bits.to_bytes(8, "little").hex()
        after["sse"]["xmm"][0] = low + xmm0[16:]
        expected[identifier] = {
            "case_id": identifier,
            "input_i32": fixture["input_i32"],
            "binary64_bits": f"0x{bits:016x}",
            "state": after,
            "rflags_defined": "0x0000000000000cd7",
            "outcome": {"kind": "stop_address"},
            "instructions_executed": "0x0000000000000001",
            "exception": None,
            "memory": [{"region_id": "code", "guest_address": f"0x{ENTRY:016x}", "bytes_hex": CODE}],
            "cvtsi2sd_r32_controls": fixture["cvtsi2sd_r32_controls"],
        }
        # Runner input follows the schema-v2 fixture; corpus-only oracle data is separate.
        runner_fixture = {k: v for k, v in fixture.items() if k not in ("expected_bits", "input_i32", "handwritten_control")}
        files[f"{identifier}.case.json"] = json_bytes(runner_fixture)
    manifest = {
        "schema_version": 1,
        "corpus_id": "s2-42m-cvtsi2sd-r32-interpreter-v1",
        "profile": PROFILE,
        "backend_policy": {"selected": "interpreter", "native_x86": "closed_not_run", "jit": "not_selected"},
        "instruction": {"mode": "long64", "bytes_hex": CODE, "budget": 1, "entry": f"0x{ENTRY:016x}", "stop_rip": f"0x{ENTRY+4:016x}"},
        "seeds": [f"0x{seed:016x}" for seed in SEEDS],
        "generated_per_seed": CASES_PER_SEED,
        "repeat_count": REPEATS,
        "case_ids": identifiers,
        "fixtures": {name: sha256(raw) for name, raw in files.items()},
        "expected": {f"{identifier}.expected.json": sha256(json_bytes(record)) for identifier, record in expected.items()},
    }
    files.update({f"{identifier}.expected.json": json_bytes(record) for identifier, record in expected.items()})
    files["manifest.json"] = json_bytes(manifest)
    return files


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=strict_object)


def verify_corpus(directory: Path) -> tuple[dict[str, bytes], dict[str, Any]]:
    files = corpus_files()
    verify_file_set(directory, files)
    manifest = load_json(directory / "manifest.json")
    if manifest.get("profile") != PROFILE or manifest.get("case_ids") != [
            name.removesuffix(".case.json") for name in files if name.endswith(".case.json")]:
        raise CorpusError("manifest profile or ordered case IDs differ")
    return files, manifest


def expected_for(directory: Path, identifier: str) -> dict[str, Any]:
    return load_json(directory / f"{identifier}.expected.json")


def compare_result(result: dict[str, Any], fixture_raw: bytes, expected: dict[str, Any],
                   build: dict[str, Any], machine: dict[str, Any], label: str) -> None:
    wanted = {
        "schema_version": 2,
        "case_id": expected["case_id"],
        "input_sha256": sha256(fixture_raw),
        "profile": PROFILE,
        "backend": "interpreter",
        "outcome": expected["outcome"],
        "instructions_executed": {"status": "available", "value": expected["instructions_executed"]},
        "build": build,
        "machine": machine,
    }
    for key, value in wanted.items():
        if result.get(key) != value:
            raise CorpusError(f"{label}: result {key} differs from run provenance/contract")
    snap = result.get("snapshot")
    if not isinstance(snap, dict) or snap.get("status") != "available" or not isinstance(snap.get("value"), dict):
        raise CorpusError(f"{label}: snapshot unavailable")
    snapshot = snap["value"]
    state = snapshot.get("state")
    if state != expected["state"]:
        raise CorpusError(f"{label}: full architectural state differs (GPR/XMM/MXCSR/RFLAGS/RIP)")
    if snapshot.get("rflags_defined") != expected["rflags_defined"]:
        raise CorpusError(f"{label}: RFLAGS definedness differs")
    if snapshot.get("memory") != expected["memory"]:
        raise CorpusError(f"{label}: code mapping differs")
    if snapshot.get("cvtsi2sd_r32_controls") != expected["cvtsi2sd_r32_controls"]:
        raise CorpusError(f"{label}: CVTSI2SD controls differ")
    exception = snapshot.get("exception")
    if not isinstance(exception, dict) or exception.get("status") != "available" or exception.get("value") is not expected["exception"]:
        raise CorpusError(f"{label}: exception result differs")


def validate_provenance(build: Any, machine: Any, label: str) -> None:
    if not isinstance(build, dict) or any(
            not isinstance(build.get(key), str) or not build[key]
            for key in ("revision", "target", "profile", "compiler")):
        raise CorpusError(f"{label}: build provenance is incomplete")
    if not isinstance(machine, dict) or any(
            not isinstance(machine.get(key), str) or not machine[key]
            for key in ("host_os", "host_arch")):
        raise CorpusError(f"{label}: machine provenance is incomplete")


def command_identity(executable: Path) -> tuple[dict[str, Any], dict[str, Any]]:
    digest = sha256(executable.read_bytes())
    revision = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()
    build = {"executable_sha256": digest, "revision": revision}
    machine = {"host_os": platform.system().lower(), "host_arch": platform.machine().lower()}
    return build, machine


def run_interpreter(directory: Path, output_path: Path, executable: Path) -> dict[str, Any]:
    _, manifest = verify_corpus(directory)
    execution_identity, execution_machine = command_identity(executable)
    reported_build: dict[str, Any] | None = None
    reported_machine: dict[str, Any] | None = None
    rows: list[dict[str, Any]] = []
    failures: list[str] = []
    ordered = manifest["case_ids"]
    for identifier in ordered:
        fixture_path = directory / f"{identifier}.case.json"
        raw = fixture_path.read_bytes()
        expected = expected_for(directory, identifier)
        repeats: list[dict[str, Any]] = []
        for repeat in range(REPEATS):
            completed = subprocess.run([str(executable), "--fixture", str(fixture_path), "--engine", "interpreter", "--json"],
                                       capture_output=True, text=True)
            try:
                if completed.returncode != 0:
                    raise CorpusError(f"runner exited {completed.returncode}: {completed.stderr.strip()}")
                result = load_json_text(completed.stdout)
                current_build = result.get("build")
                current_machine = result.get("machine")
                validate_provenance(current_build, current_machine, "runner result")
                if reported_build is None:
                    reported_build = current_build
                    reported_machine = current_machine
                if current_build != reported_build or current_machine != reported_machine:
                    raise CorpusError("runner build/machine identity changed during corpus execution")
                compare_result(result, raw, expected, reported_build, reported_machine,
                               f"{identifier} repeat {repeat + 1}")
                repeats.append(result)
            except (CorpusError, json.JSONDecodeError, OSError, subprocess.SubprocessError) as error:
                failures.append(f"{identifier} repeat {repeat + 1}: {error}")
                repeats.append({"failure": str(error), "returncode": completed.returncode})
        if len(repeats) == 2 and "failure" not in repeats[0] and "failure" not in repeats[1] and repeats[0] != repeats[1]:
            failures.append(f"{identifier}: repeat results are not byte-identical JSON objects")
        rows.append({"case_id": identifier, "input_sha256": sha256(raw), "repeats": repeats})
    summary = {
        "schema_version": 1, "status": "failed" if failures else "passed", "backend": "interpreter",
        "native_x86": "not_run_profile_closed", "profile": PROFILE,
        "execution_identity": execution_identity, "execution_machine": execution_machine,
        "runner_build": reported_build, "runner_machine": reported_machine,
        "manifest_sha256": sha256((directory / "manifest.json").read_bytes()),
        "case_count": len(ordered), "repeats_per_case": REPEATS, "results": rows, "failures": failures,
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_bytes(json_bytes(summary))
    if failures:
        raise CorpusError(f"{len(failures)} interpreter corpus failure(s); summary: {output_path}")
    return summary


def load_json_text(text: str) -> Any:
    return json.loads(text, object_pairs_hook=strict_object)


def verify_saved(directory: Path, results_path: Path) -> None:
    _, manifest = verify_corpus(directory)
    record = load_json(results_path)
    if (record.get("schema_version") != 1 or record.get("status") != "passed"
            or record.get("backend") != "interpreter" or record.get("profile") != PROFILE
            or record.get("failures") != []):
        raise CorpusError("saved result is not a passed interpreter record for the exact profile")
    if record.get("native_x86") != "not_run_profile_closed":
        raise CorpusError("saved result misrepresents native-closed profile status")
    if record.get("manifest_sha256") != sha256((directory / "manifest.json").read_bytes()):
        raise CorpusError("saved result manifest identity differs")
    execution_identity = record.get("execution_identity")
    execution_machine = record.get("execution_machine")
    runner_build = record.get("runner_build")
    runner_machine = record.get("runner_machine")
    if (not isinstance(execution_identity, dict) or not isinstance(execution_identity.get("executable_sha256"), str)
            or not isinstance(execution_machine, dict) or not isinstance(runner_build, dict)
            or not isinstance(runner_machine, dict)):
        raise CorpusError("saved result lacks local executable/runner build and machine identity")
    executable_digest = execution_identity.get("executable_sha256", "")
    if (len(executable_digest) != 64 or any(character not in "0123456789abcdef" for character in executable_digest)
            or not isinstance(execution_identity.get("revision"), str) or not execution_identity["revision"]):
        raise CorpusError("saved result local executable identity is malformed")
    validate_provenance(runner_build, runner_machine, "saved result")
    if any(not isinstance(execution_machine.get(key), str) or not execution_machine[key]
           for key in ("host_os", "host_arch")):
        raise CorpusError("saved result local machine identity is incomplete")
    if record.get("case_count") != len(manifest["case_ids"]) or record.get("repeats_per_case") != REPEATS:
        raise CorpusError("saved result case/repeat count differs")
    rows = record.get("results")
    if not isinstance(rows, list) or [row.get("case_id") for row in rows] != manifest["case_ids"]:
        raise CorpusError("saved result ordered case IDs differ")
    for row in rows:
        identifier = row["case_id"]
        raw = (directory / f"{identifier}.case.json").read_bytes()
        expected = expected_for(directory, identifier)
        if row.get("input_sha256") != sha256(raw) or len(row.get("repeats", [])) != REPEATS:
            raise CorpusError(f"saved result {identifier}: fixture digest/repeat count differs")
        for repeat, result in enumerate(row["repeats"], 1):
            if not isinstance(result, dict) or "failure" in result:
                raise CorpusError(f"saved result {identifier} repeat {repeat} contains failure")
            compare_result(result, raw, expected, runner_build, runner_machine,
                           f"saved {identifier} repeat {repeat}")
        if row["repeats"][0] != row["repeats"][1]:
            raise CorpusError(f"saved result {identifier}: interpreter repeats are not identical")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--generate", action="store_true")
    group.add_argument("--verify", action="store_true")
    group.add_argument("--run-interpreter", action="store_true")
    group.add_argument("--verify-results", action="store_true")
    group.add_argument("--native", action="store_true", help="explicitly rejected: profile is native-closed")
    parser.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--results", type=Path, default=DEFAULT_RESULTS)
    parser.add_argument("--runner", type=Path, default=ROOT / "target/debug/od-runner")
    args = parser.parse_args(argv)
    try:
        if args.native:
            message = "native_x86 is closed for this profile; native execution is not admitted"
            args.results.parent.mkdir(parents=True, exist_ok=True)
            args.results.write_bytes(json_bytes({
                "schema_version": 1, "status": "failed", "backend": "native_x86",
                "native_x86": "closed_not_run", "profile": PROFILE, "failures": [message],
            }))
            raise CorpusError(message)
        if args.generate:
            files = corpus_files()
            write_file_set(args.corpus, files)
            print(f"generated {sum(n.endswith('.case.json') for n in files)} fixtures at {args.corpus}")
        elif args.verify:
            _, manifest = verify_corpus(args.corpus)
            print(f"verified {len(manifest['case_ids'])} fixtures and exact manifest/hash/file set")
        elif args.run_interpreter:
            args.results.parent.mkdir(parents=True, exist_ok=True)
            args.results.write_bytes(json_bytes({
                "schema_version": 1, "status": "failed", "backend": "interpreter",
                "native_x86": "not_run_profile_closed", "profile": PROFILE,
                "failures": ["interpreter run did not complete"], "results": [],
            }))
            if not args.runner.is_file():
                raise CorpusError(f"interpreter runner executable is unavailable: {args.runner}")
            summary = run_interpreter(args.corpus, args.results, args.runner)
            print(f"passed {summary['case_count']} cases × {REPEATS} interpreter repeats; native=not_run_profile_closed")
        else:
            verify_saved(args.corpus, args.results)
            print("saved interpreter results verified against manifest, fixture digests, identity, and exact state")
        return 0
    except (CorpusError, OSError, subprocess.SubprocessError, json.JSONDecodeError) as error:
        print(f"S2-42m failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
