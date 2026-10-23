#!/usr/bin/env python3
"""Serialize one literal S2-21g MFENCE store/fence/load fixture."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CASE_ID = "s2_21g_mfence_store_fence_load"
RIP = 0x00100000
CODE = "4889180faef0488b08"  # MOV [RAX],RBX; MFENCE; MOV RCX,[RAX]
INITIAL_GPRS = [
    0x00300000,
    0xDEADBEEFCAFEBABE,
    0xDEADBEEFFEDCBA98,
    0x1122334455667788,
    0x0000000000200080,
    0x0000000000200080,
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
    fixture = {
        "schema_version": 2,
        "case_id": CASE_ID,
        "profile": "s2-long-integer-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": (
                "Handwritten 48 89 18 0F AE F0 48 8B 08 bytes, initial state and literal expected state; "
                "tests/cpu/generate_stage2_mfence.py only serializes these constants. No assembler, "
                "emulator or semantic expected-value generator."
            ),
            "specification": (
                "Intel SDM revision 093 (253667-093US), Vol.2B MFENCE p.4-15: 0F AE F0 orders preceding loads/stores "
                "before following loads/stores and does not serialize the instruction stream. The fixture "
                "checks only explicit completion on private synchronous FlatMemory; it makes no shared-memory, "
                "SMP or physical ordering claim. Other 0F AE forms are outside this project profile, not "
                "claimed architecturally invalid."
            ),
            "specification_version": (
                "Intel SDM revision 093 (253667-093US), Vol.2B MFENCE p.4-15; official Vol.2B PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671241; official Intel SDM page "
                "https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html; project S2-21g"
            ),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in INITIAL_GPRS],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": hx(RIP),
                "length": len(bytes.fromhex(CODE)),
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": CODE,
                "comparison_mask_hex": "ffffffffffffffffff",
            },
            {
                "region_id": "data",
                "guest_address": hx(0x00300000),
                "length": 8,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": "a1a2a3a4a5a6a7a8",
                "comparison_mask_hex": "ffffffffffffffff",
            },
        ],
        "stop_rip": hx(RIP + len(bytes.fromhex(CODE))),
        "instruction_budget": 3,
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
    print("S2-21g MFENCE fixture is fresh" if args.check else f"wrote {len(generated)} S2-21g MFENCE file")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
