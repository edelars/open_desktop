#!/usr/bin/env python3
"""Materialize literal, one-instruction S2-18d MOVSQ fixtures.

Instruction bytes and expected outputs are handwritten constants below. This
script serializes fixtures and answers; it does not encode instructions or
derive x86 results. Use --check to verify checked-in files are fresh.
"""

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
    0,
    0,
    0x0123456789ABCDEF,
    0xFEDCBA9876543210,
    0x8877665544332211,
    0xAABBCCDDEEFF0011,
    0x1122112211221122,
    0x3344334433443344,
    0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]
DATA = bytes.fromhex("00112233445566778899aabbccddeeff102132435465768798a9bacbdcedfe0f")

# case_id, code bytes, source offset, destination offset, DF, initial flags,
# expected RSI offset, expected RDI offset, outcome, fault access, literal
# final data bytes. Expected qword and memory answers are authored above.
CASES = (
    (
        "s2_18d_movsq_df0", "48a5", 4, 12, 0, 0x08D7, 12, 20,
        "stop_address", None,
        "00112233445566778899aabb445566778899aabb5465768798a9bacbdcedfe0f",
    ),
    (
        "s2_18d_movsq_df1", "48a5", 20, 12, 1, 0x0CD7, 12, 4,
        "stop_address", None,
        "00112233445566778899aabb5465768798a9bacb5465768798a9bacbdcedfe0f",
    ),
    (
        "s2_18d_movsq_alias", "48a5", 4, 4, 0, 0x08D7, 12, 12,
        "stop_address", None,
        "00112233445566778899aabbccddeeff102132435465768798a9bacbdcedfe0f",
    ),
    (
        "s2_18d_movsq_overlap", "48a5", 4, 8, 0, 0x08D7, 12, 16,
        "stop_address", None,
        "0011223344556677445566778899aabb102132435465768798a9bacbdcedfe0f",
    ),
    (
        "s2_18d_movsq_rexw_overrides_66", "6648a5", 0, 16, 0, 0x08D7, 8, 24,
        "stop_address", None,
        "00112233445566778899aabbccddeeff001122334455667798a9bacbdcedfe0f",
    ),
    (
        "s2_18d_movsq_read_fault", "48a5", 32, 4, 0, 0x08D7, 32, 4,
        "exception", "read",
        "00112233445566778899aabbccddeeff102132435465768798a9bacbdcedfe0f",
    ),
    (
        "s2_18d_movsq_write_fault", "48a5", 4, 32, 0, 0x08D7, 4, 32,
        "exception", "write",
        "00112233445566778899aabbccddeeff102132435465768798a9bacbdcedfe0f",
    ),
)


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for case_id, code, src, dst, df, flags, out_src, out_dst, outcome, fault, final_data in CASES:
        gprs = BASE_GPRS.copy()
        gprs[6] = 0x00200000 + src
        gprs[7] = 0x00200000 + dst
        expected_gprs = gprs.copy()
        if outcome == "stop_address":
            expected_gprs[6] = 0x00200000 + out_src
            expected_gprs[7] = 0x00200000 + out_dst
        provenance = {
            "kind": "project_fixture",
            "source": (
                "Handwritten REX.W+A5 bytes and literal state/memory answers; "
                "tests/cpu/generate_stage2_movsq.py serializes these constants. "
                "No assembler, emulator, or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2B MOVS/MOVSB/MOVSW/MOVSD/MOVSQ: "
                "REX.W+A5 moves one qword from [RSI] to [RDI] in 64-bit mode; "
                "DF selects index increment or decrement by eight. Vol.2A §2.2.1.2 "
                "states effective REX.W overrides 66 for non-byte operations."
            ),
            "specification_version": "Intel SDM Vol.2A/2B revision 093 (253666-093US/253667-093US); project S2-18d",
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
                    "length": len(bytes.fromhex(code)),
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": code,
                    "comparison_mask_hex": "ff" * len(bytes.fromhex(code)),
                },
                {
                    "region_id": "data",
                    "guest_address": hx(0x00200000),
                    "length": len(DATA),
                    "permissions": {"read": True, "write": True, "execute": False},
                    "initial_bytes_hex": DATA.hex(),
                    "comparison_mask_hex": "ff" * len(DATA),
                },
            ],
            "stop_rip": hx(0x00100000 + len(bytes.fromhex(code))) if outcome == "stop_address" else None,
            "instruction_budget": 1,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = json_bytes(fixture)
        result = {
            "gpr": [hx(value) for value in expected_gprs],
            "rflags": hx(flags),
            "rflags_defined": hx(0x0CD7),
            "rip": hx(0x00100000 + len(bytes.fromhex(code))) if outcome == "stop_address" else hx(0x00100000),
            "memory": {"code": code, "data": final_data},
        }
        if fault:
            result["fault"] = {
                "access": fault,
                "address": hx(0x00200000 + (src if fault == "read" else dst)),
            }
        expected[case_id] = result
    files["s2_18d_movsq_expected.json"] = json_bytes(expected)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if generated fixture files are stale")
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
    if args.check:
        print("S2-18d MOVSQ fixtures are fresh")
    else:
        print(f"wrote {len(materialize())} S2-18d MOVSQ files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
