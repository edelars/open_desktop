#!/usr/bin/env python3
"""Materialize literal one-instruction S2-18e bare STOSB fixtures.

Bytes, input state, and expected memory images are handwritten constants.
This script serializes them; it does not encode instructions or calculate x86
semantics. Use --check to verify the checked-in files are fresh.
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
DATA = "00112233445566778899aabbccddeeff"

# case_id, RDI offset, RSI offset, RFLAGS, final RDI offset, expected exit,
# literal final data image, injected fault address (None for mapped successes).
CASES = (
    (
        "s2_18e_stosb_df0",
        2,
        9,
        0x08D7,
        3,
        "stop_address",
        "00118833445566778899aabbccddeeff",
        None,
    ),
    (
        "s2_18e_stosb_df1",
        7,
        3,
        0x0CD7,
        6,
        "stop_address",
        "00112233445566888899aabbccddeeff",
        None,
    ),
    (
        "s2_18e_stosb_alias_canary",
        4,
        4,
        0x08D7,
        5,
        "stop_address",
        "00112233885566778899aabbccddeeff",
        None,
    ),
    (
        "s2_18e_stosb_write_fault",
        16,
        4,
        0x08D7,
        16,
        "exception",
        DATA,
        0x00200010,
    ),
)


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for case_id, rdi, rsi, flags, out_rdi, outcome, final_data, fault_address in CASES:
        gprs = BASE_GPRS.copy()
        gprs[6] = 0x00200000 + rsi
        gprs[7] = 0x00200000 + rdi
        expected_gprs = gprs.copy()
        if outcome == "stop_address":
            expected_gprs[7] = 0x00200000 + out_rdi
        provenance = {
            "kind": "project_fixture",
            "source": (
                "Handwritten bare AA bytes and literal state/memory answers; "
                "tests/cpu/generate_stage2_stosb.py serializes these constants. "
                "No assembler, emulator, or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2B STOS/STOSB/STOSW/STOSD/STOSQ: "
                "bare AA stores AL to ES:[RDI] in 64-bit mode; DF selects the "
                "RDI increment or decrement by one. The ES base is flat in "
                "64-bit mode."
            ),
            "specification_version": (
                "Intel SDM Vol.2B revision 093 (253667-093US); project S2-18e"
            ),
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
                    "initial_bytes_hex": "aa",
                    "comparison_mask_hex": "ff",
                },
                {
                    "region_id": "data",
                    "guest_address": hx(0x00200000),
                    "length": 16,
                    "permissions": {"read": True, "write": True, "execute": False},
                    "initial_bytes_hex": DATA,
                    "comparison_mask_hex": "ff" * 16,
                },
            ],
            "stop_rip": hx(0x00100001) if outcome == "stop_address" else None,
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
            "rip": hx(0x00100001 if outcome == "stop_address" else 0x00100000),
            "memory": {"code": "aa", "data": final_data},
        }
        if fault_address is not None:
            # This expected fault is a local injected backend check, not a
            # hardware restart-state claim.
            answer["fault"] = {"access": "write", "address": hx(fault_address)}
        expected[case_id] = answer
    files["s2_18e_stosb_expected.json"] = json_bytes(expected)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if generated files are stale")
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
        print("S2-18e STOSB fixtures are fresh")
    else:
        print(f"wrote {len(materialize())} S2-18e STOSB files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
