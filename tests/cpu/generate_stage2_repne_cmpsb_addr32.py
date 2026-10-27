#!/usr/bin/env python3
"""Serialize handwritten one-shot S2-19r REPNE CMPSB fixtures and answers."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
H = lambda n: f"0x{n:016x}"

CASES = {
    "s2_19r_cmpsb_zero_count": {
        "code": "f267a6", "gpr": [0x1234, 0xdeadbeef00000000, *([0] * 4), 0, 0xfeedface80000000, *([0] * 8)],
        "flags": 0x8d7, "final_gpr": [0x1234, 0xdeadbeef00000000, *([0] * 4), 0, 0xfeedface80000000, *([0] * 8)],
        "final_flags": 0x8d7, "data": None,
    },
    "s2_19r_cmpsb_mismatch_then_match": {
        "code": "67f2a6", "gpr": [0xdeadbeef00000000, 2, *([0] * 4), 0xfeedface00300000, 0xcafebabe00300100, *([0] * 8)],
        "flags": 2, "final_gpr": [0xdeadbeef00000000, 0, *([0] * 4), 0x00300002, 0x00300102, *([0] * 8)],
        "final_flags": 0x46, "data": ("42 41", "41 41"),
    },
}


def dump(obj: object) -> bytes:
    return (json.dumps(obj, indent=2) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers = {}
    for case_id, row in CASES.items():
        code = row["code"]
        regions = [{"region_id": "code", "guest_address": H(0x100000), "length": 3,
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": code, "comparison_mask_hex": "ffffff"}]
        expected_memory = {"code": code}
        if row["data"]:
            source, destination = row["data"]
            regions.extend([
                {"region_id": "source", "guest_address": H(0x300000), "length": 2,
                 "permissions": {"read": True, "write": False, "execute": False},
                 "initial_bytes_hex": source.replace(" ", ""), "comparison_mask_hex": "ffff"},
                {"region_id": "destination", "guest_address": H(0x300100), "length": 2,
                 "permissions": {"read": True, "write": False, "execute": False},
                 "initial_bytes_hex": destination.replace(" ", ""), "comparison_mask_hex": "ffff"},
            ])
            expected_memory.update(source=source.replace(" ", ""), destination=destination.replace(" ", ""))
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Handwritten bytes and literal complete snapshots; serialization only.",
                           "specification": "Intel SDM revision 093, Vol. 2B REPNE CMPS; project S2-19r.",
                           "specification_version": "Intel SDM Vol. 2B revision 093; project S2-19r"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [H(n) for n in row["gpr"]],
                              "rip": H(0x100000), "rflags": H(row["flags"])},
            "memory": regions, "stop_rip": H(0x100003), "instruction_budget": 1,
            "wall_timeout_ms": 2000, "rflags_mask": H(0x8d5), "expected_exit": "stop_address",
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answers[case_id] = {"gpr": [H(n) for n in row["final_gpr"]], "rflags": H(row["final_flags"]),
                            "rflags_defined": H(0xcd7), "rip": H(0x100003), "memory": expected_memory}
    files["s2_19r_repne_cmpsb_addr32_expected.json"] = dump(answers)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    failed = False
    for name, content in materialize().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != content:
                print(f"out of date: {path.relative_to(ROOT)}")
                failed = True
        else:
            path.write_bytes(content)
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
