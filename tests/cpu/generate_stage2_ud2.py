#!/usr/bin/env python3
"""Serialize the literal S2-20b complete UD2 exception fixture."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CASE_ID = "s2_20b_ud2"
SPLIT_CASE_ID = "s2_20d_ud2_split_regions"
RIP = 0x00100000
FLAGS = 0x08D7
CODE = "0f0b"
GPRS = [
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


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    provenance = {
        "kind": "project_fixture",
        "source": (
            "Handwritten complete UD2 bytes and literal input/output constants; "
            "tests/cpu/generate_stage2_ud2.py only serializes them. No assembler, "
            "emulator, or semantic expected-value generator."
        ),
        "specification": (
            "Intel SDM revision 093, Vol.2A UD2: opcode 0F 0B raises the invalid-opcode "
            "exception. This case contains both bytes of the complete instruction; it "
            "does not claim priority over an instruction-fetch fault."
        ),
        "specification_version": "Intel SDM Vol.2A revision 093 (253667-093US); project S2-20b",
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
                "comparison_mask_hex": "ffff",
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
            "vector": 6,
            "error_code": "unavailable",
            "faulting_rip": hx(RIP),
            "fault_address": "unavailable",
        },
        "instructions_executed": 0,
    }
    split_fixture = dict(fixture)
    split_fixture["case_id"] = SPLIT_CASE_ID
    split_fixture["provenance"] = {
        **provenance,
        "source": (
            "Handwritten complete UD2 bytes split across two adjacent executable "
            "fixture regions; this checks the local FlatMemory callback boundary."
        ),
        "specification_version": "project S2-20d; local fixture memory contract",
    }
    split_fixture["memory"] = [
        {
            "region_id": "code",
            "guest_address": hx(RIP),
            "length": 1,
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": "0f",
            "comparison_mask_hex": "ff",
        },
        {
            "region_id": "tail",
            "guest_address": hx(RIP + 1),
            "length": 1,
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": "0b",
            "comparison_mask_hex": "ff",
        },
    ]
    split_expected = {
        **expected,
        "memory": {"code": "0f", "tail": "0b"},
    }
    return {
        f"{CASE_ID}.case.json": json_bytes(fixture),
        f"{CASE_ID}_expected.json": json_bytes(expected),
        f"{SPLIT_CASE_ID}.case.json": json_bytes(split_fixture),
        f"{SPLIT_CASE_ID}_expected.json": json_bytes(split_expected),
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
    print("S2-20b/S2-20d UD2 fixtures are fresh" if args.check else f"wrote {len(generated)} UD2 fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
