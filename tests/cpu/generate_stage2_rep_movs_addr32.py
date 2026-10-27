#!/usr/bin/env python3
"""Serialize literal S2-19l REP MOVS address-size-32 fixtures."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
BASE = [0x1122334455667788, 0, 0xDEADBEEFFEDCBA98, 0x200000, 0x300080, 0x300080, 0x200000, 0x300000,
        0x0123456789ABCDEF, 0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
        0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]

# Each row contains literal initial and final values; this module only serializes them.
ROWS = {
    "byte_df0": ("f367a4", 2, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 0xFEED_BEEF_0000_0002, 0, 0x0020_0002, 0x0030_0002, "10203040", "10203040", "a5a5a5a5", "1020a5a5", "stop_address", 1, None, None),
    "word_df1": ("f36766a5", 2, 0xAAAA_BBBB_0020_0002, 0xCCCC_DDDD_0030_0002, 0xFEED_BEEF_0000_0002, 0, 0x001f_fffe, 0x002f_fffe, "1020304050607080", "1020304050607080", "a5a5a5a5a5a5a5a5", "10203040a5a5a5a5", "stop_address", 1, None, None),
    "dword_df0": ("f367a5", 1, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 0xFEED_BEEF_0000_0001, 0, 0x0020_0004, 0x0030_0004, "1020304050607080", "1020304050607080", "a5"*8, "10203040a5a5a5a5", "stop_address", 1, None, None),
    "qword_df1": ("f36748a5", 1, 0xAAAA_BBBB_0020_0008, 0xCCCC_DDDD_0030_0008, 0xFEED_BEEF_0000_0001, 0, 0x0020_0000, 0x0030_0000, "102030405060708090a0b0c0d0e0f000", "102030405060708090a0b0c0d0e0f000", "a5"*16, "a5"*8+"90a0b0c0d0e0f000", "stop_address", 1, None, None),
    "qword_wrap_up": ("f36748a5", 1, 0xAAAA_BBBB_FFFF_FFFC, 0xCCCC_DDDD_0030_0000, 1, 0, 4, 0x0030_0008, "0102030405060708", "0102030405060708", "a5"*8, "0102030405060708", "stop_address", 1, None, None),
    "qword_zero": ("f36748a5", 0xFEED_BEEF_0000_0000, 0x0000_8000_0000_1000, 0x0000_9000_0000_2000, 0xFEED_BEEF_0000_0000, 0xFEED_BEEF_0000_0000, 0x0000_8000_0000_1000, 0x0000_9000_0000_2000, "0102030405060708", "0102030405060708", "a5"*8, "a5"*8, "stop_address", 1, None, None),
    "qword_source_fault_after_one": ("f36748a5", 2, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 0xFEED_BEEF_0000_0002, 1, 0x0020_0008, 0x0030_0008, "0102030405060708", "0102030405060708", "a5"*16, "0102030405060708"+"a5"*8, "harness_error", 1, "read", 0x0020_0008),
    "qword_destination_fault_after_one": ("f36748a5", 2, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 0xFEED_BEEF_0000_0002, 1, 0x0020_0008, 0x0030_0008, "0102030405060708"*2, "0102030405060708"*2, "a5"*8, "0102030405060708", "harness_error", 1, "write", 0x0030_0008),
    "dword_zero_budget": ("f367a5", 2, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 0xFEED_BEEF_0000_0002, 0xFEED_BEEF_0000_0002, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, "1020304050607080", "1020304050607080", "a5"*8, "a5"*8, "instruction_budget_exhausted", 0, None, None),
    "dword_initial_stop": ("f367a5", 2, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 0xFEED_BEEF_0000_0002, 0xFEED_BEEF_0000_0002, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, "1020304050607080", "1020304050607080", "a5"*8, "a5"*8, "stop_address", 1, None, None),
    "word_normalized": ("f36667a5", 1, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 1, 0, 0x0020_0002, 0x0030_0002, "1020304050607080", "1020304050607080", "a5"*8, "1020a5a5a5a5a5a5", "stop_address", 1, None, None),
    "qword_normalized": ("f3676648a5", 1, 0xAAAA_BBBB_0020_0000, 0xCCCC_DDDD_0030_0000, 1, 0, 0x0020_0008, 0x0030_0008, "102030405060708090a0b0c0d0e0f000", "102030405060708090a0b0c0d0e0f000", "a5"*16, "1020304050607080"+"a5"*8, "stop_address", 1, None, None),
}


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for suffix, (code_hex, count, rsi, rdi, rcx, final_rcx, final_rsi, final_rdi, source_before, source_after, destination_before, destination_after, outcome, budget, fault_access, fault_address) in ROWS.items():
        code = bytes.fromhex(code_hex)
        code_id = f"s2_19l_rep_movs_addr32_{suffix}"
        registers = BASE.copy()
        registers[1], registers[6], registers[7] = rcx, rsi, rdi
        is_zero = suffix == "qword_zero"
        flags = 0x0CD7 if suffix.endswith("_df1") else 0x08D7
        after = registers.copy()
        after[1], after[6], after[7] = final_rcx, final_rsi, final_rdi
        source_address = 0xFFFFFFFC if suffix == "qword_wrap_up" else 0x200000
        source_length = len(bytes.fromhex(source_before))
        destination_address = 0x300000
        destination_length = len(bytes.fromhex(destination_before))
        stop_rip = CODE_BASE if suffix == "dword_initial_stop" else (CODE_BASE + len(code) if outcome == "stop_address" else None)
        if is_zero:
            source_address, destination_address = 0x00200000, 0x00300000
        fixture = {
            "schema_version": 2, "case_id": code_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Handwritten instruction bytes and predetermined complete expected snapshots; this generator serializes constants only. No assembler or execution engine is used.", "specification": "Intel SDM revision 093, Vol.2B MOVS and REP entries: address-size 32 selects ECX, ESI and EDI; each iteration reads the source element before writing the destination; DF selects element direction.", "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19l REP MOVSB/W/D/Q address-size-32"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(v) for v in registers], "rip": hx(CODE_BASE), "rflags": hx(flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code), "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff"*len(code)},
                {"region_id": "source", "guest_address": hx(source_address), "length": source_length, "permissions": {"read": True, "write": False, "execute": False}, "initial_bytes_hex": source_before, "comparison_mask_hex": "ff"*source_length},
                {"region_id": "destination", "guest_address": hx(destination_address), "length": destination_length, "permissions": {"read": True, "write": True, "execute": False}, "initial_bytes_hex": destination_before, "comparison_mask_hex": "ff"*destination_length},
            ],
            "stop_rip": hx(stop_rip) if stop_rip is not None else None, "instruction_budget": budget, "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        files[f"{code_id}.case.json"] = dump(fixture)
        answer = {"gpr": [hx(v) for v in after], "rflags": hx(flags), "rflags_defined": hx(0x0CD7), "rip": hx(stop_rip if stop_rip is not None else CODE_BASE), "memory": {"code": code.hex(), "source": source_after, "destination": destination_after}}
        if fault_access:
            answer["fault"] = {"access": fault_access, "address": hx(fault_address)}
        expected[code_id] = answer
    files["s2_19l_rep_movs_addr32_expected.json"] = dump(expected)
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
    print("S2-19l REP MOVS address-size-32 fixtures are fresh" if args.check else f"wrote {len(generated) - 1} S2-19l fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
