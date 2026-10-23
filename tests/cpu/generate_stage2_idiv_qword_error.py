#!/usr/bin/env python3
"""Serialize one literal S2-20c qword IDIV quotient-overflow fixture."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CASE_ID = "s2_20c_idiv_qword_quotient_overflow"
RIP = 0x00100000
FLAGS = 0x0CD7
CODE = "48f7f9"  # IDIV RCX
GPRS = [
    0,
    0xFFFFFFFFFFFFFFFF,
    0x8000000000000000,
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


def materialize() -> dict[str, bytes]:
    provenance = {
        "kind": "project_fixture",
        "source": (
            "Handwritten 48 F7 F9 bytes, register values, and literal expected state; "
            "tests/cpu/generate_stage2_idiv_qword_error.py only serializes these constants. "
            "No assembler or semantic expected-value generator."
        ),
        "specification": (
            "Intel SDM revision 093, Vol.2A IDIV: RDX:RAX is the signed 128-bit dividend, "
            "RCX is the signed 64-bit divisor, and a quotient outside signed 64-bit range "
            "raises #DE. This input is signed 128-bit minimum divided by -1."
        ),
        "specification_version": "Intel SDM Vol.2A revision 093 (253667-093US); project S2-20c",
    }
    fixture = {
        "schema_version": 2,
        "case_id": CASE_ID,
        "profile": "s2-long-integer-v1",
        "provenance": provenance,
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in GPRS],
            "rip": hx(RIP),
            "rflags": hx(FLAGS),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": hx(RIP),
                "length": len(bytes.fromhex(CODE)),
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": CODE,
                "comparison_mask_hex": "ffffff",
            }
        ],
        "stop_rip": None,
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "exception",
    }
    expected = {
        "gpr": [hx(value) for value in GPRS],
        "rip": hx(RIP),
        "rflags": hx(FLAGS),
        "rflags_defined": hx(0x0CD7),
        "memory": {"code": CODE},
        "exception": {
            "vector": 0,
            "error_code": "unavailable",
            "faulting_rip": hx(RIP),
            "fault_address": "unavailable",
        },
        "instructions_executed": 0,
    }
    return {
        f"{CASE_ID}.case.json": json_bytes(fixture),
        f"{CASE_ID}_expected.json": json_bytes(expected),
    }


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
    print("S2-20c qword IDIV fixture is fresh" if args.check else f"wrote {len(generated)} S2-20c IDIV files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
