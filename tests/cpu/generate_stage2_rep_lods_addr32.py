#!/usr/bin/env python3
"""Serialize handwritten literal S2-19j REP LODS address-size-32 fixtures."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE = [0x1122334455667788, 0, 0xDEADBEEFFEDCBA98, 0x200000, 0x300080, 0x300080, 0x200009, 0x200002,
        0x0123456789ABCDEF, 0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
        0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
DATA = bytes.fromhex("0102030405060708090a0b0c0d0e0f101112131415161718")
ROWS = {
    "byte_df0": (bytes.fromhex("f367ac"), 1, 0x200000, False, DATA, 0x1122334455667701, 0x200001, 0),
    "word_df1": (bytes.fromhex("f36766ad"), 2, 0x200002, True, DATA, 0x1122334455660201, 0x1ffffe, 0),
    "dword_df0": (bytes.fromhex("f367ad"), 2, 0x200000, False, DATA, 0x0000000008070605, 0x200008, 0),
    "qword_df1": (bytes.fromhex("f36748ad"), 2, 0x200008, True, DATA, 0x0807060504030201, 0x1ffff8, 0),
    "qword_wrap_up": (bytes.fromhex("f36748ad"), 1, 0x12345678fffffffc, False, DATA[:8], 0x0807060504030201, 0x4, 0),
    "qword_zero": (bytes.fromhex("f36748ad"), 0, 0x0000800000000000, False, DATA, BASE[0], 0x0000800000000000, 0),
    "qword_partial": (bytes.fromhex("f36748ad"), 2, 0x200000, False, DATA[:8], 0x0807060504030201, 0x200008, 1),
}


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    for suffix, (code, count, rsi, df, data, rax, out_rsi, remaining) in ROWS.items():
        data_base = 0xFFFFFFFC if suffix == "qword_wrap_up" else 0x00200000
        case_id = f"s2_19j_rep_lods_addr32_{suffix}"
        gprs = BASE.copy()
        gprs[1] = count
        gprs[6] = rsi
        flags = 0x0CD7 if df else 0x08D7
        out = gprs.copy()
        out[0] = rax
        out[1] = remaining
        out[6] = out_rsi
        outcome = "harness_error" if suffix == "qword_partial" else "stop_address"
        stop_rip = 0x00100000 + len(code) if outcome == "stop_address" else None
        fixture = {
            "schema_version": 2,
            "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten instruction bytes and predetermined complete state answers; tests/cpu/generate_stage2_rep_lods_addr32.py serializes constants only. No assembler or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B LODS and REP entries: address-size 32 selects ESI, F3 repeats the selected LODS width from DS:[ESI], DF selects element direction, and completed iterations update RCX and ESI.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19j REP LODS address-size-32",
            },
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(value) for value in gprs], "rip": hx(0x00100000), "rflags": hx(flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(0x00100000), "length": len(code), "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)},
                {"region_id": "data", "guest_address": hx(data_base), "length": len(data), "permissions": {"read": True, "write": False, "execute": False}, "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data)},
            ],
            "stop_rip": hx(stop_rip) if stop_rip is not None else None,
            "instruction_budget": 1,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answer = {"gpr": [hx(value) for value in out], "rflags": hx(flags), "rflags_defined": hx(0x0CD7), "rip": hx(stop_rip if stop_rip is not None else 0x00100000), "memory": {"code": code.hex(), "data": data.hex()}}
        if suffix == "qword_partial":
            answer["fault"] = {"access": "read", "address": hx(0x00200008)}
        expected[case_id] = answer
    files["s2_19j_rep_lods_addr32_expected.json"] = dump(expected)
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
    print("S2-19j REP LODS address-size-32 fixtures are fresh" if args.check else f"wrote {len(generated) - 1} S2-19j fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
