#!/usr/bin/env python3
"""Serialize handwritten S2-18k address-size-32 LODS fixtures."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
DATA = bytes.fromhex("d1c2b3a495867768899aabbccddeef10")
INITIAL_RAX = 0x1122334455667788
INITIAL_FLAGS = 0x08D7
DF = 0x0400
DEFINED = 0x0CD7
TEST_DEFINED = 0x0CC7


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


# Each row contains handwritten expected accumulator, pointer, flags, exit,
# retirement count, and read mapping. This is fixture data, not an interpreter
# oracle. Fields: id, encoding, initial RSI, expected RAX, RFLAGS, expected
# exit, retired, data base, data bytes, address width, budget, faulting RIP
# offset, expected RSI.
CASES = [
    # Canonical non-REP address-size-32 forms, both direction values and high
    # RSI halves. The address-size override truncates only the starting RSI.
    ("s2_18k_lodsb_df0", "67ac", 0x1234567800200000, 0x11223344556677D1, 0x08D7, "stop_address", 1, DATA_BASE, DATA.hex(), 1, 1, 2, 0x0000000000200001),
    ("s2_18k_lodsb_df1", "67ac", 0xFFFFFFFF00200008, 0x1122334455667789, 0x0CD7, "stop_address", 1, DATA_BASE, DATA.hex(), 1, 1, 2, 0x0000000000200007),
    ("s2_18k_lodsw_df0", "6766ad", 0xFFFFFFFF00200000, 0x112233445566C2D1, 0x08D7, "stop_address", 1, DATA_BASE, DATA.hex(), 2, 1, 3, 0x0000000000200002),
    ("s2_18k_lodsw_df1", "6766ad", 0x1234567800200008, 0x1122334455669A89, 0x0CD7, "stop_address", 1, DATA_BASE, DATA.hex(), 2, 1, 3, 0x0000000000200006),
    ("s2_18k_lodsd_df0", "67ad", 0xFFFFFFFF00200000, 0x00000000A4B3C2D1, 0x08D7, "stop_address", 1, DATA_BASE, DATA.hex(), 4, 1, 2, 0x0000000000200004),
    ("s2_18k_lodsd_df1", "67ad", 0x1234567800200008, 0x00000000BCAB9A89, 0x0CD7, "stop_address", 1, DATA_BASE, DATA.hex(), 4, 1, 2, 0x0000000000200004),
    ("s2_18k_lodsq_df0", "6748ad", 0xFFFFFFFF00200000, 0x68778695A4B3C2D1, 0x08D7, "stop_address", 1, DATA_BASE, DATA.hex(), 8, 1, 3, 0x0000000000200008),
    ("s2_18k_lodsq_df1", "6748ad", 0x1234567800200008, 0x10EFDECDBCAB9A89, 0x0CD7, "stop_address", 1, DATA_BASE, DATA.hex(), 8, 1, 3, 0x0000000000200000),
    # Unmapped reads remain harness errors and preserve every guest register.
    ("s2_18k_lodsb_unmapped", "67ac", 0xFFFFFFFF00210000, INITIAL_RAX, INITIAL_FLAGS, "harness_error", 0, DATA_BASE, DATA.hex(), 1, 4, 0, 0xFFFFFFFF00210000),
    ("s2_18k_lodsw_unmapped", "6766ad", 0xFFFFFFFF00210000, INITIAL_RAX, INITIAL_FLAGS, "harness_error", 0, DATA_BASE, DATA.hex(), 2, 4, 0, 0xFFFFFFFF00210000),
    ("s2_18k_lodsd_unmapped", "67ad", 0xFFFFFFFF00210000, INITIAL_RAX, INITIAL_FLAGS, "harness_error", 0, DATA_BASE, DATA.hex(), 4, 4, 0, 0xFFFFFFFF00210000),
    ("s2_18k_lodsq_unmapped", "6748ad", 0xFFFFFFFF00210000, INITIAL_RAX, INITIAL_FLAGS, "harness_error", 0, DATA_BASE, DATA.hex(), 8, 4, 0, 0xFFFFFFFF00210000),
    # Budget zero performs neither a read nor a register commit.
    ("s2_18k_lodsb_budget0", "67ac", 0x1234567800200000, INITIAL_RAX, INITIAL_FLAGS, "instruction_budget_exhausted", 0, DATA_BASE, DATA.hex(), 1, 0, 0, 0x1234567800200000),
    ("s2_18k_lodsw_budget0", "6766ad", 0x1234567800200000, INITIAL_RAX, INITIAL_FLAGS, "instruction_budget_exhausted", 0, DATA_BASE, DATA.hex(), 2, 0, 0, 0x1234567800200000),
    ("s2_18k_lodsd_budget0", "67ad", 0x1234567800200000, INITIAL_RAX, INITIAL_FLAGS, "instruction_budget_exhausted", 0, DATA_BASE, DATA.hex(), 4, 0, 0, 0x1234567800200000),
    ("s2_18k_lodsq_budget0", "6748ad", 0x1234567800200000, INITIAL_RAX, INITIAL_FLAGS, "instruction_budget_exhausted", 0, DATA_BASE, DATA.hex(), 8, 0, 0, 0x1234567800200000),
    # Successful reads end exactly at either 32-bit pointer wrap boundary.
    ("s2_18k_lodsb_wrap_up", "67ac", 0x7FFFFFFF_FFFFFFFF, 0x11223344556677D1, INITIAL_FLAGS, "stop_address", 1, 0xFFFFFFFF, DATA[:1].hex(), 1, 1, 2, 0x0000000000000000),
    ("s2_18k_lodsw_wrap_up", "6766ad", 0x7FFFFFFF_FFFFFFFE, 0x112233445566C2D1, INITIAL_FLAGS, "stop_address", 1, 0xFFFFFFFE, DATA[:2].hex(), 2, 1, 3, 0x0000000000000000),
    ("s2_18k_lodsd_wrap_up", "67ad", 0x7FFFFFFF_FFFFFFFC, 0x00000000A4B3C2D1, INITIAL_FLAGS, "stop_address", 1, 0xFFFFFFFC, DATA[:4].hex(), 4, 1, 2, 0x0000000000000000),
    ("s2_18k_lodsq_wrap_up", "6748ad", 0x7FFFFFFF_FFFFFFF8, 0x68778695A4B3C2D1, INITIAL_FLAGS, "stop_address", 1, 0xFFFFFFF8, DATA[:8].hex(), 8, 1, 3, 0x0000000000000000),
    ("s2_18k_lodsb_wrap_down", "67ac", 0xFFFFFFFF00000000, 0x11223344556677D1, 0x0CD7, "stop_address", 1, 0, DATA[:1].hex(), 1, 1, 2, 0x00000000FFFFFFFF),
    ("s2_18k_lodsw_wrap_down", "6766ad", 0xFFFFFFFF00000001, 0x112233445566C2D1, 0x0CD7, "stop_address", 1, 1, DATA[:2].hex(), 2, 1, 3, 0x00000000FFFFFFFF),
    ("s2_18k_lodsd_wrap_down", "67ad", 0xFFFFFFFF00000003, 0x00000000A4B3C2D1, 0x0CD7, "stop_address", 1, 3, DATA[:4].hex(), 4, 1, 2, 0x00000000FFFFFFFF),
    ("s2_18k_lodsq_wrap_down", "6748ad", 0xFFFFFFFF00000007, 0x68778695A4B3C2D1, 0x0CD7, "stop_address", 1, 7, DATA[:8].hex(), 8, 1, 3, 0x00000000FFFFFFFF),
    # The initial ESI is truncated, but the read itself remains contiguous
    # through the 4-GiB boundary and does not wrap each byte to address zero.
    ("s2_18k_lodsw_cross_4g", "6766ad", 0xA5A5A5A5FFFFFFFF, 0x112233445566C2D1, INITIAL_FLAGS, "stop_address", 1, 0xFFFFFFFF, DATA[:2].hex(), 2, 1, 3, 0x0000000000000001),
    ("s2_18k_lodsd_cross_4g", "67ad", 0xA5A5A5A5FFFFFFFF, 0x00000000A4B3C2D1, INITIAL_FLAGS, "stop_address", 1, 0xFFFFFFFF, DATA[:4].hex(), 4, 1, 2, 0x0000000000000003),
    ("s2_18k_lodsq_cross_4g", "6748ad", 0xA5A5A5A5FFFFFFFF, 0x68778695A4B3C2D1, INITIAL_FLAGS, "stop_address", 1, 0xFFFFFFFF, DATA[:8].hex(), 8, 1, 3, 0x0000000000000007),
    # Prefix normalization: later 67 is effective; a legacy 66 after REX
    # discards that REX, leaving word size.
    ("s2_18k_lodsw_normalize_66_67", "6667ad", 0xCAFEBABE00200000, 0x112233445566C2D1, INITIAL_FLAGS, "stop_address", 1, DATA_BASE, DATA.hex(), 2, 1, 3, 0x0000000000200002),
    ("s2_18k_lodsw_normalize_rex_66", "674866ad", 0xCAFEBABE00200000, 0x112233445566C2D1, INITIAL_FLAGS, "stop_address", 1, DATA_BASE, DATA.hex(), 2, 1, 4, 0x0000000000200002),
    # TEST defines the arithmetic flags except AF; LODS preserves that
    # definedness, both on success and on the following unmapped read.
    ("s2_18k_test_lodsd", "4885c067ad", 0x1234567800200000, 0x00000000A4B3C2D1, 0x0046, "stop_address", 2, DATA_BASE, DATA.hex(), 4, 2, 5, 0x0000000000200004),
    ("s2_18k_test_lodsd_unmapped", "4885c067ad", 0x1234567800210000, 0, 0x0046, "harness_error", 1, DATA_BASE, DATA.hex(), 4, 3, 3, 0x1234567800210000),
    # CLD/STD direction controls compose with all four address-size-32 LODS
    # widths; the flags mask differs only by DF.
    ("s2_18k_cld_lodsb", "fc67ac", 0xFFFFFFFF00200000, 0x11223344556677D1, 0x08D7, "stop_address", 2, DATA_BASE, DATA.hex(), 1, 2, 3, 0x0000000000200001),
    ("s2_18k_cld_lodsw", "fc6766ad", 0xFFFFFFFF00200000, 0x112233445566C2D1, 0x08D7, "stop_address", 2, DATA_BASE, DATA.hex(), 2, 2, 4, 0x0000000000200002),
    ("s2_18k_cld_lodsd", "fc67ad", 0xFFFFFFFF00200000, 0x00000000A4B3C2D1, 0x08D7, "stop_address", 2, DATA_BASE, DATA.hex(), 4, 2, 3, 0x0000000000200004),
    ("s2_18k_cld_lodsq", "fc6748ad", 0xFFFFFFFF00200000, 0x68778695A4B3C2D1, 0x08D7, "stop_address", 2, DATA_BASE, DATA.hex(), 8, 2, 4, 0x0000000000200008),
    ("s2_18k_std_lodsb", "fd67ac", 0x1234567800200008, 0x1122334455667789, 0x0CD7, "stop_address", 2, DATA_BASE, DATA.hex(), 1, 2, 3, 0x0000000000200007),
    ("s2_18k_std_lodsw", "fd6766ad", 0xFFFFFFFF00200008, 0x1122334455669A89, 0x0CD7, "stop_address", 2, DATA_BASE, DATA.hex(), 2, 2, 4, 0x0000000000200006),
    ("s2_18k_std_lodsd", "fd67ad", 0x1234567800200008, 0x00000000BCAB9A89, 0x0CD7, "stop_address", 2, DATA_BASE, DATA.hex(), 4, 2, 3, 0x0000000000200004),
    ("s2_18k_std_lodsq", "fd6748ad", 0xFFFFFFFF00200008, 0x10EFDECDBCAB9A89, 0x0CD7, "stop_address", 2, DATA_BASE, DATA.hex(), 8, 2, 4, 0x0000000000200000),
]


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    provenance = {
        "kind": "project_fixture",
        "source": "Handwritten instruction bytes, initial state, memory bytes, and expected snapshots; this script serializes constants only.",
        "specification": "Intel SDM revision 093, Vol.2A LODS; 32-bit address-size in Long64 truncates the starting RSI to ESI, reads contiguous bytes, and updates ESI modulo 2^32.",
        "specification_version": "Intel SDM revision 093 (253666-093US); project S2-18k",
    }
    for (case_id, code_hex, initial_rsi, expected_rax, expected_flags, outcome,
         retired, data_base, data_hex, size, budget, rip_offset, expected_rsi) in CASES:
        code = bytes.fromhex(code_hex)
        initial_gprs = [
            INITIAL_RAX, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
            0x0000000000200000, 0x0000000000300080, 0x0000000000300080,
            initial_rsi, 0x0000000000200000, 0x0123456789ABCDEF,
            0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
            0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
            0xFFFFFFFFFFFFFFFF,
        ]
        if case_id == "s2_18k_test_lodsd":
            initial_gprs[0] = 0
            initial_flags = 0x0802
        elif case_id == "s2_18k_test_lodsd_unmapped":
            initial_gprs[0] = 0
            initial_flags = 0x0802
        elif "cld_" in case_id or case_id.startswith("s2_18k_cld_"):
            initial_flags = 0x0CD7
        elif "std_" in case_id or case_id.startswith("s2_18k_std_"):
            initial_flags = 0x08D7
        else:
            initial_flags = 0x0CD7 if expected_flags & DF else INITIAL_FLAGS
        initial_gprs[6] = initial_rsi
        final_gprs = initial_gprs.copy()
        final_gprs[0] = expected_rax
        final_gprs[6] = expected_rsi
        if outcome != "stop_address" or retired == 0:
            final_gprs = initial_gprs.copy()
        elif case_id == "s2_18k_test_lodsd_unmapped":
            final_gprs = initial_gprs.copy()
        code_region = {
            "region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }
        raw_data = bytes.fromhex(data_hex)
        data_region = {
            "region_id": "data", "guest_address": hx(data_base), "length": len(raw_data),
            "permissions": {"read": True, "write": True, "execute": False},
            "initial_bytes_hex": data_hex, "comparison_mask_hex": "ff" * len(raw_data),
        }
        fixture = {
            "schema_version": 2,
            "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": provenance,
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(value) for value in initial_gprs], "rip": hx(CODE_BASE), "rflags": hx(initial_flags)},
            "memory": [code_region, data_region],
            "stop_rip": hx(CODE_BASE + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(DEFINED),
            "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        expected[case_id] = {
            "gpr": [hx(value) for value in final_gprs],
            "rflags": hx(expected_flags),
            "rflags_defined": hx(TEST_DEFINED if case_id.startswith("s2_18k_test_") else DEFINED),
            "rip": hx(CODE_BASE + rip_offset),
            "memory": {"code": code.hex(), "data": data_hex},
            "expected_exit": outcome,
            "retired": retired,
            "fault_address": hx(initial_rsi & 0xFFFFFFFF) if outcome == "harness_error" and not case_id.startswith("s2_18k_test_") else (hx(0x00210000) if case_id == "s2_18k_test_lodsd_unmapped" else None),
        }
    files["s2_18k_lods_addr32_expected.json"] = dump(expected)
    return files


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
    print("S2-18k address-size-32 LODS fixtures are fresh" if args.check else f"wrote {len(CASES)} S2-18k address-size-32 LODS fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
