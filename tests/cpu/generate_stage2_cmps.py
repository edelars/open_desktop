#!/usr/bin/env python3
"""Serialize literal S2-18i CMPS fixtures and full-state answers."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE = 0x00100000
DATA = 0x00200000
BASE_GPRS = [0x8877665544332211, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98, DATA,
    0x300080, 0x300080, 0x200009, 0x200004, 0x0123456789ABCDEF,
    0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
DATA_BYTES = "11111111111111111111111111111111"

# id, bytes, RSI, RDI, incoming flags, exit, expected RSI, expected RDI,
# expected flags, definedness, expected code/data RAM, fault vector/error.
CASES = [
    ("s2_18i_cmpsb_df0", "a6", DATA, DATA+8, 0x8d7, "stop_address", DATA+1, DATA+9, 0x46, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmpsb_df1", "a6", DATA, DATA+8, 0xcd7, "stop_address", DATA-1, DATA+7, 0x446, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmpsw_df0", "66a7", DATA, DATA+8, 0x8d7, "stop_address", DATA+2, DATA+10, 0x46, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmpsw_df1", "66a7", DATA, DATA+8, 0xcd7, "stop_address", DATA-2, DATA+6, 0x446, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmpsd_df0", "a7", DATA, DATA+8, 0x8d7, "stop_address", DATA+4, DATA+12, 0x46, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmpsd_df1", "a7", DATA, DATA+8, 0xcd7, "stop_address", DATA-4, DATA+4, 0x446, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmpsq_df0", "48a7", DATA, DATA+8, 0x8d7, "stop_address", DATA+8, DATA+16, 0x46, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmpsq_df1", "48a7", DATA, DATA+8, 0xcd7, "stop_address", DATA-8, DATA, 0x446, 0xcd7, DATA_BYTES, None, 0),
]

for width, code in [(1, "a6"), (2, "66a7"), (4, "a7"), (8, "48a7")]:
    for site in ("first", "second"):
        rsi, rdi = (DATA, DATA+8)
        if site == "first": rsi = 0x201000
        else: rdi = 0x201000
        CASES.append((f"s2_18i_cmps_unmapped_{width}_{site}", code, rsi, rdi, 0x8d7,
            "harness_error", rsi, rdi, 0x8d7, 0xcd7, DATA_BYTES, None, 0))

for width, code in [(1, "a6"), (2, "66a7"), (4, "a7"), (8, "48a7")]:
    for operand in ("first", "second"):
        rsi, rdi = (DATA, DATA+8)
        if operand == "first": rsi = 0x0000800000000000
        else: rdi = 0x0000800000000000
        CASES.append((f"s2_18i_cmps_noncanonical_{width}_{operand}", code, rsi, rdi, 0x8d7,
            "exception", rsi, rdi, 0x8d7, 0xcd7, DATA_BYTES, 13, 0))

for width, code in [(1, "a6"), (2, "66a7"), (4, "a7"), (8, "48a7")]:
    CASES.append((f"s2_18i_cmps_budget_{width}", code, DATA, DATA+8, 0x8d7,
        "instruction_budget_exhausted", DATA, DATA+8, 0x8d7, 0xcd7, DATA_BYTES, None, 0))

CASES.extend([
    ("s2_18i_cmps_normalized_4866a7", "4866a7", DATA, DATA+8, 0x8d7, "stop_address", DATA+2, DATA+10, 0x46, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmps_normalized_6648a7", "6648a7", DATA, DATA+8, 0x8d7, "stop_address", DATA+8, DATA+16, 0x46, 0xcd7, DATA_BYTES, None, 0),
    ("s2_18i_cmps_test_success", "4885c0a6", DATA, DATA+8, 2, "stop_address", DATA+1, DATA+9, 0x46, 0xcd7, "01010000000000000100000000000000", None, 1),
    ("s2_18i_cmps_test_first_fault", "4885c0a6", 0x0000800000000000, DATA+8, 0x12, "exception", 0x0000800000000000, DATA+8, 0x12, 0xcc7, "01010000000000000100000000000000", 13, 1),
    ("s2_18i_cmps_test_second_fault", "4885c0a6", DATA, 0x0000800000000000, 0x12, "exception", DATA, 0x0000800000000000, 0x12, 0xcc7, "01010000000000000100000000000000", 13, 1),
])

assert len(CASES) == 33


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    generated: dict[str, bytes] = {}
    answers: dict[str, object] = {}
    for (case_id, code_hex, rsi, rdi, flags, outcome, after_rsi, after_rdi,
         after_flags, defined, data_hex, vector, prior_test) in CASES:
        code = bytes.fromhex(code_hex)
        before = BASE_GPRS.copy()
        before[0] = 1 if prior_test else before[0]
        before[6], before[7] = rsi, rdi
        after = before.copy()
        after[6], after[7] = after_rsi, after_rdi
        length = len(code)
        rip = CODE + length if outcome == "stop_address" else CODE + 3 if prior_test else CODE
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Handwritten CMPS bytes and literal full states/memory; tests/cpu/generate_stage2_cmps.py serializes these constants only.", "specification": "Intel SDM revision 093, Vol.2B CMPS; local bounded profile reads flat DS:[RSI] then flat ES:[RDI].", "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-18i"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(v) for v in before], "rip": hx(CODE), "rflags": hx(flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(CODE), "length": length, "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code_hex, "comparison_mask_hex": "ff" * length},
                {"region_id": "data", "guest_address": hx(DATA), "length": 16, "permissions": {"read": True, "write": True, "execute": False}, "initial_bytes_hex": data_hex, "comparison_mask_hex": "ff" * 16},
            ],
            "stop_rip": hx(CODE + length) if outcome == "stop_address" else None,
            "instruction_budget": 0 if "budget" in case_id else 2 if prior_test else 1,
            "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        generated[f"{case_id}.case.json"] = dump(fixture)
        answer = {"gpr": [hx(v) for v in after], "rflags": hx(after_flags), "rflags_defined": hx(defined), "rip": hx(rip), "memory": {"code": code_hex, "data": data_hex}}
        if vector is not None: answer["fault"] = {"vector": vector, "error_code": hx(0)}
        answers[case_id] = answer
    generated["s2_18i_cmps_expected.json"] = dump(answers)
    return generated


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    for name, contents in materialize().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents: stale.append(str(path.relative_to(ROOT)))
        else: path.write_bytes(contents)
    if stale:
        print("stale generated files: " + ", ".join(stale))
        return 1
    print("S2-18i CMPS fixtures are fresh" if args.check else "wrote 34 S2-18i fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
