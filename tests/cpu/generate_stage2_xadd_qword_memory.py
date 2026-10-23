#!/usr/bin/env python3
"""Serialize one literal S2-21d qword memory XADD fixture-v2 case."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CASE_ID = "s2_21d_xadd_qword_memory_alias_split"
RIP = 0x00100000
CODE = "480fc100"  # XADD [RAX], RAX
RAX = 0x003000FB
FLAGS = 0x0CD7
INITIAL_GPRS = [
    RAX,
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
            "Handwritten 48 0F C1 00 bytes, initial register/memory bytes and literal "
            "expected state; tests/cpu/generate_stage2_xadd_qword_memory.py only serializes "
            "these constants. No assembler, emulator, or semantic expected-value generator."
        ),
        "specification": (
            "Intel SDM revision 093, Vol.2D XADD: the old memory destination is copied to "
            "the source register, the source is added to the destination, and arithmetic "
            "flags reflect the sum. The source register is also the effective-address base. "
            "This local-only fixture crosses two adjacent RAM regions."
        ),
        "specification_version": (
            "Intel SDM Vol.2D revision 093 (334569-093US), XADD p.6-23; project S2-21d"
        ),
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
            "gpr": [hx(value) for value in INITIAL_GPRS],
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
                "comparison_mask_hex": "ffffffff",
            },
            {
                "region_id": "data_left",
                "guest_address": hx(0x003000F8),
                "length": 8,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": "a5a5a5ffffffffff",
                "comparison_mask_hex": "ffffffffffffffff",
            },
            {
                "region_id": "data_right",
                "guest_address": hx(0x00300100),
                "length": 8,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": "ffffffa5a5a5a5a5",
                "comparison_mask_hex": "ffffffffffffffff",
            },
        ],
        "stop_rip": hx(RIP + len(bytes.fromhex(CODE))),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "stop_address",
    }
    return {f"{CASE_ID}.case.json": json_bytes(fixture)}


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
    print("S2-21d qword memory XADD fixture is fresh" if args.check else f"wrote {len(generated)} S2-21d XADD file")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
