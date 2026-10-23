#!/usr/bin/env python3
"""Serialize literal S2-22e VERW/GDT cases; expected semantics are handwritten."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
RIP = 0x00100000
GDTR_BASE = 0x00200000
CODE = "0f00e8"  # VERW AX
GPRS = [
    0xAABBCCDDEEFF000B, 0x0102030405060708,
    0x1122334455667788, 0x8877665544332211,
    0x0000000000200080, 0x0000000000200081,
    0x0000000000200004, 0x0000000000200010,
    0x0123456789ABCDEF, 0xFEDCBA9876543210,
    0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344,
    0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF,
]

# (case id, selector, access byte, GDTR limit, expected architectural description)
CASES = [
    ("s2_22e_verw_writable_dpl3", 0x0B, 0xF3, 0x000F,
     "present writable data descriptor; CPL/RPL/DPL permit access"),
    ("s2_22e_verw_readonly_dpl3", 0x0B, 0xF1, 0x000F,
     "read-only data descriptor clears ZF"),
    ("s2_22e_verw_code_dpl3", 0x0B, 0xFA, 0x000F,
     "code descriptor is not writable"),
    ("s2_22e_verw_system", 0x0B, 0x82, 0x000F,
     "system descriptor is not eligible"),
    ("s2_22e_verw_privilege", 0x0B, 0xD2, 0x000F,
     "writable data descriptor with DPL below CPL clears ZF"),
    ("s2_22e_verw_table_limit", 0x0B, 0xF2, 0x000E,
     "full 8-byte descriptor crosses the table limit, so ZF clears"),
    ("s2_22e_verw_null", 0x03, 0xF2, 0xFFFF,
     "null selector clears ZF without descriptor lookup"),
    ("s2_22e_verw_rpl0", 0x08, 0xF2, 0x000F,
     "writable data descriptor DPL3 is visible for RPL0"),
]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def make_case(case_id: str, selector: int, access: int, limit: int, note: str) -> dict:
    gprs = list(GPRS)
    gprs[0] = (gprs[0] & ~0xFFFF) | selector
    entry = bytes([0, 0, 0, 0, 0, access, 0, 0]).hex()
    gdt = "0000000000000000" + entry
    return {
        "schema_version": 2,
        "case_id": case_id,
        "profile": "s2-long-verw-gdt-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten instruction bytes, selector, descriptor bytes, state, and expected exit. "
                       "This generator serializes literals only; it does not assemble, emulate, or calculate CPU semantics."),
            "specification": ("Intel SDM revision 093 (326018-093US), Vol.2C VERR/VERW pp.5-162–5-163; "
                              "Vol.3A (325384-093US) §6.10.2 p.6-25, §6.11.5 p.6-29, and §3.5.1."),
            "specification_version": ("Intel SDM revision 093; Vol.2C PDF "
                                      "https://cdrdv2.intel.com/v1/dl/getContent/671072; Vol.3A PDF "
                                      "https://cdrdv2.intel.com/v1/dl/getContent/671447; project S2-22e. " + note),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in gprs],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
            "gdtr_base": hx(GDTR_BASE),
            "gdtr_limit": hx(limit),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": hx(RIP),
                "length": 3,
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": CODE,
                "comparison_mask_hex": "ffffff",
            },
            {
                "region_id": "gdt",
                "guest_address": hx(GDTR_BASE),
                "length": 16,
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": gdt,
                "comparison_mask_hex": "ff" * 16,
            },
        ],
        "stop_rip": hx(RIP + 3),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "stop_address",
    }


def materialize() -> dict[str, bytes]:
    return {
        f"{case_id}.case.json": (json.dumps(make_case(*case), indent=2) + "\n").encode()
        for case in CASES
        for case_id in [case[0]]
    }


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
    print("S2-22e VERW/GDT fixtures are fresh" if args.check else "wrote S2-22e VERW/GDT fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
