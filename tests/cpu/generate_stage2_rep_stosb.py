#!/usr/bin/env python3
"""Serialize handwritten literal S2-19a REP STOSB fixtures."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE_GPRS = [
    0x1122334455667788,
    0,
    0xDEADBEEFFEDCBA98,
    0x0000000000200000,
    0x0000000000300080,
    0x0000000000300080,
    0x0000000000200009,
    0x0000000000200002,
    0x0123456789ABCDEF,
    0xFEDCBA9876543210,
    0x8877665544332211,
    0xAABBCCDDEEFF0011,
    0x1122112211221122,
    0x3344334433443344,
    0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]
DATA = "00112233445566778899aabbccddeeff"

# case_id, count, RDI offset, DF, mapped data length, expected data, expected
# RCX, expected RDI offset, outcome, injected fault address.
CASES = (
    ("s2_19a_rep_stosb_zero", 0, 2, False, 16, DATA, 0, 2, "stop_address", None),
    ("s2_19a_rep_stosb_one", 1, 2, False, 16, "00118833445566778899aabbccddeeff", 0, 3, "stop_address", None),
    ("s2_19a_rep_stosb_df0", 3, 2, False, 16, "00118888885566778899aabbccddeeff", 0, 5, "stop_address", None),
    ("s2_19a_rep_stosb_df1", 3, 7, True, 16, "00112233448888888899aabbccddeeff", 0, 4, "stop_address", None),
    ("s2_19a_rep_stosb_fault_middle", 2, 0, False, 1, "88", 1, 1, "exception", 0x00200001),
)


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for case_id, count, rdi, df, data_length, final_data, out_count, out_rdi, outcome, fault in CASES:
        gprs = BASE_GPRS.copy()
        gprs[1] = count
        gprs[7] = 0x00200000 + rdi
        flags = 0x0CD7 if df else 0x08D7
        expected_gprs = gprs.copy()
        expected_gprs[1] = out_count
        expected_gprs[7] = 0x00200000 + out_rdi
        provenance = {
            "kind": "project_fixture",
            "source": (
                "Handwritten F3 AA bytes and literal input/output constants; "
                "tests/cpu/generate_stage2_rep_stosb.py only serializes them. "
                "No assembler or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2B REP/STOS: F3 repeats STOSB "
                "according to RCX in 64-bit mode; AL is stored at ES:[RDI], "
                "DF selects the index direction, and the repeat state records "
                "remaining count and next destination after completed iterations."
            ),
            "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19a",
        }
        fixture = {
            "schema_version": 2,
            "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": provenance,
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {
                "mode": "long64",
                "gpr": [hx(value) for value in gprs],
                "rip": hx(0x00100000),
                "rflags": hx(flags),
            },
            "memory": [
                {
                    "region_id": "code",
                    "guest_address": hx(0x00100000),
                    "length": 2,
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": "f3aa",
                    "comparison_mask_hex": "ffff",
                },
                {
                    "region_id": "data",
                    "guest_address": hx(0x00200000),
                    "length": data_length,
                    "permissions": {"read": True, "write": True, "execute": False},
                    "initial_bytes_hex": DATA[: data_length * 2],
                    "comparison_mask_hex": "ff" * data_length,
                },
            ],
            "stop_rip": hx(0x00100002) if outcome == "stop_address" else None,
            "instruction_budget": 1,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = json_bytes(fixture)
        answer: dict[str, object] = {
            "gpr": [hx(value) for value in expected_gprs],
            "rflags": hx(flags),
            "rflags_defined": hx(0x0CD7),
            "rip": hx(0x00100002 if outcome == "stop_address" else 0x00100000),
            "memory": {"code": "f3aa", "data": final_data},
        }
        if fault is not None:
            answer["fault"] = {"access": "write", "address": hx(fault)}
        expected[case_id] = answer
    files["s2_19a_rep_stosb_expected.json"] = json_bytes(expected)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    mismatches = []
    generated = materialize()
    for name, contents in generated.items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                mismatches.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(contents)
    if mismatches:
        print("stale generated files: " + ", ".join(mismatches))
        return 1
    print("S2-19a REP STOSB fixtures are fresh" if args.check else f"wrote {len(generated)} S2-19a REP STOSB files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
