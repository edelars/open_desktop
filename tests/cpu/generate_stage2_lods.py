#!/usr/bin/env python3
"""Serialize handwritten S2-18f LODS fixtures and literal expected answers."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE_GPRS = [
    0x1122334455667788,
    0xAABBCCDDEEFF0011,
    0xDEADBEEFFEDCBA98,
    0x0000000000200000,
    0x0000000000300080,
    0x0000000000300080,
    0x0000000000200000,
    0x0000000000200000,
    0x0123456789ABCDEF,
    0xFEDCBA9876543210,
    0x8877665544332211,
    0xAABBCCDDEEFF0011,
    0x1122112211221122,
    0x3344334433443344,
    0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]
DATA = "d1c2b3a495867768899aabbccddeef10"
FLAGS = 0x08D7

# id, code bytes, initial RSI, expected RAX, expected RSI, outcome, budget,
# read permission. Expected values below are literal contract answers.
CASES = (
    ("s2_18f_lodsb_df0", "ac", 0x00200000, 0x11223344556677D1, 0x00200001, "stop_address", 1, True),
    ("s2_18f_lodsb_df1", "ac", 0x00200008, 0x1122334455667789, 0x00200007, "stop_address", 1, True),
    ("s2_18f_lodsw_df0", "66ad", 0x00200000, 0x112233445566C2D1, 0x00200002, "stop_address", 1, True),
    ("s2_18f_lodsw_df1", "66ad", 0x00200008, 0x1122334455669A89, 0x00200006, "stop_address", 1, True),
    ("s2_18f_lodsd_df0", "ad", 0x00200000, 0x00000000A4B3C2D1, 0x00200004, "stop_address", 1, True),
    ("s2_18f_lodsd_df1", "ad", 0x00200008, 0x00000000BCAB9A89, 0x00200004, "stop_address", 1, True),
    ("s2_18f_lodsq_df0", "48ad", 0x00200000, 0x68778695A4B3C2D1, 0x00200008, "stop_address", 1, True),
    ("s2_18f_lodsq_df1", "48ad", 0x00200008, 0x10EFDECDBCAB9A89, 0x00200000, "stop_address", 1, True),
    ("s2_18f_lodsb_unmapped", "ac", 0x00210000, 0x1122334455667788, 0x00210000, "harness_error", 1, True),
    ("s2_18f_lodsw_unmapped", "66ad", 0x00210000, 0x1122334455667788, 0x00210000, "harness_error", 1, True),
    ("s2_18f_lodsd_unmapped", "ad", 0x00210000, 0x1122334455667788, 0x00210000, "harness_error", 1, True),
    ("s2_18f_lodsq_unmapped", "48ad", 0x00210000, 0x1122334455667788, 0x00210000, "harness_error", 1, True),
    ("s2_18f_lodsb_noncanonical", "ac", 0x0000800000000000, 0x1122334455667788, 0x0000800000000000, "exception", 1, True),
    ("s2_18f_lodsw_noncanonical", "66ad", 0x0000800000000000, 0x1122334455667788, 0x0000800000000000, "exception", 1, True),
    ("s2_18f_lodsd_noncanonical", "ad", 0x0000800000000000, 0x1122334455667788, 0x0000800000000000, "exception", 1, True),
    ("s2_18f_lodsq_noncanonical", "48ad", 0x0000800000000000, 0x1122334455667788, 0x0000800000000000, "exception", 1, True),
    ("s2_18f_lodsb_budget0", "ac", 0x00200000, 0x1122334455667788, 0x00200000, "instruction_budget_exhausted", 0, True),
    ("s2_18f_lodsw_budget0", "66ad", 0x00200000, 0x1122334455667788, 0x00200000, "instruction_budget_exhausted", 0, True),
    ("s2_18f_lodsd_budget0", "ad", 0x00200000, 0x1122334455667788, 0x00200000, "instruction_budget_exhausted", 0, True),
    ("s2_18f_lodsq_budget0", "48ad", 0x00200000, 0x1122334455667788, 0x00200000, "instruction_budget_exhausted", 0, True),
    ("s2_18f_lodsw_ignored_rex", "4866ad", 0x00200000, 0x112233445566C2D1, 0x00200002, "stop_address", 1, True),
    ("s2_18f_lodsq_rexw_after_66", "6648ad", 0x00200000, 0x68778695A4B3C2D1, 0x00200008, "stop_address", 1, True),
)


def hx(value: int) -> str:
    return f"0x{value:016x}"


def encoded_hex(code: str) -> bytes:
    return bytes.fromhex(code)


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    provenance = {
        "kind": "project_fixture",
        "source": "Handwritten instruction bytes, state, and expected snapshots; this script only serializes constants.",
        "specification": "Intel SDM revision 093, Vol.2B LODS/LODSB/LODSW/LODSD/LODSQ; long-mode flat DS:[RSI], DF-controlled RSI, accumulator-width aliasing.",
        "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-18f",
    }
    for case_id, code_hex, initial_rsi, expected_rax, expected_rsi, outcome, budget, readable in CASES:
        code = encoded_hex(code_hex)
        initial_gprs = BASE_GPRS.copy()
        initial_gprs[6] = initial_rsi
        final_gprs = initial_gprs.copy()
        final_gprs[0] = expected_rax
        final_gprs[6] = expected_rsi
        initial_flags = FLAGS | (0x400 if "df1" in case_id else 0)
        if outcome in ("exception", "harness_error", "instruction_budget_exhausted"):
            final_gprs = initial_gprs.copy()
            expected_rflags = initial_flags
            expected_rip = 0x00100000
        else:
            expected_rflags = initial_flags
            expected_rip = 0x00100000 + len(code)
        fixture = {
            "schema_version": 2,
            "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": provenance,
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in initial_gprs], "rip": hx(0x00100000), "rflags": hx(initial_flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(0x00100000), "length": len(code), "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)},
                {"region_id": "data", "guest_address": hx(0x00200000), "length": 16, "permissions": {"read": readable, "write": True, "execute": False}, "initial_bytes_hex": DATA, "comparison_mask_hex": "ff" * 16},
            ],
            "stop_rip": hx(0x00100000 + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = json_bytes(fixture)
        answer: dict[str, object] = {
            "gpr": [hx(x) for x in final_gprs],
            "rflags": hx(expected_rflags),
            "rflags_defined": hx(0x0CD7),
            "rip": hx(expected_rip),
            "memory": {"code": code.hex(), "data": DATA},
        }
        if "noncanonical" in case_id:
            answer["fault"] = {"vector": 13, "error_code": 0}
        expected[case_id] = answer
    files["s2_18f_lods_expected.json"] = json_bytes(expected)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    mismatches = []
    for name, contents in materialize().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                mismatches.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(contents)
    if mismatches:
        print("stale generated files: " + ", ".join(mismatches))
        return 1
    print("S2-18f LODS fixtures are fresh" if args.check else f"wrote {len(CASES)} S2-18f LODS fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
