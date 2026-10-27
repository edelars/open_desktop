#!/usr/bin/env python3
"""Serialize literal S2-18j CLD/STD fixtures and complete expected snapshots."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
INITIAL_DATA = "0011223341414141414141aabbccddee"
GPRS = [
    0x8877665544332211, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
    DATA_BASE, 0x0000000000300080, 0x0000000000300080,
    DATA_BASE + 4, DATA_BASE + 8, 0x0123456789ABCDEF,
    0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]
DF = 0x400
DEFINED = 0x0CD7
TEST_DEFINED = 0x0CC7


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


# Every expected register override, flag value and memory image below is a
# handwritten architectural result. This table is the fixture oracle.
CASES = [
    # Bare opcodes, both incoming DF values.
    {"id": "s2_18j_cld_df0", "code": "fc", "input_flags": 0x08D7, "flags": 0x08D7, "retired": 1},
    {"id": "s2_18j_cld_df1", "code": "fc", "input_flags": 0x0CD7, "flags": 0x08D7, "retired": 1},
    {"id": "s2_18j_std_df0", "code": "fd", "input_flags": 0x08D7, "flags": 0x0CD7, "retired": 1},
    {"id": "s2_18j_std_df1", "code": "fd", "input_flags": 0x0CD7, "flags": 0x0CD7, "retired": 1},
    # Zero budget commits neither CLD nor STD.
    {"id": "s2_18j_cld_budget_zero", "code": "fc", "input_flags": 0x0CD7, "flags": 0x0CD7, "retired": 0, "budget": 0, "exit": "instruction_budget_exhausted"},
    {"id": "s2_18j_std_budget_zero", "code": "fd", "input_flags": 0x08D7, "flags": 0x08D7, "retired": 0, "budget": 0, "exit": "instruction_budget_exhausted"},
    # TEST changes arithmetic flags; the following direction instruction only
    # changes DF and preserves TEST's definedness (AF remains undefined).
    {"id": "s2_18j_test_cld", "code": "4885c0fc", "input_flags": 0x0C02, "input_rax": 0, "flags": 0x046, "defined": TEST_DEFINED, "retired": 2},
    {"id": "s2_18j_test_std", "code": "4885c0fd", "input_flags": 0x0802, "input_rax": 0, "flags": 0x446, "defined": TEST_DEFINED, "retired": 2},
    # Non-REP byte string consumers after each direction instruction.
    {"id": "s2_18j_cld_movsb", "code": "fca4", "input_flags": 0x0CD7, "flags": 0x08D7, "retired": 2, "gpr_after": {6: DATA_BASE + 5, 7: DATA_BASE + 9}},
    {"id": "s2_18j_std_movsb", "code": "fda4", "input_flags": 0x08D7, "flags": 0x0CD7, "retired": 2, "gpr_after": {6: DATA_BASE + 3, 7: DATA_BASE + 7}},
    {"id": "s2_18j_cld_stosb", "code": "fcaa", "input_flags": 0x0CD7, "flags": 0x08D7, "input_rax": 0x5A, "retired": 2, "gpr_after": {7: DATA_BASE + 9}, "data_after": "00112233414141415a4141aabbccddee"},
    {"id": "s2_18j_std_stosb", "code": "fdaa", "input_flags": 0x08D7, "flags": 0x0CD7, "input_rax": 0x5A, "retired": 2, "gpr_after": {7: DATA_BASE + 7}, "data_after": "00112233414141415a4141aabbccddee"},
    {"id": "s2_18j_cld_lods", "code": "fcac", "input_flags": 0x0CD7, "flags": 0x08D7, "retired": 2, "gpr_after": {0: 0x8877665544332241, 6: DATA_BASE + 5}},
    {"id": "s2_18j_std_lods", "code": "fdac", "input_flags": 0x08D7, "flags": 0x0CD7, "retired": 2, "gpr_after": {0: 0x8877665544332241, 6: DATA_BASE + 3}},
    {"id": "s2_18j_cld_scasb", "code": "fcae", "input_flags": 0x0CD7, "input_rax": 0x41, "flags": 0x046, "retired": 2, "gpr_after": {7: DATA_BASE + 9}},
    {"id": "s2_18j_std_scasb", "code": "fdae", "input_flags": 0x08D7, "input_rax": 0x41, "flags": 0x446, "retired": 2, "gpr_after": {7: DATA_BASE + 7}},
    {"id": "s2_18j_cld_cmpsb", "code": "fca6", "input_flags": 0x0CD7, "flags": 0x046, "retired": 2, "gpr_after": {6: DATA_BASE + 5, 7: DATA_BASE + 9}},
    {"id": "s2_18j_std_cmpsb", "code": "fda6", "input_flags": 0x08D7, "flags": 0x446, "retired": 2, "gpr_after": {6: DATA_BASE + 3, 7: DATA_BASE + 7}},
    # Three-element REP MOVS/STOS/LODS with each direction.
    {"id": "s2_18j_cld_rep_movsb", "code": "fcf3a4", "input_flags": 0x0CD7, "input_count": 3, "input_rsi": DATA_BASE + 4, "input_rdi": DATA_BASE + 8, "flags": 0x08D7, "retired": 2, "gpr_after": {1: 0, 6: DATA_BASE + 7, 7: DATA_BASE + 11}},
    {"id": "s2_18j_std_rep_movsb", "code": "fdf3a4", "input_flags": 0x08D7, "input_count": 3, "input_rsi": DATA_BASE + 6, "input_rdi": DATA_BASE + 10, "flags": 0x0CD7, "retired": 2, "gpr_after": {1: 0, 6: DATA_BASE + 3, 7: DATA_BASE + 7}},
    {"id": "s2_18j_cld_rep_stosb", "code": "fcf3aa", "input_flags": 0x0CD7, "input_count": 3, "input_rax": 0x5A, "input_rdi": DATA_BASE + 8, "flags": 0x08D7, "retired": 2, "gpr_after": {1: 0, 7: DATA_BASE + 11}, "data_after": "00112233414141415a5a5aaabbccddee"},
    {"id": "s2_18j_std_rep_stosb", "code": "fdf3aa", "input_flags": 0x08D7, "input_count": 3, "input_rax": 0x5A, "input_rdi": DATA_BASE + 10, "flags": 0x0CD7, "retired": 2, "gpr_after": {1: 0, 7: DATA_BASE + 7}, "data_after": "00112233414141415a5a5aaabbccddee"},
    {"id": "s2_18j_cld_rep_lods", "code": "fcf3ac", "input_flags": 0x0CD7, "input_count": 3, "input_rsi": DATA_BASE + 4, "flags": 0x08D7, "retired": 2, "gpr_after": {0: 0x8877665544332241, 1: 0, 6: DATA_BASE + 7}},
    {"id": "s2_18j_std_rep_lods", "code": "fdf3ac", "input_flags": 0x08D7, "input_count": 3, "input_rsi": DATA_BASE + 6, "flags": 0x0CD7, "retired": 2, "gpr_after": {0: 0x8877665544332241, 1: 0, 6: DATA_BASE + 3}},
    # CMP makes every arithmetic condition flag defined before REPE consumes
    # the equal byte range. Direction metadata remains defined by CLD/STD.
    {"id": "s2_18j_cld_repe_scasb", "code": "4839c0fcf3ae", "input_flags": 0x0CD7, "input_rax": 0x41, "input_count": 3, "input_rdi": DATA_BASE + 8, "flags": 0x046, "defined": DEFINED, "retired": 3, "gpr_after": {1: 0, 7: DATA_BASE + 11}},
    {"id": "s2_18j_std_repe_scasb", "code": "4839c0fdf3ae", "input_flags": 0x08D7, "input_rax": 0x41, "input_count": 3, "input_rdi": DATA_BASE + 10, "flags": 0x446, "defined": DEFINED, "retired": 3, "gpr_after": {1: 0, 7: DATA_BASE + 7}},
    {"id": "s2_18j_cld_repe_cmpsb", "code": "4839c0fcf3a6", "input_flags": 0x0CD7, "input_rax": 0x41, "input_count": 3, "input_rsi": DATA_BASE + 4, "input_rdi": DATA_BASE + 8, "flags": 0x046, "defined": DEFINED, "retired": 3, "gpr_after": {1: 0, 6: DATA_BASE + 7, 7: DATA_BASE + 11}},
    {"id": "s2_18j_std_repe_cmpsb", "code": "4839c0fdf3a6", "input_flags": 0x08D7, "input_rax": 0x41, "input_count": 3, "input_rsi": DATA_BASE + 6, "input_rdi": DATA_BASE + 10, "flags": 0x446, "defined": DEFINED, "retired": 3, "gpr_after": {1: 0, 6: DATA_BASE + 3, 7: DATA_BASE + 7}},
    # A fault in the second instruction preserves CLD/STD's committed prefix.
    {"id": "s2_18j_cld_lods_unmapped", "code": "fcac", "input_flags": 0x0CD7, "input_rsi": 0x00201000, "flags": 0x08D7, "retired": 1, "budget": 2, "exit": "harness_error", "gpr_after": {6: 0x00201000}},
    {"id": "s2_18j_std_lods_unmapped", "code": "fdac", "input_flags": 0x08D7, "input_rsi": 0x00201000, "flags": 0x0CD7, "retired": 1, "budget": 2, "exit": "harness_error", "gpr_after": {6: 0x00201000}},
    # TEST/CLD-or-STD/REPE SCAS rejects the zero-accumulator metadata profile.
    {"id": "s2_18j_cld_repe_scasb_zero_rax_reject", "code": "4885c0fcf3ae", "input_flags": 0x0C02, "input_rax": 0, "input_count": 3, "flags": 0x046, "defined": TEST_DEFINED, "retired": 2, "rip_offset": 4, "budget": 3, "exit": "unsupported"},
    {"id": "s2_18j_std_repe_scasb_zero_rax_reject", "code": "4885c0fdf3ae", "input_flags": 0x0802, "input_rax": 0, "input_count": 3, "flags": 0x446, "defined": TEST_DEFINED, "retired": 2, "rip_offset": 4, "budget": 3, "exit": "unsupported"},
]


def materialize() -> dict[str, bytes]:
    assert len(CASES) == 32
    generated: dict[str, bytes] = {}
    answers: dict[str, object] = {}
    for case in CASES:
        code = bytes.fromhex(case["code"])
        before = GPRS.copy()
        before[0] = case.get("input_rax", before[0])
        before[1] = case.get("input_count", before[1])
        before[6] = case.get("input_rsi", before[6])
        before[7] = case.get("input_rdi", before[7])
        after = before.copy()
        for index, value in case.get("gpr_after", {}).items():
            after[index] = value
        rip_after = CODE_BASE + case.get(
            "rip_offset",
            len(code) if case.get("exit", "stop_address") == "stop_address"
            else 1 if case.get("exit") == "harness_error" else 0,
        )
        data_after = case.get("data_after", INITIAL_DATA)
        outcome = case.get("exit", "stop_address")
        fixture = {
            "schema_version": 2,
            "case_id": case["id"],
            "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
            "source": "Handwritten CLD/STD bytes and literal full-state/memory answers; tests/cpu/generate_stage2_cld_std.py serializes these constants only. No assembler, oracle, or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B CLD/STD; project S2-18j defines only bare primary-map FC/FD in its local Long64 profile.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-18j",
            },
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {
                "mode": "long64", "gpr": [hx(value) for value in before],
                "rip": hx(CODE_BASE), "rflags": hx(case["input_flags"]),
            },
            "memory": [
                {"region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code),
                 "permissions": {"read": False, "write": False, "execute": True},
                 "initial_bytes_hex": case["code"], "comparison_mask_hex": "ff" * len(code)},
                {"region_id": "data", "guest_address": hx(DATA_BASE), "length": 16,
                 "permissions": {"read": True, "write": True, "execute": False},
                 "initial_bytes_hex": INITIAL_DATA, "comparison_mask_hex": "ff" * 16},
            ],
            "stop_rip": hx(CODE_BASE + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": case.get("budget", max(case["retired"], 1)),
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        generated[f"{case['id']}.case.json"] = dump(fixture)
        answer = {
            "gpr": [hx(value) for value in after],
            "rflags": hx(case["flags"]),
            "rflags_defined": hx(case.get("defined", DEFINED)),
            "rip": hx(rip_after),
            "instructions_executed": hx(case["retired"]),
            "memory": {"code": case["code"], "data": data_after},
        }
        if outcome == "exception":
            answer["fault"] = {"vector": 13, "error_code": hx(0)}
        answers[case["id"]] = answer
    generated["s2_18j_cld_std_expected.json"] = dump(answers)
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
    print("S2-18j CLD/STD fixtures are fresh" if args.check else "wrote 33 S2-18j fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
