#!/usr/bin/env python3
"""Serialize handwritten S2-18m address-size-32 SCAS fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
INITIAL_RAX = 0x1122334455667788
INITIAL_FLAGS = 0x08D7
DF_SET_FLAGS = 0x0CD7
ALL_ARITHMETIC_DEFINED = 0x0CD7
TEST_AF_UNDEFINED = 0x0CC7
DATA = "001122338877665544332211ccddeeff"
WRAP_DATA = "88776655443322110011223344556677"
CROSS_4G_DATA = "001122334455668877665544332211cc"

BASE_GPRS = [
    INITIAL_RAX,
    0xAABBCCDDEEFF0011,
    0xDEADBEEFFEDCBA98,
    0x0000000000200000,
    0x0000000000300080,
    0x0000000000300080,
    0x0000000000200009,
    0x0000000000200003,
    0x0123456789ABCDEF,
    0xFEDCBA9876543210,
    0x8877665544332211,
    0xAABBCCDDEEFF0011,
    0x1122112211221122,
    0x3344334433443344,
    0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]

# Fields: id, bytes, initial RDI, expected RDI, initial flags, expected flags,
# expected defined flags, expected exit, retired, data base, data bytes, budget,
# expected RIP offset. All result values and memory bytes are literal inputs.
CASES = [
    # Canonical mapped equal comparisons, all four widths and both DF values.
    ("s2_18m_scasb_df0", "67ae", 0x1234567800200004, 0x0000000000200005, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 2),
    ("s2_18m_scasb_df1", "67ae", 0xFFFFFFFF00200004, 0x0000000000200003, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 2),
    ("s2_18m_scasw_df0", "6766af", 0x1234567800200004, 0x0000000000200006, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 3),
    ("s2_18m_scasw_df1", "6766af", 0xFFFFFFFF00200004, 0x0000000000200002, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 3),
    ("s2_18m_scasd_df0", "67af", 0x1234567800200004, 0x0000000000200008, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 2),
    ("s2_18m_scasd_df1", "67af", 0xFFFFFFFF00200004, 0x0000000000200000, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 2),
    ("s2_18m_scasq_df0", "6748af", 0x1234567800200004, 0x000000000020000C, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 3),
    ("s2_18m_scasq_df1", "6748af", 0xFFFFFFFF00200004, 0x00000000001FFFFC, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 3),
    # Unmapped reads retain full RDI, flags, and memory.
    ("s2_18m_scasb_unmapped", "67ae", 0xFFFFFFFF00300000, 0xFFFFFFFF00300000, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "harness_error", 0, DATA_BASE, DATA, 1, 0),
    ("s2_18m_scasw_unmapped", "6766af", 0xFFFFFFFF00300000, 0xFFFFFFFF00300000, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "harness_error", 0, DATA_BASE, DATA, 1, 0),
    ("s2_18m_scasd_unmapped", "67af", 0xFFFFFFFF00300000, 0xFFFFFFFF00300000, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "harness_error", 0, DATA_BASE, DATA, 1, 0),
    ("s2_18m_scasq_unmapped", "6748af", 0xFFFFFFFF00300000, 0xFFFFFFFF00300000, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "harness_error", 0, DATA_BASE, DATA, 1, 0),
    # A zero budget must not read or commit the candidate SCAS instruction.
    ("s2_18m_scasb_budget0", "67ae", 0x1234567800200004, 0x1234567800200004, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "instruction_budget_exhausted", 0, DATA_BASE, DATA, 0, 0),
    ("s2_18m_scasw_budget0", "6766af", 0x1234567800200004, 0x1234567800200004, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "instruction_budget_exhausted", 0, DATA_BASE, DATA, 0, 0),
    ("s2_18m_scasd_budget0", "67af", 0x1234567800200004, 0x1234567800200004, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "instruction_budget_exhausted", 0, DATA_BASE, DATA, 0, 0),
    ("s2_18m_scasq_budget0", "6748af", 0x1234567800200004, 0x1234567800200004, INITIAL_FLAGS, INITIAL_FLAGS, ALL_ARITHMETIC_DEFINED, "instruction_budget_exhausted", 0, DATA_BASE, DATA, 0, 0),
    # EDI pointer update wraps at 32 bits in each direction; reads are mapped.
    ("s2_18m_scasb_wrap_up", "67ae", 0xA5A5A5A5FFFFFFFF, 0x0000000000000000, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0xFFFFFFFF, WRAP_DATA, 1, 2),
    ("s2_18m_scasw_wrap_up", "6766af", 0xA5A5A5A5FFFFFFFE, 0x0000000000000000, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0xFFFFFFFE, WRAP_DATA, 1, 3),
    ("s2_18m_scasd_wrap_up", "67af", 0xA5A5A5A5FFFFFFFC, 0x0000000000000000, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0xFFFFFFFC, WRAP_DATA, 1, 2),
    ("s2_18m_scasq_wrap_up", "6748af", 0xA5A5A5A5FFFFFFF8, 0x0000000000000000, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0xFFFFFFF8, WRAP_DATA, 1, 3),
    ("s2_18m_scasb_wrap_down", "67ae", 0xFFFFFFFF00000000, 0x00000000FFFFFFFF, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0x00000000, WRAP_DATA, 1, 2),
    ("s2_18m_scasw_wrap_down", "6766af", 0xFFFFFFFF00000000, 0x00000000FFFFFFFE, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0x00000000, WRAP_DATA, 1, 3),
    ("s2_18m_scasd_wrap_down", "67af", 0xFFFFFFFF00000000, 0x00000000FFFFFFFC, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0x00000000, WRAP_DATA, 1, 2),
    ("s2_18m_scasq_wrap_down", "6748af", 0xFFFFFFFF00000000, 0x00000000FFFFFFF8, DF_SET_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0x00000000, WRAP_DATA, 1, 3),
    # Word/dword/qword reads beginning at EDI=ffffffff are contiguous over 4GiB.
    ("s2_18m_scasw_cross_4g", "6766af", 0xA5A5A5A5FFFFFFFF, 0x0000000000000001, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0xFFFFFFF8, CROSS_4G_DATA, 1, 3),
    ("s2_18m_scasd_cross_4g", "67af", 0xA5A5A5A5FFFFFFFF, 0x0000000000000003, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0xFFFFFFF8, CROSS_4G_DATA, 1, 2),
    ("s2_18m_scasq_cross_4g", "6748af", 0xA5A5A5A5FFFFFFFF, 0x0000000000000007, 0x08D7, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, 0xFFFFFFF8, CROSS_4G_DATA, 1, 3),
    # Existing normalization: later 66 discards earlier REX; later REX.W wins.
    ("s2_18m_scasw_normalize_rex_66", "674866af", 0x1234567800200004, 0x0000000000200006, INITIAL_FLAGS, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 4),
    ("s2_18m_scasq_normalize_66_rex", "676648af", 0x1234567800200004, 0x000000000020000C, INITIAL_FLAGS, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 1, DATA_BASE, DATA, 1, 4),
    # TEST leaves AF undefined; successful SCAS defines it, a failed read preserves it.
    ("s2_18m_test_scasd", "4885c067af", 0x1234567800200004, 0x0000000000200008, INITIAL_FLAGS, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 5),
    ("s2_18m_test_scasq_unmapped", "4885c06748af", 0xFFFFFFFF00300000, 0xFFFFFFFF00300000, DF_SET_FLAGS, 0x0416, TEST_AF_UNDEFINED, "harness_error", 1, DATA_BASE, DATA, 2, 3),
    # CLD/STD compose with each width; SCAS compares equal and preserves DF.
    ("s2_18m_cld_scasb", "fc67ae", 0xFFFFFFFF00200004, 0x0000000000200005, DF_SET_FLAGS, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 3),
    ("s2_18m_std_scasb", "fd67ae", 0x1234567800200004, 0x0000000000200003, INITIAL_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 3),
    ("s2_18m_cld_scasw", "fc6766af", 0xFFFFFFFF00200004, 0x0000000000200006, DF_SET_FLAGS, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 4),
    ("s2_18m_std_scasw", "fd6766af", 0x1234567800200004, 0x0000000000200002, INITIAL_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 4),
    ("s2_18m_cld_scasd", "fc67af", 0xFFFFFFFF00200004, 0x0000000000200008, DF_SET_FLAGS, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 3),
    ("s2_18m_std_scasd", "fd67af", 0x1234567800200004, 0x0000000000200000, INITIAL_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 3),
    ("s2_18m_cld_scasq", "fc6748af", 0xFFFFFFFF00200004, 0x000000000020000C, DF_SET_FLAGS, 0x0046, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 4),
    ("s2_18m_std_scasq", "fd6748af", 0x1234567800200004, 0x00000000001FFFFC, INITIAL_FLAGS, 0x0446, ALL_ARITHMETIC_DEFINED, "stop_address", 2, DATA_BASE, DATA, 2, 4),
]

HARNESS_ERRORS = {
    "s2_18m_scasb_unmapped": ("Unmapped", 0x00300000, "Read"),
    "s2_18m_scasw_unmapped": ("Unmapped", 0x00300000, "Read"),
    "s2_18m_scasd_unmapped": ("Unmapped", 0x00300000, "Read"),
    "s2_18m_scasq_unmapped": ("Unmapped", 0x00300000, "Read"),
    "s2_18m_test_scasq_unmapped": ("Unmapped", 0x00300000, "Read"),
}


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    assert len(CASES) == 39
    generated: dict[str, bytes] = {}
    answers: dict[str, object] = {}
    hashes: dict[str, str] = {}
    provenance = {
        "kind": "project_fixture",
        "source": "Handwritten instruction encodings, register state, expected flags/pointers, and memory bytes; generator serializes literal rows only.",
        "specification": "Intel SDM revision 093, Vol.2B SCAS; address-size override in Long64 truncates only the starting EDI, reads one contiguous operand, and updates EDI modulo 2^32.",
        "specification_version": "Intel SDM revision 093; project S2-18m bounded local profile",
    }
    for (case_id, code_hex, initial_rdi, expected_rdi, flags_in, flags_out,
         flags_defined, outcome, retired, data_base, data_hex, budget,
         rip_offset) in CASES:
        code = bytes.fromhex(code_hex)
        raw_data = bytes.fromhex(data_hex)
        before = BASE_GPRS.copy()
        before[7] = initial_rdi
        after = BASE_GPRS.copy()
        after[7] = expected_rdi
        code_region = {
            "region_id": "code",
            "guest_address": hx(CODE_BASE),
            "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(),
            "comparison_mask_hex": "ff" * len(code),
        }
        data_region = {
            "region_id": "data",
            "guest_address": hx(data_base),
            "length": len(raw_data),
            "permissions": {"read": True, "write": False, "execute": False},
            "initial_bytes_hex": data_hex,
            "comparison_mask_hex": "ff" * len(raw_data),
        }
        fixture = {
            "schema_version": 2,
            "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": provenance,
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {
                "mode": "long64",
                "gpr": [hx(value) for value in before],
                "rip": hx(CODE_BASE),
                "rflags": hx(flags_in),
            },
            "memory": [code_region, data_region],
            "stop_rip": hx(CODE_BASE + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(ALL_ARITHMETIC_DEFINED),
            "expected_exit": outcome,
        }
        filename = f"{case_id}.case.json"
        generated[filename] = dump(fixture)
        answer = {
            "expected_exit": outcome,
            "gpr": [hx(value) for value in after],
            "rflags": hx(flags_out),
            "rflags_defined": hx(flags_defined),
            "rip": hx(CODE_BASE + rip_offset),
            "memory": {"code": code.hex(), "data": data_hex},
            "retired": retired,
        }
        if case_id in HARNESS_ERRORS:
            kind, address, access = HARNESS_ERRORS[case_id]
            answer["harness_error"] = {
                "kind": kind,
                "address": hx(address),
                "access": access,
            }
        answers[case_id] = answer
        hashes[filename] = hashlib.sha256(generated[filename]).hexdigest()

    generated["s2_18m_scas_addr32_expected.json"] = dump(answers)
    generated["s2_18m_scas_addr32_manifest.json"] = dump(
        {
            "fixture_ids": sorted(answers),
            "fixture_sha256": hashes,
            "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        }
    )
    return generated


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    for name, contents in materialize().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                stale.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(contents)
    if stale:
        print("stale generated files: " + ", ".join(stale))
        return 1
    print("S2-18m SCAS fixtures are fresh" if args.check else f"wrote {len(materialize())} S2-18m fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
