#!/usr/bin/env python3
"""Serialize handwritten one-shot S2-19t REPNE CMPSW/D/Q fixtures."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
H = lambda n: f"0x{n:016x}"

CASES = {
    "s2_19t_repne_cmpsw_equal": {
        "code": "f26766a7", "width": 2,
        "initial": [0xdeadbeef00001234, 1, 0, 0, 0, 0, 0xfeedface00003000, 0xcafebabe00003100, *([0] * 8)],
        "final": [0xdeadbeef00001234, 0, 0, 0, 0, 0, 0x3002, 0x3102, *([0] * 8)],
        "flags": 2, "final_flags": 0x46, "source": "3412", "destination": "3412",
    },
    "s2_19t_repne_cmpsd_mismatch": {
        "code": "67f2a7", "width": 4,
        "initial": [0, 0xaaaaaaaa00000001, 0, 0, 0, 0, 0xfeedface00003000, 0xcafebabe00003100, *([0] * 8)],
        "final": [0, 0, 0, 0, 0, 0, 0x3004, 0x3104, *([0] * 8)],
        "flags": 2, "final_flags": 0x97, "source": "78563412", "destination": "79563412",
    },
    "s2_19t_repne_cmpsq_zero_count": {
        "code": "f26748a7", "width": 8,
        "initial": [0, 0xdeadbeef00000000, 0, 0, 0, 0, 0xfeedface80000000, 0xcafebabeffffffff, *([0] * 8)],
        "final": [0, 0xdeadbeef00000000, 0, 0, 0, 0, 0xfeedface80000000, 0xcafebabeffffffff, *([0] * 8)],
        "flags": 0x8d7, "final_flags": 0x8d7, "source": None, "destination": None,
    },
}


def dump(obj: object) -> bytes:
    return (json.dumps(obj, indent=2) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers = {}
    for case_id, row in CASES.items():
        code, width = row["code"], row["width"]
        code_length = len(code) // 2
        regions = [{"region_id": "code", "guest_address": H(0x100000), "length": code_length,
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": code, "comparison_mask_hex": "ff" * code_length}]
        expected_memory = {"code": code}
        if row["source"] is not None:
            for name in ("source", "destination"):
                value = row[name]
                address = 0x3000 if name == "source" else 0x3100
                regions.append({"region_id": name, "guest_address": H(address), "length": width,
                                "permissions": {"read": True, "write": False, "execute": False},
                                "initial_bytes_hex": value, "comparison_mask_hex": "ff" * width})
                expected_memory[name] = value
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Handwritten instruction bytes and literal complete snapshots; serialization only.",
                           "specification": "Intel SDM revision 093, Vol. 2B REPNE CMPS; project S2-19t.",
                           "specification_version": "Intel SDM Vol. 2B revision 093; project S2-19t"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [H(n) for n in row["initial"]],
                              "rip": H(0x100000), "rflags": H(row["flags"])},
            "memory": regions, "stop_rip": H(0x100000 + code_length), "instruction_budget": 1,
            "wall_timeout_ms": 2000, "rflags_mask": H(0x8d5), "expected_exit": "stop_address",
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answers[case_id] = {"gpr": [H(n) for n in row["final"]], "rflags": H(row["final_flags"]),
                            "rflags_defined": H(0xcd7), "rip": H(0x100000 + code_length),
                            "memory": expected_memory}
    files["s2_19t_repne_cmps_wdq_addr32_expected.json"] = dump(answers)
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
