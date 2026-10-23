#!/usr/bin/env python3
"""Serialize two literal S2-21c qword register CMPXCHG fixture-v2 cases."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
RIP = 0x00100000
FLAGS = 0x0CD7
INITIAL_GPRS = [
    0x1122334455667788,
    0x1122334455667788,
    0xAABBCCDDEEFF0011,
    0x0000000000200000,
    0x0000000000300080,
    0x0000000000300080,
    0x0000000000200004,
    0x0000000000200010,
    0x0123456789ABCDEF,
    0xFEDCBA9876543210,
    0x8877665544332211,
    0xAABBCCDDEEFF0011,
    0x1122112211221122,
    0x3344334433443344,
    0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def case(case_id: str, code: str, gprs: list[int]) -> dict:
    return {
        "schema_version": 2,
        "case_id": case_id,
        "profile": "s2-long-integer-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": (
                f"Handwritten {code} bytes, register values, and literal expected state; "
                "tests/cpu/generate_stage2_cmpxchg_qword_register.py only serializes "
                "these constants. No assembler, emulator, or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2A CMPXCHG: REX.W + 0F B1 /r compares "
                "RAX with the destination; equality writes the source to the destination, "
                "otherwise the destination is copied to RAX. Arithmetic flags reflect "
                "the comparison. This local-only case is register-register only."
            ),
            "specification_version": (
                "Intel SDM Vol.2A revision 093 (253666-093US), CMPXCHG; project S2-21c"
            ),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in gprs],
            "rip": hx(RIP),
            "rflags": hx(FLAGS),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": hx(RIP),
                "length": len(bytes.fromhex(code)),
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": code,
                "comparison_mask_hex": "ffffffff",
            }
        ],
        "stop_rip": hx(RIP + len(bytes.fromhex(code))),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "stop_address",
    }


def materialize() -> dict[str, bytes]:
    equal_gprs = INITIAL_GPRS.copy()
    equal_gprs[0] = 0x1122334455667788
    equal_gprs[1] = 0x1122334455667788
    equal_gprs[3] = 0xAABBCCDDEEFF0011

    unequal_gprs = INITIAL_GPRS.copy()
    unequal_gprs[0] = 5
    unequal_gprs[1] = 7

    cases = {
        "s2_21c_cmpxchg_qword_register_equal": case(
            "s2_21c_cmpxchg_qword_register_equal",
            "480fb1d9",  # CMPXCHG RCX, RBX; RAX == RCX
            equal_gprs,
        ),
        "s2_21c_cmpxchg_qword_register_unequal_alias": case(
            "s2_21c_cmpxchg_qword_register_unequal_alias",
            "480fb1c9",  # CMPXCHG RCX, RCX; source aliases destination
            unequal_gprs,
        ),
    }
    return {f"{case_id}.case.json": json_bytes(fixture) for case_id, fixture in cases.items()}


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
    print("S2-21c qword register CMPXCHG fixtures are fresh" if args.check else f"wrote {len(generated)} S2-21c CMPXCHG files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
