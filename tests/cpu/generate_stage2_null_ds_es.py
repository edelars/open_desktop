#!/usr/bin/env python3
"""Serialize literal S2-22k Long64/CPL0 null DS/ES selector cases."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
RIP = 0x00100000
GPRS = [
    0xFEEDBEEF00000000, 0x0102030405060708, 0x1122334455667788, 0x8877665544332211,
    0x0000000000200080, 0x0000000000200081, 0x0000000000200004, 0x0000000000200010,
    0x0123456789ABCDEF, 0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF,
]
CASES = [
    ("s2_22k_null_ds", "8ed8", "MOV DS, AX with AX=0; high RAX is ignored", 1),
    ("s2_22k_null_es", "8ec0", "MOV ES, AX with AX=0; high RAX is ignored", 1),
    ("s2_22k_null_ds_then_es", "8ed88ec0", "sequential null DS then ES loads", 2),
    ("s2_22k_null_ds_repeated", "8ed88ed8", "repeated null DS load", 2),
]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def make_case(case_id: str, code: str, note: str, instructions: int) -> dict:
    return {
        "schema_version": 2,
        "case_id": case_id,
        "profile": "s2-long-null-ds-es-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": "Instruction bytes, initial state, and expected exit are handwritten. This generator serializes literals only; it does not assemble, emulate, or calculate CPU semantics.",
            "specification": "Intel 64 and IA-32 Architectures Software Developer's Manual, Vol. 2B MOV—Move, 253667-093US, pp. 4-28–4-31; Vol. 3A segment registers and IA-32e segmentation, 325384-093US, §§3.4.3–3.5.1.",
            "specification_version": f"Intel SDM revision 093; Vol.2B https://cdrdv2.intel.com/v1/dl/getContent/671241; Vol.3A https://cdrdv2.intel.com/v1/dl/getContent/671447; S2-22k local-only. {note}",
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in GPRS],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
            "ds_selector": hx(0x002B),
            "es_selector": hx(0x0030),
            "ds_unusable": False,
            "es_unusable": False,
        },
        "memory": [{
            "region_id": "code",
            "guest_address": hx(RIP),
            "length": len(code) // 2,
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code,
            "comparison_mask_hex": "ff" * (len(code) // 2),
        }],
        "stop_rip": hx(RIP + len(code) // 2),
        "instruction_budget": instructions,
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
    print("S2-22k null DS/ES fixtures are fresh" if args.check
          else "wrote S2-22k null DS/ES fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
