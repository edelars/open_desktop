#!/usr/bin/env python3
"""Serialize literal S2-22f MOV FS,AX descriptor-load cases."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
RIP = 0x00100000
GDTR_BASE = 0x00200000
CODE = "8ee0"  # MOV FS, AX
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

# (case id, selector, access, descriptor flags, GDTR limit, expected exit, note, base32)
CASES = [
    ("s2_22f_fs_load_writable_g0", 0x0B, 0xF3, 0x0, 0x000F, "stop_address",
     "accessed writable data descriptor with G=0 and base32=0x89abcdef", 0x89ABCDEF),
    ("s2_22f_fs_load_readonly_g1", 0x0B, 0xF1, 0x8, 0x000F, "stop_address",
     "accessed read-only data descriptor with G=1", 0),
    ("s2_22f_fs_load_readable_code", 0x0B, 0xFB, 0x0, 0x000F, "stop_address",
     "accessed readable non-conforming code descriptor", 0),
    ("s2_22f_fs_load_conforming_code", 0x0B, 0xFF, 0x0, 0x000F, "stop_address",
     "accessed readable conforming code descriptor", 0),
    ("s2_22f_fs_load_system_gp", 0x0B, 0x83, 0x0, 0x000F, "exception",
     "system descriptor raises #GP(selector)", 0),
    ("s2_22f_fs_load_execute_only_gp", 0x0B, 0xF9, 0x0, 0x000F, "exception",
     "execute-only code descriptor raises #GP(selector)", 0),
    ("s2_22f_fs_load_privilege_gp", 0x08, 0x93, 0x0, 0x000F, "exception",
     "CPL3 exceeds descriptor DPL0 even with RPL0; raises #GP(selector)", 0),
    ("s2_22f_fs_load_not_present_np", 0x0B, 0x73, 0x0, 0x000F, "exception",
     "otherwise valid not-present descriptor raises #NP(selector)", 0),
    ("s2_22f_fs_load_table_limit_gp", 0x0B, 0xF3, 0x0, 0x000E, "exception",
     "full descriptor crosses GDTR limit and raises #GP(selector) before read", 0),
]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def make_descriptor(access: int, flags: int, base32: int) -> bytes:
    # The eight-byte descriptor encodes base[23:0] in bytes 2–4 and
    # base[31:24] in byte 7. Keep the high-byte base fixture literal.
    return bytes([
        0, 0,
        base32 & 0xff,
        (base32 >> 8) & 0xff,
        (base32 >> 16) & 0xff,
        access,
        (flags & 0x0f) << 4,
        (base32 >> 24) & 0xff,
    ])


def make_case(case_id: str, selector: int, access: int, flags: int, gdtr_limit: int,
              expected_exit: str, note: str, base32: int) -> dict:
    gprs = list(GPRS)
    gprs[0] = (gprs[0] & ~0xFFFF) | selector
    return {
        "schema_version": 2,
        "case_id": case_id,
        "profile": "s2-long-fs-load-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten instruction bytes, GDT descriptor bytes, state, and expected exit. "
                       "This generator serializes literals only; it does not assemble, emulate, "
                       "or calculate CPU semantics."),
            "specification": ("Intel SDM revision 093 (253667-093US), Vol.2B MOV pp.4-28–4-31; "
                              "Vol.3A (325384-093US) §§3.4.3–3.4.5.1, 3.5.1, "
                              "6.4.1.1, 6.6–6.6.1, 6.11.5, 7.13."),
            "specification_version": (
                "Intel SDM revision 093; Vol.2B PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671241; Vol.3A PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671447; project S2-22f. " + note
            ),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in gprs],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
            "fs_base": hx(0xFFFF800000001000),
            "fs_selector": hx(0x0033),
            "gdtr_base": hx(GDTR_BASE),
            "gdtr_limit": hx(gdtr_limit),
            "fs_cache_limit": hx(0x0000FFFF),
            "fs_cache_access": hx(0x00F3),
            "fs_cache_flags": hx(0x0000),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": hx(RIP),
                "length": 2,
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": CODE,
                "comparison_mask_hex": "ffff",
            },
            {
                "region_id": "gdt",
                "guest_address": hx(GDTR_BASE),
                "length": 16,
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": (
                    "0000000000000000" + make_descriptor(access, flags, base32).hex()
                ),
                "comparison_mask_hex": "ff" * 16,
            },
        ],
        "stop_rip": hx(RIP + 2),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": expected_exit,
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
    print("S2-22f MOV FS descriptor-load fixtures are fresh" if args.check
          else "wrote S2-22f MOV FS descriptor-load fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
