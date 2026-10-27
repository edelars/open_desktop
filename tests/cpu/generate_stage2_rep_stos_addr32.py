#!/usr/bin/env python3
"""Serialize handwritten literal S2-19k REP STOS address-size-32 fixtures."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
BASE = [0x1122334455667788, 0, 0xDEADBEEFFEDCBA98, 0x200000, 0x300080, 0x300080, 0x200009, 0x200002,
        0x0123456789ABCDEF, 0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
        0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]

# Inputs and complete expected data are literal. This file only serializes them.
ROWS = {
    "byte_df0": ("f367aa", 2, 0x200000, 0xfeed_beef_0000_0002, 0, 0x200002, False, "10203040", "88883040", "stop_address", 1, None),
    "word_df1": ("f36766ab", 2, 0x200002, 0xfeed_beef_0000_0002, 0, 0x001ffffe, True, "1020304050607080", "8877887750607080", "stop_address", 1, None),
    "dword_df0": ("f367ab", 1, 0x200000, 0xfeed_beef_0000_0001, 0, 0x200004, False, "1020304050607080", "8877665550607080", "stop_address", 1, None),
    "qword_df1": ("f36748ab", 1, 0x200008, 0xfeed_beef_0000_0001, 0, 0x200000, True, "102030405060708090a0b0c0d0e0f000", "10203040506070808877665544332211", "stop_address", 1, None),
    "qword_wrap_up": ("f36748ab", 1, 0xfffffffc, 1, 0, 4, False, "0000000000000000", "8877665544332211", "stop_address", 1, None),
    "qword_zero": ("f36748ab", 0, 0x0000800000001000, 0xfeed_beef_0000_0000, 0xfeed_beef_0000_0000, 0x0000800000001000, False, "1020304050607080", "1020304050607080", "stop_address", 1, None),
    "byte_partial": ("f367aa", 2, 0x200000, 0xfeed_beef_0000_0002, 1, 0x200001, False, "10", "88", "harness_error", 1, 0x200001),
    "dword_zero_budget": ("f367ab", 2, 0x200000, 0xfeed_beef_0000_0002, 0xfeed_beef_0000_0002, 0x200000, False, "1020304050607080", "1020304050607080", "instruction_budget_exhausted", 0, None),
    "dword_initial_stop": ("f367ab", 2, 0x200000, 0xfeed_beef_0000_0002, 0xfeed_beef_0000_0002, 0x200000, False, "1020304050607080", "1020304050607080", "stop_address", 1, None),
    "word_normalized": ("f3674866ab", 1, 0x200000, 1, 0, 0x200002, False, "1020304050607080", "8877304050607080", "stop_address", 1, None),
    "qword_normalized": ("f3676648ab", 1, 0x200000, 1, 0, 0x200008, False, "1020304050607080", "8877665544332211", "stop_address", 1, None),
}


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for suffix, (code_hex, count, edi, rcx, expected_rcx, expected_edi, df, initial_data_hex, final_data_hex, outcome, budget, fault) in ROWS.items():
        code = bytes.fromhex(code_hex)
        initial_data = bytes.fromhex(initial_data_hex)
        final_data = bytes.fromhex(final_data_hex)
        case_id = f"s2_19k_rep_stos_addr32_{suffix}"
        gprs = BASE.copy()
        gprs[1] = rcx
        gprs[7] = edi
        flags = 0x0CD7 if df else 0x08D7
        out = gprs.copy()
        out[1] = expected_rcx
        out[7] = expected_edi
        initial_rip = CODE_BASE
        stop_rip = CODE_BASE if suffix == "dword_initial_stop" else (CODE_BASE + len(code) if outcome == "stop_address" else None)
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten instruction bytes and predetermined complete state answers; tests/cpu/generate_stage2_rep_stos_addr32.py serializes constants only. No assembler or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B STOS and REP entries: address-size 32 selects ECX and EDI, F3 repeats the selected STOS width using AL/AX/EAX/RAX, DF selects element direction, and each completed iteration decrements ECX with zero-extension into RCX. A zero ECX performs no decrement.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19k REP STOS address-size-32",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(v) for v in gprs], "rip": hx(initial_rip), "rflags": hx(flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code), "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)},
                {"region_id": "data", "guest_address": hx(0xFFFFFFFC if suffix == "qword_wrap_up" else DATA_BASE), "length": len(initial_data), "permissions": {"read": True, "write": True, "execute": False}, "initial_bytes_hex": initial_data.hex(), "comparison_mask_hex": "ff" * len(initial_data)},
            ],
            "stop_rip": hx(stop_rip) if stop_rip is not None else None, "instruction_budget": budget,
            "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answer = {"gpr": [hx(v) for v in out], "rflags": hx(flags), "rflags_defined": hx(0x0CD7),
                  "rip": hx(stop_rip if stop_rip is not None else initial_rip),
                  "memory": {"code": code.hex(), "data": final_data.hex()}}
        if fault is not None:
            answer["fault"] = {"access": "write", "address": hx(fault)}
        expected[case_id] = answer
    files["s2_19k_rep_stos_addr32_expected.json"] = dump(expected)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    mismatches = []
    generated = materialize()
    for name, data in generated.items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != data:
                mismatches.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(data)
    if mismatches:
        print("stale generated files: " + ", ".join(mismatches))
        return 1
    print("S2-19k REP STOS address-size-32 fixtures are fresh" if args.check else f"wrote {len(generated) - 1} S2-19k fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
