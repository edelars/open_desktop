#!/usr/bin/env python3
"""Serialize two literal S2-21e qword memory CMPXCHG fixture-v2 cases."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
RIP = 0x00100000
FLAGS = 0x0002


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def case(case_id: str, code: str, gprs: list[int], old: int) -> dict:
    old_bytes = old.to_bytes(8, "little").hex()
    return {
        "schema_version": 2,
        "case_id": case_id,
        "profile": "s2-long-integer-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": (
                f"Handwritten {code} bytes, register values and literal memory bytes; "
                "tests/cpu/generate_stage2_cmpxchg_qword_memory.py only serializes "
                "these constants. No assembler, emulator, or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2A CMPXCHG: REX.W + 0F B1 /r compares "
                "RAX with the old memory value; on equality it stores the source, "
                "otherwise it copies the old memory value to RAX. Arithmetic flags "
                "reflect the comparison. This local-only case uses a split-region qword."
            ),
            "specification_version": (
                "Intel SDM Vol.2A revision 093 (253666-093US), CMPXCHG; project S2-21e"
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
            },
            {
                "region_id": "data_left",
                "guest_address": hx(0x2000),
                "length": 7,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": "a5a5a5" + old_bytes[:8],
                "comparison_mask_hex": "ffffffffffffff",
            },
            {
                "region_id": "data_right",
                "guest_address": hx(0x2007),
                "length": 8,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": old_bytes[8:] + "a5a5a5a5",
                "comparison_mask_hex": "ffffffffffffffff",
            },
        ],
        "stop_rip": hx(RIP + len(bytes.fromhex(code))),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "stop_address",
    }


def materialize() -> dict[str, bytes]:
    equal_gprs = [
        0x2003,
        0xAABBCCDDEEFF0011,
        0xDEADBEEFFE123456,
        0x8877665544332211,
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
    unequal_alias_gprs = equal_gprs.copy()
    unequal_alias_gprs[0] = 0x2003

    cases = {
        "s2_21e_cmpxchg_qword_memory_equal": case(
            "s2_21e_cmpxchg_qword_memory_equal",
            "480fb118",  # CMPXCHG [RAX], RBX; RAX == [RAX]
            equal_gprs,
            0x2003,
        ),
        "s2_21e_cmpxchg_qword_memory_unequal_alias": case(
            "s2_21e_cmpxchg_qword_memory_unequal_alias",
            "480fb100",  # CMPXCHG [RAX], RAX; RAX is expected, source and base
            unequal_alias_gprs,
            0x2004,
        ),
    }
    return {f"{case_id}.case.json": json_bytes(fixture) for case_id, fixture in cases.items()}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    mismatches = []
    generated = materialize()
    if args.check:
        for name, content in generated.items():
            path = FIXTURES / name
            if not path.exists() or path.read_bytes() != content:
                mismatches.append(name)
        if mismatches:
            print("stale or missing S2-21e fixtures:")
            print("\n".join(mismatches))
            return 1
        print("S2-21e qword memory CMPXCHG fixtures are fresh")
        return 0
    FIXTURES.mkdir(parents=True, exist_ok=True)
    for name, content in generated.items():
        (FIXTURES / name).write_bytes(content)
    print(f"wrote {len(generated)} S2-21e CMPXCHG fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
