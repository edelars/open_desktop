#!/usr/bin/env python3
"""Serialize literal S2-20a DIV byte-form #DE fixtures."""

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
RIP = 0x00100000
FLAGS = 0x08D7
CODE = bytes.fromhex("f6f3")  # DIV BL

# case_id, AX dividend, BL divisor. Cases exercise zero divisor and quotient
# overflow; state/output constants remain handwritten and are not calculated
# by an instruction emulator or assembler.
CASES = (
    ("s2_20a_divide_error_zero", 0x0100, 0),
    ("s2_20a_divide_error_quotient_overflow", 0x0100, 1),
)


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for case_id, ax, bl in CASES:
        gprs = BASE_GPRS.copy()
        gprs[0] = (gprs[0] & ~0xFFFF) | ax
        gprs[3] = (gprs[3] & ~0xFF) | bl
        provenance = {
            "kind": "project_fixture",
            "source": (
                "Handwritten F6 F3 bytes, register values, and literal expected state; "
                "tests/cpu/generate_stage2_divide_errors.py only serializes these constants. "
                "No assembler or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2A DIV: a zero divisor or quotient outside "
                "the destination range raises #DE; the implicit dividend/destination "
                "for byte DIV is AX and divisor is r/m8."
            ),
            "specification_version": "Intel SDM Vol.2A revision 093 (253667-093US); project S2-20a",
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
                "rip": hx(RIP),
                "rflags": hx(FLAGS),
            },
            "memory": [
                {
                    "region_id": "code",
                    "guest_address": hx(RIP),
                    "length": len(CODE),
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": CODE.hex(),
                    "comparison_mask_hex": "ff" * len(CODE),
                }
            ],
            "stop_rip": None,
            "instruction_budget": 1,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": "exception",
        }
        files[f"{case_id}.case.json"] = json_bytes(fixture)
        expected[case_id] = {
            "gpr": [hx(value) for value in gprs],
            "rip": hx(RIP),
            "rflags": hx(FLAGS),
            "rflags_defined": hx(0x0CD7),
            "memory": {"code": CODE.hex()},
            "exception": {
                "vector": 0,
                "error_code": "unavailable",
                "faulting_rip": hx(RIP),
                "fault_address": "unavailable",
            },
            "instructions_executed": 0,
        }
    files["s2_20a_divide_errors_expected.json"] = json_bytes(expected)
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
    print("S2-20a divide-error fixtures are fresh" if args.check else f"wrote {len(generated)} S2-20a divide-error files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
