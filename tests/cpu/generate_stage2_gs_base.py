#!/usr/bin/env python3
"""Serialize the literal S2-22b GS-base qword load fixture."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CASE_ID = "s2_22b_gs_base_qword_load_alias"
RIP = 0x00100000
CODE = "65488b00"  # MOV RAX, qword ptr GS:[RAX]
OFFSET = 0x0000800000400000
GS_BASE = 0xFFFF800000000000
LINEAR = 0x0000000000400000
VALUE = 0x8877665544332211
GPRS = [OFFSET, 0xDEADBEEFCAFEBABE, 0xDEADBEEFFEDCBA98, 0x1122334455667788,
        0x0000000000200080, 0x0000000000200080, 0x0000000000200004,
        0x0000000000200010, 0x0123456789ABCDEF, 0xFEDCBA9876543210,
        0x8877665544332211, 0xAABBCCDDEEFF0011, 0x1122112211221122,
        0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    fixture = {
        "schema_version": 2,
        "case_id": CASE_ID,
        "profile": "s2-long-gs-base-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten 65 48 8B 00 bytes, initial state and expected result. "
                       "This generator serializes literals only; no assembler, emulator or "
                       "semantic expected-value generator is used."),
            "specification": ("Intel SDM revision 093 (253665-093US), Vol.1 §§3.3.7–3.3.7.1 "
                              "pp.3-10–3-11 and §3.4.2.1 p.3-15: effective offset is "
                              "computed before adding the full 64-bit GS base; GS base "
                              "participates in canonical-address checks. Vol.2B "
                              "(253667-093US), MOV pp.4-28–4-31."),
            "specification_version": ("Intel SDM revision 093; Vol.1 PDF "
                                      "https://cdrdv2.intel.com/v1/dl/getContent/671436; "
                                      "Vol.2B PDF https://cdrdv2.intel.com/v1/dl/getContent/671241; "
                                      "project S2-22b."),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64", "gpr": [hx(v) for v in GPRS],
            "rip": hx(RIP), "rflags": hx(0x0CD7), "gs_base": hx(GS_BASE),
        },
        "memory": [
            {"region_id": "code", "guest_address": hx(RIP), "length": 4,
             "permissions": {"read": False, "write": False, "execute": True},
             "initial_bytes_hex": CODE, "comparison_mask_hex": "ffffffff"},
            {"region_id": "data", "guest_address": hx(LINEAR), "length": 8,
             "permissions": {"read": True, "write": False, "execute": False},
             "initial_bytes_hex": VALUE.to_bytes(8, "little").hex(),
             "comparison_mask_hex": "ffffffffffffffff"},
        ],
        "stop_rip": hx(RIP + 4), "instruction_budget": 1,
        "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7),
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
    print("S2-22b GS-base fixture is fresh" if args.check else "wrote S2-22b fixture")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
