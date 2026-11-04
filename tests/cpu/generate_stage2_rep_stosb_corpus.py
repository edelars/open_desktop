#!/usr/bin/env python3
"""Generate, verify and run the deterministic S2-42i REP STOSB corpus."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
from typing import Any

from stage2_corpus_support import (
    CorpusError,
    json_bytes,
    sha256,
    strict_object,
    verify_file_set,
    write_file_set,
)


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CORPUS = ROOT / "work/stage2/s2-42i-rep-stosb"
DEFAULT_ORACLE_ROOT = ROOT / "work/stage2-42i-oracle"
PROFILE = "s2-long-integer-v1"
GENERATOR_VERSION = "s2-42i-rep-stosb-v1"
MANIFEST_VERSION = 1
FIXED_SEEDS = (
    0x243F6A8885A308D3,
    0x13198A2E03707344,
    0xA4093822299F31D0,
    0x082EFA98EC4E6C89,
)
CASES_PER_SEED = 256
GENERATED_CASE_COUNT = len(FIXED_SEEDS) * CASES_PER_SEED
HANDWRITTEN_CASE_COUNT = 36
TOTAL_CASE_COUNT = GENERATED_CASE_COUNT + HANDWRITTEN_CASE_COUNT
COUNTS = tuple(range(9))
DIRECTIONS = (0, 1)
CODE_ADDRESS = 0x00100000
DATA_ADDRESS = 0x00300000
DATA_LENGTH = 32
UNMAPPED_RDI = 0x00000000BAD00000
BASE_RFLAGS = 0x802
DF_MASK = 1 << 10
RFLAGS_MASK = 0x0CD7
MANIFEST_NAME = "manifest.json"
EXPECTED_NAME = "expected.json"


def _hx(value: int) -> str:
    return f"0x{value & 0xFFFFFFFFFFFFFFFF:016x}"


class SplitMix64:
    """Small deterministic PRNG so fixture identity is stable across Python versions."""

    def __init__(self, seed: int, sequence: int) -> None:
        self.state = seed ^ ((sequence * 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF)

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & 0xFFFFFFFFFFFFFFFF
        value = self.state
        value = ((value ^ (value >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
        value = ((value ^ (value >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
        return value ^ (value >> 31)


def _case_id(seed: int, sequence: int) -> str:
    return f"s2_42i_rep_stosb_{seed:016x}_{sequence:04x}"


def _state_rflags(df: int) -> int:
    if df not in DIRECTIONS:
        raise CorpusError("DF must be zero or one")
    return BASE_RFLAGS | (DF_MASK if df else 0)


def _rdi_offset(count: int, df: int, variant: int, random: SplitMix64) -> int | None:
    if not 0 <= count <= 8 or df not in DIRECTIONS:
        raise CorpusError("count/DF is outside the exact S2-42i profile")
    if count == 0:
        if variant == 0:
            return None  # Handwritten proof: zero count never fetches this address.
        return random.next() % DATA_LENGTH
    if variant == 0:
        return 0 if df == 0 else count - 1  # First valid touched span.
    if variant == 1:
        return DATA_LENGTH - count if df == 0 else DATA_LENGTH - 1  # Last valid touched span.
    if variant == 2:
        return 3 if df == 0 else 3 + count - 1  # Unaligned start, range begins at byte 3.
    if df == 0:
        return random.next() % (DATA_LENGTH - count + 1)
    return count - 1 + random.next() % (DATA_LENGTH - count + 1)


def _make_fixture(
    case_id: str,
    seed: int | None,
    sequence: int | None,
    count: int,
    df: int,
    rdi_offset: int | None,
    gpr: list[int],
    data: bytes,
    vector: str,
) -> tuple[dict[str, Any], dict[str, Any]]:
    if not 0 <= count <= 8 or df not in DIRECTIONS or len(gpr) != 16 or len(data) != DATA_LENGTH:
        raise CorpusError("fixture values are outside the S2-42i contract")
    if count and rdi_offset is None:
        raise CorpusError("nonzero REP count requires a fully mapped RDI span")
    if count:
        assert rdi_offset is not None
        low = rdi_offset if df == 0 else rdi_offset - count + 1
        high = rdi_offset + count if df == 0 else rdi_offset + 1
        if low < 0 or high > DATA_LENGTH:
            raise CorpusError("REP destination span is not fully mapped")
    gpr = [value & 0xFFFFFFFFFFFFFFFF for value in gpr]
    gpr[1] = count  # RCX is the full architectural count in long mode.
    gpr[7] = UNMAPPED_RDI if rdi_offset is None else DATA_ADDRESS + rdi_offset
    rflags = _state_rflags(df)
    fixture: dict[str, Any] = {
        "schema_version": 2,
        "case_id": case_id,
        "profile": PROFILE,
        "provenance": {
            "kind": "deterministic_generator" if seed is not None else "handwritten_control",
            "source": (
                f"{GENERATOR_VERSION}; seed=0x{seed:016x}; sequence={sequence}"
                if seed is not None and sequence is not None
                else f"S2-42i fixed REP STOSB {vector} control"
            ),
            "specification": "Intel SDM revision 093, Vol. 2, STOS and REP STOS",
            "specification_version": "Intel SDM revision 093; project S2-19a",
        },
        "seed": _hx(seed) if seed is not None else None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [_hx(value) for value in gpr],
            "rip": _hx(CODE_ADDRESS),
            "rflags": _hx(rflags),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": _hx(CODE_ADDRESS),
                "length": 2,
                "permissions": {"read": True, "write": False, "execute": True},
                "initial_bytes_hex": "f3aa",
                "comparison_mask_hex": "ffff",
            },
            {
                "region_id": "data",
                "guest_address": _hx(DATA_ADDRESS),
                "length": DATA_LENGTH,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": data.hex(),
                "comparison_mask_hex": "ff" * DATA_LENGTH,
            },
        ],
        "stop_rip": _hx(CODE_ADDRESS + 2),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": _hx(RFLAGS_MASK),
        "expected_exit": "stop_address",
    }
    expected = _expected_result(fixture)
    touched = []
    if count:
        assert rdi_offset is not None
        touched = [rdi_offset + index if df == 0 else rdi_offset - index for index in range(count)]
    expected["case"] = {
        "count": count,
        "df": df,
        "rdi_offset": rdi_offset,
        "touched_offsets": touched,
        "vector": vector,
    }
    return fixture, expected


def generate_fixture(seed: int, sequence: int) -> tuple[dict[str, Any], dict[str, Any]]:
    if seed not in FIXED_SEEDS or not 0 <= sequence < CASES_PER_SEED:
        raise CorpusError("seed or sequence is outside the fixed S2-42i corpus")
    random = SplitMix64(seed, sequence)
    combination = sequence % (len(COUNTS) * len(DIRECTIONS))
    count = COUNTS[combination // len(DIRECTIONS)]
    df = DIRECTIONS[combination % len(DIRECTIONS)]
    variant = (sequence // (len(COUNTS) * len(DIRECTIONS))) % 8
    rdi_offset = _rdi_offset(count, df, variant, random)
    data = bytes(random.next() & 0xFF for _ in range(DATA_LENGTH))
    gpr = [random.next() for _ in range(16)]
    return _make_fixture(
        _case_id(seed, sequence), seed, sequence, count, df, rdi_offset,
        gpr, data, f"generated_{count}_{df}_{variant}",
    )


def _handwritten_fixture(count: int, df: int, vector: str) -> tuple[dict[str, Any], dict[str, Any]]:
    if vector == "first":
        rdi_offset = None if count == 0 else (0 if df == 0 else count - 1)
    elif vector == "last":
        if count == 0:
            rdi_offset = None
        else:
            rdi_offset = DATA_LENGTH - count if df == 0 else DATA_LENGTH - 1
    elif vector == "unaligned" and count == 8:
        rdi_offset = 3 if df == 0 else 10
    else:
        raise CorpusError("handwritten vector is outside the fixed boundary matrix")
    fill = ((0xA5 if df == 0 else 0x5A) ^ (count * 0x11)) & 0xFF
    data = bytes((0xC3 if index % 2 == 0 else 0x3C) for index in range(DATA_LENGTH))
    gpr = [0x1122334455667788 ^ (index * 0x0101010101010101) for index in range(16)]
    gpr[0] = (gpr[0] & ~0xFF) | fill
    vector_id = f"{vector}_count_{count}_df_{df}"
    case_id = f"s2_42i_hand_{vector_id}"
    return _make_fixture(case_id, None, None, count, df, rdi_offset, gpr, data, vector_id)


def handwritten_fixtures() -> list[tuple[dict[str, Any], dict[str, Any]]]:
    cases: list[tuple[dict[str, Any], dict[str, Any]]] = []
    for count in COUNTS:
        for df in DIRECTIONS:
            cases.append(_handwritten_fixture(count, df, "first"))
            if count:
                cases.append(_handwritten_fixture(count, df, "last"))
    for df in DIRECTIONS:
        cases.append(_handwritten_fixture(8, df, "unaligned"))
    return cases


def _expected_result(fixture: dict[str, Any]) -> dict[str, Any]:
    """Independent byte-fill oracle for one exact REP STOSB instruction."""
    initial = fixture["initial_state"]
    gpr = [int(value, 16) for value in initial["gpr"]]
    data = bytearray.fromhex(fixture["memory"][1]["initial_bytes_hex"])
    count = gpr[1]
    df = 1 if int(initial["rflags"], 16) & DF_MASK else 0
    old_rdi = gpr[7]
    if count > 8:
        raise CorpusError("independent oracle only models native-admitted counts 0..8")
    if count:
        for _ in range(count):
            offset = gpr[7] - DATA_ADDRESS
            if not 0 <= offset < DATA_LENGTH:
                raise CorpusError("independent oracle destination is outside the mapped data span")
            data[offset] = gpr[0] & 0xFF
            gpr[7] = (gpr[7] - 1 if df else gpr[7] + 1) & 0xFFFFFFFFFFFFFFFF
    gpr[1] = 0
    expected = {
        "case_id": fixture["case_id"],
        "state": {
            "mode": "long64",
            "gpr": [_hx(value) for value in gpr],
            "rip": fixture["stop_rip"],
            "rflags": initial["rflags"],
            "rflags_defined": _hx(RFLAGS_MASK),
        },
        "memory": [
            {"region_id": region["region_id"], "guest_address": region["guest_address"],
             "bytes_hex": region["initial_bytes_hex"] if region["region_id"] == "code" else data.hex()}
            for region in fixture["memory"]
        ],
        "outcome": {"kind": "stop_address"},
        "instructions_executed": {"status": "available", "value": _hx(1)},
    }
    if count == 0 and gpr[7] != old_rdi:
        raise CorpusError("zero-count oracle changed RDI despite performing no destination access")
    return expected


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    case_digests: list[dict[str, Any]] = []
    expected_cases: list[dict[str, Any]] = []

    def add_case(fixture: dict[str, Any], expected: dict[str, Any], kind: str) -> None:
        raw = json_bytes(fixture)
        files[f"{fixture['case_id']}.case.json"] = raw
        case_digests.append({
            "case_id": fixture["case_id"],
            "kind": kind,
            "sha256": sha256(raw),
            **expected["case"],
        })
        expected_cases.append(expected)

    for seed in FIXED_SEEDS:
        for sequence in range(CASES_PER_SEED):
            fixture, expected = generate_fixture(seed, sequence)
            add_case(fixture, expected, "generated")
    handwritten = handwritten_fixtures()
    if len(handwritten) != HANDWRITTEN_CASE_COUNT:
        raise CorpusError("handwritten S2-42i boundary matrix has the wrong size")
    for fixture, expected in handwritten:
        add_case(fixture, expected, "handwritten")

    expected_raw = json_bytes({"profile": PROFILE, "cases": expected_cases})
    files[EXPECTED_NAME] = expected_raw
    manifest = {
        "manifest_version": MANIFEST_VERSION,
        "generator_version": GENERATOR_VERSION,
        "profile": PROFILE,
        "instruction_hex": "f3aa",
        "instruction_length": 2,
        "address_size": 64,
        "stop_rip_offset": 2,
        "architectural_retirements": 1,
        "seeds": [f"0x{seed:016x}" for seed in FIXED_SEEDS],
        "cases_per_seed": CASES_PER_SEED,
        "counts": list(COUNTS),
        "df_values": list(DIRECTIONS),
        "generated_case_count": GENERATED_CASE_COUNT,
        "handwritten_case_count": HANDWRITTEN_CASE_COUNT,
        "case_count": len(case_digests),
        "case_digests": case_digests,
        "expected_sha256": sha256(expected_raw),
        "boundary_vectors": {
            "zero_count_unmapped_destination": True,
            "maximum_count": 8,
            "df0_first_span": 0,
            "df0_last_span": DATA_LENGTH - 8,
            "df1_first_span_rdi": 7,
            "df1_last_span_rdi": DATA_LENGTH - 1,
            "unaligned_rdi_offsets": [3, 10],
            "canary_bytes_compared": DATA_LENGTH,
        },
        "corpus_digest": sha256("\n".join(item["sha256"] for item in case_digests).encode()),
    }
    files[MANIFEST_NAME] = json_bytes(manifest)
    return files


def verify_directory(directory: Path) -> dict[str, Any]:
    expected_files = materialize()
    verify_file_set(directory, expected_files)
    manifest = json.loads((directory / MANIFEST_NAME).read_bytes(), object_pairs_hook=strict_object)
    if not isinstance(manifest, dict) or manifest.get("manifest_version") != MANIFEST_VERSION:
        raise CorpusError("manifest fields do not match the S2-42i contract")
    if manifest.get("profile") != PROFILE or manifest.get("case_count") != TOTAL_CASE_COUNT:
        raise CorpusError("manifest profile or case count differs")
    return manifest


def write_directory(directory: Path) -> None:
    write_file_set(directory, materialize())


def _verify_result(
    result: Any,
    expected: dict[str, Any],
    fixture_sha256: str,
    backend: str,
    expected_build: Any,
    expected_machine: Any,
    label: str,
) -> None:
    try:
        if not isinstance(result, dict):
            raise CorpusError(f"{label}: result must be an object")
        identity = {
            "schema_version": 2,
            "case_id": expected["case_id"],
            "input_sha256": fixture_sha256,
            "profile": PROFILE,
            "backend": backend,
        }
        if type(result.get("schema_version")) is not int or result.get("schema_version") != 2:
            raise CorpusError(f"{label}: schema_version does not match the selected fixture")
        for field, wanted in identity.items():
            if field != "schema_version" and result.get(field) != wanted:
                raise CorpusError(f"{label}: {field} does not match fixture/backend identity")
        if result.get("outcome") != expected["outcome"]:
            raise CorpusError(f"{label}: execution outcome differs")
        if result.get("instructions_executed") != expected["instructions_executed"]:
            raise CorpusError(f"{label}: expected exactly one architectural retirement")
        build = result.get("build")
        if (not isinstance(build, dict)
                or any(not build.get(field) for field in ("revision", "compiler", "target", "profile"))
                or not isinstance(expected_build, dict) or build != expected_build):
            raise CorpusError(f"{label}: build provenance is missing or differs from the run manifest")
        machine = result.get("machine")
        if (not isinstance(machine, dict)
                or any(not machine.get(field) for field in ("host_os", "host_arch"))
                or not isinstance(expected_machine, dict) or machine != expected_machine):
            raise CorpusError(f"{label}: machine provenance is missing or differs from the run manifest")
        if backend == "native_x86" and (machine.get("host_os") != "linux" or machine.get("host_arch") != "x86_64"):
            raise CorpusError(f"{label}: native result is not from Linux x86_64 hardware")
        snapshot = result.get("snapshot")
        if not isinstance(snapshot, dict) or snapshot.get("status") != "available":
            raise CorpusError(f"{label}: full snapshot is unavailable")
        actual = snapshot.get("value")
        if not isinstance(actual, dict) or not isinstance(actual.get("state"), dict):
            raise CorpusError(f"{label}: snapshot value and state must be objects")
        state = actual["state"]
        for field in ("mode", "gpr", "rip", "rflags"):
            if state.get(field) != expected["state"][field]:
                raise CorpusError(f"{label}: state field {field} differs from the independent fill oracle")
        if actual.get("rflags_defined") != expected["state"]["rflags_defined"]:
            raise CorpusError(f"{label}: flags definedness differs")
        memory = actual.get("memory")
        if not isinstance(memory, list) or len(memory) != len(expected["memory"]):
            raise CorpusError(f"{label}: complete memory snapshot is unavailable")
        for observed, wanted in zip(memory, expected["memory"], strict=True):
            if not isinstance(observed, dict) or any(
                observed.get(field) != wanted[field]
                for field in ("region_id", "guest_address", "bytes_hex")
            ):
                raise CorpusError(f"{label}: memory bytes or canaries differ")
        exception = actual.get("exception")
        if not isinstance(exception, dict) or exception.get("status") != "available" or exception.get("value") is not None:
            raise CorpusError(f"{label}: exception observation is missing or non-empty")
    except (KeyError, TypeError, AttributeError, IndexError, ValueError) as error:
        raise CorpusError(f"{label}: malformed execution result: {error}") from error


def verify_saved_run(manifest: dict[str, Any], expected_document: dict[str, Any],
                     run_manifest: dict[str, Any], run_directory: Path) -> dict[str, Any]:
    if (run_manifest.get("status") != "passed" or run_manifest.get("repeats") != 2
            or run_manifest.get("profile") != PROFILE or run_manifest.get("suite") != "integer"):
        raise CorpusError("interpreter/native oracle did not pass the selected two-repeat profile")
    cases = expected_document.get("cases")
    inputs, records = run_manifest.get("inputs"), run_manifest.get("fixtures")
    if not isinstance(cases, list) or not isinstance(inputs, list) or not isinstance(records, list):
        raise CorpusError("saved run case collections must be arrays")
    expected_by_id = {
        entry["case_id"]: entry for entry in cases
        if isinstance(entry, dict) and isinstance(entry.get("case_id"), str)
    }
    if (len(expected_by_id) != len(cases) or len(inputs) != len(cases)
            or len(cases) != manifest.get("case_count")):
        raise CorpusError("saved run omitted or duplicated expected cases")
    digest_by_id = {
        entry["case_id"]: entry["sha256"] for entry in manifest.get("case_digests", [])
        if isinstance(entry, dict) and isinstance(entry.get("case_id"), str)
    }
    if len(digest_by_id) != len(expected_by_id):
        raise CorpusError("corpus manifest case digests are incomplete")
    inputs_by_id: dict[str, dict[str, Any]] = {}
    for item in inputs:
        if not isinstance(item, dict):
            raise CorpusError("saved run input record must be an object")
        case_id = item.get("case_id")
        if not isinstance(case_id, str) or case_id in inputs_by_id or case_id not in expected_by_id:
            raise CorpusError("saved run has unexpected or duplicate input identity")
        if (type(item.get("schema_version")) is not int or item.get("schema_version") != 2
                or item.get("profile") != PROFILE or item.get("fixture_sha256") != digest_by_id.get(case_id)):
            raise CorpusError("saved run input profile/schema/digest differs from the selected corpus")
        inputs_by_id[case_id] = item
    if set(inputs_by_id) != set(expected_by_id):
        raise CorpusError("saved run input identities do not match the corpus")
    if len(records) != 2 * len(cases):
        raise CorpusError("saved run omitted repeat records")
    seen: set[tuple[str, int]] = set()
    for record in records:
        if not isinstance(record, dict):
            raise CorpusError("saved run result record must be an object")
        case_id, repeat = record.get("case_id"), record.get("repeat")
        if (not isinstance(case_id, str) or case_id not in expected_by_id
                or type(repeat) is not int or repeat not in (1, 2) or (case_id, repeat) in seen):
            raise CorpusError("saved run has unexpected or duplicate result identity")
        fixture_sha256 = digest_by_id[case_id]
        if (type(record.get("schema_version")) is not int or record.get("schema_version") != 2
                or record.get("fixture_sha256") != fixture_sha256
                or record.get("differences") != {"interpreter_vs_native": 0}
                or record.get("stable_repeat") is not (repeat > 1)):
            raise CorpusError("saved run record does not bind the selected fixture and repeat comparison")
        builds, machines = record.get("build"), record.get("machines")
        if not isinstance(builds, dict) or not isinstance(machines, dict):
            raise CorpusError("saved run record lacks backend build or machine identities")
        seen.add((case_id, repeat))
        for backend in ("interpreter", "native_x86"):
            path = run_directory / f"repeat-{repeat:02d}" / case_id / f"{backend}.json"
            try:
                result = json.loads(path.read_bytes(), object_pairs_hook=strict_object)
            except (OSError, UnicodeError, json.JSONDecodeError, CorpusError) as error:
                raise CorpusError(f"{backend} {case_id} repeat {repeat}: cannot read result: {error}") from error
            _verify_result(
                result, expected_by_id[case_id], fixture_sha256, backend,
                builds.get(backend), machines.get(backend), f"{backend} {case_id} repeat {repeat}",
            )
    if len(seen) != 2 * len(cases):
        raise CorpusError("saved run repeat matrix is incomplete")
    return {
        "status": "passed",
        "profile": PROFILE,
        "corpus_digest": manifest["corpus_digest"],
        "case_count": len(cases),
        "repeat_count": 2,
        "counts": list(COUNTS),
        "df_values": list(DIRECTIONS),
        "result_count_per_backend": 2 * len(cases),
        "independent_oracle": "matched_full_gpr_rflags_definedness_retirement_memory_canaries",
        "interpreter": "matched",
        "native_x86": "matched_linux_x86_64",
        "physical_mac_acceptance": "not_run",
        "oracle_manifest": str(run_directory / "manifest.json"),
    }


def verify_oracle_run(corpus_directory: Path, run_directory: Path) -> dict[str, Any]:
    manifest = verify_directory(corpus_directory)
    expected_document = json.loads(
        (corpus_directory / EXPECTED_NAME).read_bytes(), object_pairs_hook=strict_object
    )
    run_manifest = json.loads(
        (run_directory / "manifest.json").read_bytes(), object_pairs_hook=strict_object
    )
    if not isinstance(expected_document, dict) or not isinstance(run_manifest, dict):
        raise CorpusError("saved oracle manifests must be JSON objects")
    return verify_saved_run(manifest, expected_document, run_manifest, run_directory)


def _path_inside_work(path: Path) -> Path:
    resolved = path.resolve()
    work = (ROOT / "work").resolve()
    if resolved != work and work not in resolved.parents:
        raise CorpusError("oracle output path must resolve inside repository work/")
    cursor = path.absolute()
    while cursor != ROOT and cursor != cursor.parent:
        if cursor.exists() and cursor.is_symlink():
            raise CorpusError("oracle output paths may not contain symlinks")
        cursor = cursor.parent
    if resolved.exists() and resolved.is_symlink():
        raise CorpusError("oracle output path may not be a symlink")
    return resolved


def run_oracle(corpus_directory: Path, host: str, user: str, remote_root: str,
               output_root: Path) -> tuple[int, Path | None, str, str]:
    output_root = _path_inside_work(output_root)
    output_root.mkdir(parents=True, exist_ok=True)
    prior = {entry.resolve() for entry in output_root.iterdir() if entry.is_dir() and not entry.is_symlink()}
    fixtures = sorted(corpus_directory.glob("*.case.json"))
    try:
        cases = [str(path.resolve().relative_to(ROOT)) for path in fixtures]
        command = [
            sys.executable, str(ROOT / "tests/cpu/stage2_oracle.py"),
            "--host", host, "--user", user, "--remote-root", remote_root,
            "--repeats", "2", "--output-root", str(output_root.relative_to(ROOT)),
            *cases,
        ]
        process = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
        created = [entry for entry in output_root.iterdir()
                   if entry.is_dir() and not entry.is_symlink() and entry.resolve() not in prior]
        run_directory = created[0] if len(created) == 1 else None
        if run_directory is not None:
            (run_directory / "s2-42i-command.stdout.log").write_text(process.stdout)
            (run_directory / "s2-42i-command.stderr.log").write_text(process.stderr)
        return process.returncode, run_directory, process.stdout, process.stderr
    except (OSError, ValueError) as error:
        return 127, None, "", str(error)


def _write_summary(path: Path, summary: dict[str, Any]) -> None:
    if path.exists() and path.is_symlink():
        raise CorpusError("summary path may not be a symlink")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(json_bytes(summary))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus-dir", type=Path, default=DEFAULT_CORPUS)
    parser.add_argument("--verify", action="store_true", help="verify an existing corpus instead of generating it")
    parser.add_argument("--run-oracle", action="store_true", help="run two interpreter and Linux x86 native repeats")
    parser.add_argument("--verify-oracle-run", type=Path, help="verify saved interpreter/native run evidence without rerunning")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--oracle-output-root", type=Path, default=DEFAULT_ORACLE_ROOT)
    args = parser.parse_args(argv)

    if args.verify_oracle_run is not None and args.run_oracle:
        parser.error("--verify-oracle-run and --run-oracle are mutually exclusive")
    summary_path: Path | None = None
    try:
        corpus_directory = args.corpus_dir if args.corpus_dir.is_absolute() else ROOT / args.corpus_dir
        corpus_directory = _path_inside_work(corpus_directory)
        output_root: Path | None = None
        if args.run_oracle:
            output_root = _path_inside_work(args.oracle_output_root if args.oracle_output_root.is_absolute()
                                            else ROOT / args.oracle_output_root)
            summary_path = output_root / "s2-42i-summary.json"
            _write_summary(summary_path, {
                "status": "running", "profile": PROFILE,
                "case_count": TOTAL_CASE_COUNT, "physical_mac_acceptance": "not_run",
            })

        if args.verify_oracle_run is not None:
            manifest = verify_directory(corpus_directory)
            run_directory = args.verify_oracle_run
            if not run_directory.is_absolute():
                run_directory = ROOT / run_directory
            run_directory = _path_inside_work(run_directory)
            result = verify_oracle_run(corpus_directory, run_directory)
            print(json.dumps(result, sort_keys=True))
            return 0

        if not args.verify:
            write_directory(corpus_directory)
        manifest = verify_directory(corpus_directory)
        if not args.run_oracle:
            print(f"verified {manifest['case_count']} cases; corpus digest {manifest['corpus_digest']}")
            return 0

        assert output_root is not None and summary_path is not None
        status, run_directory, stdout, stderr = run_oracle(
            corpus_directory, args.host, args.user, args.remote_root, output_root
        )
        summary: dict[str, Any] = {
            "status": "failed",
            "profile": PROFILE,
            "corpus_digest": manifest["corpus_digest"],
            "case_count": manifest["case_count"],
            "physical_mac_acceptance": "not_run",
            "oracle_exit_status": status,
            "oracle_stdout": stdout[-4000:],
            "oracle_stderr": stderr[-4000:],
        }
        if run_directory is not None and (run_directory / "manifest.json").is_file() and status == 0:
            try:
                summary.update(verify_oracle_run(corpus_directory, run_directory))
            except (CorpusError, OSError, UnicodeError, json.JSONDecodeError, KeyError, TypeError,
                    AttributeError, IndexError, ValueError, RecursionError) as error:
                summary["error"] = str(error)
        else:
            summary["error"] = "required interpreter/native oracle run did not complete successfully"
        _write_summary(summary_path, summary)
        print(f"machine-readable summary: {summary_path}")
        if status != 0 or summary.get("status") != "passed":
            print("S2-42i oracle failed or required Linux native evidence is missing", file=sys.stderr)
            return 1
        print(f"matched {summary['result_count_per_backend']} interpreter and native results")
        return 0
    except (CorpusError, OSError, UnicodeError, json.JSONDecodeError, ValueError,
            TypeError, AttributeError, IndexError, RecursionError) as error:
        if args.run_oracle and summary_path is not None:
            try:
                _write_summary(summary_path, {
                    "status": "failed", "profile": PROFILE,
                    "case_count": TOTAL_CASE_COUNT, "physical_mac_acceptance": "not_run",
                    "error": str(error),
                })
                print(f"machine-readable summary: {summary_path}")
            except (OSError, CorpusError) as write_error:
                print(f"could not write failed machine summary: {write_error}", file=sys.stderr)
            print("S2-42i corpus preflight failed", file=sys.stderr)
            return 1
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
