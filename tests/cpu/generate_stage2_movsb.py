#!/usr/bin/env python3
"""Materialize the literal, one-instruction S2-18a MOVSB fixtures.

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
DATA = bytes.fromhex("00112233445566778899aabbccddeeff")

# case_id: (RSI offset, RDI offset, DF, initial flags, written data byte,
#           expected RSI, expected RDI, expected exit, fault access)
CASES = (
    ("s2_18a_movsb_df0", 2, 10, 0, 0x08D7, 0x22, 3, 11, "stop_address", None),
    ("s2_18a_movsb_df1", 5, 12, 1, 0x0CD7, 0x55, 4, 11, "stop_address", None),
    ("s2_18a_movsb_overlap", 6, 6, 0, 0x08D7, 0x66, 7, 7, "stop_address", None),
    ("s2_18a_movsb_read_fault", 16, 4, 0, 0x08D7, None, 16, 4, "exception", "read"),
    ("s2_18a_movsb_write_fault", 4, 16, 0, 0x08D7, None, 4, 16, "exception", "write"),
)


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for case_id, src, dst, df, flags, copied, out_src, out_dst, exit_kind, fault in CASES:
        gprs = BASE_GPRS.copy()
        gprs[6] = 0x00200000 + src
        gprs[7] = 0x00200000 + dst
        expected_gprs = gprs.copy()
        data_after = bytearray(DATA)
        if exit_kind == "stop_address":
            data_after[dst] = copied
            expected_gprs[6] = 0x00200000 + out_src
            expected_gprs[7] = 0x00200000 + out_dst
        initial_data = DATA.hex()
        final_data = bytes(data_after).hex()
        provenance = {
            "kind": "project_fixture",
            "source": (
                "Handwritten bare A4 bytes and literal state/memory answers; "
                "tests/cpu/generate_stage2_movsb.py serializes these constants. "
                "No assembler, emulator, or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2B MOVS/MOVSB/MOVSW/MOVSD/MOVSQ "
                "pp.4-107–4-109: in 64-bit mode A4 copies one byte from [RSI] "
                "to [RDI]; DF selects RSI/RDI increment or decrement by one."
            ),
            "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-18a",
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
                    "length": 1,
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": "a4",
                    "comparison_mask_hex": "ff",
                },
                {
                    "region_id": "data",
                    "guest_address": hx(0x00200000),
                    "length": len(DATA),
                    "permissions": {"read": True, "write": True, "execute": False},
                    "initial_bytes_hex": initial_data,
                    "comparison_mask_hex": "ff" * len(DATA),
                },
            ],
            "stop_rip": hx(0x00100001) if exit_kind == "stop_address" else None,
            "instruction_budget": 1,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": exit_kind,
        }
        files[f"{case_id}.case.json"] = json_bytes(fixture)
        if exit_kind == "stop_address":
            expected[case_id] = {
                "gpr": [hx(value) for value in expected_gprs],
                "rflags": hx(flags),
                "rflags_defined": hx(0x0CD7),
                "rip": hx(0x00100001),
                "memory": {"code": "a4", "data": final_data},
            }
        else:
            # The harness injects the architectural #PF at this implicit
            # address. These are local fault-policy checks, not physical
            # post-fault state expectations.
            expected[case_id] = {
                "gpr": [hx(value) for value in expected_gprs],
                "rflags": hx(flags),
                "rflags_defined": hx(0x0CD7),
                "rip": hx(0x00100000),
                "memory": {"code": "a4", "data": initial_data},
                "fault": {"access": fault, "address": hx(0x00200000 + (src if fault == "read" else dst))},
            }
    files["s2_18a_movsb_expected.json"] = json_bytes(expected)
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
        print("S2-18a MOVSB fixtures are fresh")
    else:
        print(f"wrote {len(materialize())} S2-18a MOVSB files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
