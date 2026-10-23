#!/usr/bin/env python3
"""Serialize the literal S2-22d GS-selector read fixture."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CASE_ID = "s2_22d_gs_selector_to_rax"
RIP = 0x00100000
CODE = "488ce8"  # MOV RAX, GS
GS_BASE = 0xFFFF800000001000
GS_SELECTOR = 0xF123
GPRS = [0xDEADBEEFCAFEBABE, 0x0102030405060708,
        0x1122334455667788, 0x8877665544332211,
        0x0000000000200080, 0x0000000000200080,
        0x0000000000200004, 0x0000000000200010,
        0x0123456789ABCDEF, 0xFEDCBA9876543210,
        0x8877665544332211, 0xAABBCCDDEEFF0011,
        0x1122112211221122, 0x3344334433443344,
        0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    fixture = {
        "schema_version": 2,
        "case_id": CASE_ID,
        "profile": "s2-long-gs-selector-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten 48 8C E8 bytes, initial state and expected result. "
                       "This generator serializes literals only; it does not assemble, "
                       "emulate or calculate CPU semantics."),
            "specification": ("Intel SDM revision 093 (253665-093US), Vol.1 §3.4.2 "
                              "p.3-13: segment registers contain selectors; Vol.2B "
                              "(253667-093US), MOV pp.4-28–4-31: REX.W + 8C /r "
                              "zero-extends the selected segment selector into a r64 "
                              "destination; ModR/M E8 selects GS and RAX."),
            "specification_version": ("Intel SDM revision 093; Vol.1 PDF "
                                      "https://cdrdv2.intel.com/v1/dl/getContent/671436; "
                                      "Vol.2B PDF https://cdrdv2.intel.com/v1/dl/getContent/671241; "
                                      "project S2-22d."),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in GPRS],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
            "gs_base": hx(GS_BASE),
            "gs_selector": hx(GS_SELECTOR),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": hx(RIP),
                "length": 3,
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": CODE,
                "comparison_mask_hex": "ffffff",
            }
        ],
        "stop_rip": hx(RIP + 3),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "stop_address",
    }
    return {f"{CASE_ID}.case.json": (json.dumps(fixture, indent=2) + "\n").encode()}


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
    print("S2-22d GS-selector fixture is fresh" if args.check else "wrote S2-22d fixture")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
