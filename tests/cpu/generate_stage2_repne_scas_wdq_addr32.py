#!/usr/bin/env python3
"""Serialize handwritten one-shot S2-19s REPNE SCASW/D/Q fixtures."""
from __future__ import annotations
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
H = lambda n: f"0x{n:016x}"
CASES = {
    "s2_19s_repne_scasw_equal": {
        "code": "f26766af", "initial": [0xdeadbeef00001234, 1, 0, 0, 0, 0, 0, 0xfeedface00003000, *([0] * 8)],
        "final": [0xdeadbeef00001234, 0, 0, 0, 0, 0, 0, 0x3002, *([0] * 8)],
        "flags": 2, "final_flags": 0x46, "data": "3412", "data_length": 2,
    },
    "s2_19s_repne_scasd_mismatch": {
        "code": "67f2af", "initial": [0xdeadbeef12345678, 1, 0, 0, 0, 0, 0, 0xfeedface00003000, *([0] * 8)],
        "final": [0xdeadbeef12345678, 0, 0, 0, 0, 0, 0, 0x3004, *([0] * 8)],
        "flags": 2, "final_flags": 0x97, "data": "79563412", "data_length": 4,
    },
    "s2_19s_repne_scasq_zero": {
        "code": "f26748af", "initial": [0x1122334455667788, 0xaaaaaaaa00000000, 0, 0, 0, 0, 0, 0xfeedface00003000, *([0] * 8)],
        "final": [0x1122334455667788, 0xaaaaaaaa00000000, 0, 0, 0, 0, 0, 0xfeedface00003000, *([0] * 8)],
        "flags": 0x8d7, "final_flags": 0x8d7, "data": None, "data_length": 0,
    },
}

def dump(obj: object) -> bytes:
    return (json.dumps(obj, indent=2) + "\n").encode()

def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers = {}
    for case_id, row in CASES.items():
        code, data = row["code"], row["data"]
        regions = [{"region_id": "code", "guest_address": H(0x100000), "length": len(code)//2,
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": code, "comparison_mask_hex": "ff" * (len(code)//2)}]
        memory_expected = {"code": code}
        if data is not None:
            regions.append({"region_id": "data", "guest_address": H(0x3000), "length": row["data_length"],
                            "permissions": {"read": True, "write": False, "execute": False},
                            "initial_bytes_hex": data, "comparison_mask_hex": "ff" * row["data_length"]})
            memory_expected["data"] = data
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Handwritten instruction bytes and complete states; serialization only.",
                           "specification": "Intel SDM revision 093, Vol. 2B REPNE SCAS; project S2-19s.",
                           "specification_version": "Intel SDM Vol. 2B revision 093; project S2-19s"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [H(n) for n in row["initial"]],
                              "rip": H(0x100000), "rflags": H(row["flags"])},
            "memory": regions, "stop_rip": H(0x100000 + len(code)//2), "instruction_budget": 1,
            "wall_timeout_ms": 2000, "rflags_mask": H(0x8d5), "expected_exit": "stop_address",
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answers[case_id] = {"gpr": [H(n) for n in row["final"]], "rflags": H(row["final_flags"]),
                            "rflags_defined": H(0xcd7), "rip": H(0x100000 + len(code)//2),
                            "memory": memory_expected}
    files["s2_19s_repne_scas_wdq_addr32_expected.json"] = dump(answers)
    return files

def main() -> int:
    parser = argparse.ArgumentParser(); parser.add_argument("--check", action="store_true"); args = parser.parse_args()
    failed = False
    for name, content in materialize().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != content:
                print(f"out of date: {path.relative_to(ROOT)}"); failed = True
        else:
            path.write_bytes(content)
    return int(failed)

if __name__ == "__main__":
    raise SystemExit(main())
