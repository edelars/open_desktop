#!/usr/bin/env python3
"""Serialize hand-authored S2-19p REPE CMPSW/D/Q address-size-32 fixtures."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE_GPR = [0x1122334455667741, 0, 0, 0x200000, 0x1000, 0x300000,
            0x200000, 0x301000, 0x0123456789ABCDEF, 0xFEDCBA9876543210,
            0x8877665544332211, 0xAABBCCDDEEFF0011, 0x1122112211221122,
            0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
MASK = (1 << 64) - 1
PF_ZF = 0x44


def hx(value: int) -> str:
    return f"0x{value & MASK:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def region(region_id: str, address: int, data: bytes, *, execute: bool = False) -> dict[str, object]:
    return {
        "region_id": region_id, "guest_address": hx(address), "length": len(data),
        "permissions": {"read": not execute, "write": False, "execute": execute},
        "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data),
    }


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    # Inputs and expected snapshots are authored literals. This only serializes them.
    def add(case_id: str, code: bytes, *, rcx: int, rsi: int, rdi: int,
            flags: int, regions: list[dict[str, object]], final_rcx: int,
            final_rsi: int, final_rdi: int, final_flags: int, outcome: str,
            final_rip: int, budget: int = 1, defined: int = 0x0CD7,
            rax: int = BASE_GPR[0]) -> None:
        initial = BASE_GPR.copy()
        initial[0], initial[1], initial[6], initial[7] = rax, rcx, rsi, rdi
        final = initial.copy()
        final[1], final[6], final[7] = final_rcx, final_rsi, final_rdi
        code_region = region("code", 0x100000, code, execute=True)
        fixture = {
            "schema_version": 2, "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten bytes and literal complete snapshots; this generator serializes constants only. No assembler, CPU model, or executor is used.",
                "specification": "Intel SDM revision 093, Vol.2B CMPS and REP/REPE entries; project S2-19p bounded address-size-32 profile.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19p",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in initial],
                              "rip": hx(0x100000), "rflags": hx(flags)},
            "memory": [code_region, *regions],
            "stop_rip": hx(0x100000 + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x8D5), "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answers[case_id] = {
            "gpr": [hx(x) for x in final], "rflags": hx(final_flags),
            "rflags_defined": hx(defined), "rip": hx(final_rip),
            "memory": {r["region_id"]: r["initial_bytes_hex"] for r in [code_region, *regions]},
        }

    word = bytes.fromhex("f36766a7")
    dword = bytes.fromhex("f367a7")
    qword = bytes.fromhex("f36748a7")
    add("s2_19p_cmpsw_zero_high_halves", word,
        rcx=0xdeadbeef00000000, rsi=0xabcdef0180000000,
        rdi=0xfeedface80000000, flags=2 | PF_ZF, regions=[],
        final_rcx=0xdeadbeef00000000, final_rsi=0xabcdef0180000000,
        final_rdi=0xfeedface80000000, final_flags=2 | PF_ZF,
        outcome="stop_address", final_rip=0x100004)
    add("s2_19p_cmpsw_equal_legacy_order", bytes.fromhex("67f366a7"),
        rcx=1, rsi=0xdeadbeef00001000, rdi=0xcafebabe00001010,
        flags=2 | PF_ZF, regions=[region("data", 0x1000, bytes.fromhex("4100" + "00" * 14 + "4100"))],
        final_rcx=0, final_rsi=0x1002, final_rdi=0x1012,
        final_flags=0x46, outcome="stop_address", final_rip=0x100004)
    add("s2_19p_cmpsd_mismatch_df", dword, rcx=0x1234567800000002,
        rsi=0xdeadbeef00001000, rdi=0xcafebabe00001010,
        flags=2 | PF_ZF | 0x400,
        regions=[region("data", 0x1000, bytes.fromhex("41000000" + "00" * 12 + "42000000"))],
        final_rcx=1, final_rsi=0x0ffc, final_rdi=0x100c, final_flags=0x497,
        outcome="stop_address", final_rip=0x100003)
    add("s2_19p_cmpsq_contiguous_read_crosses_4g", qword,
        rcx=0xaaaaaaaa00000001, rsi=0xfacecafeffffffff,
        rdi=0xbeefcafeffffffff, flags=2 | PF_ZF,
        regions=[region("data", 0xffffffff, bytes.fromhex("4100000000000000"))],
        final_rcx=0, final_rsi=7, final_rdi=7, final_flags=0x46,
        outcome="stop_address", final_rip=0x100004)
    add("s2_19p_cmpsq_destination_unmapped", qword,
        rcx=1, rsi=0xdeadbeef00001000, rdi=0xcafebabe00002000,
        flags=2 | PF_ZF, regions=[region("source", 0x1000, bytes.fromhex("4100000000000000"))],
        final_rcx=1, final_rsi=0xdeadbeef00001000, final_rdi=0xcafebabe00002000,
        final_flags=2 | PF_ZF, outcome="harness_error", final_rip=0x100000)
    add("s2_19p_cmpsw_source_unmapped", word,
        rcx=1, rsi=0xdeadbeef00003000, rdi=0xcafebabe00004000,
        flags=2 | PF_ZF, regions=[], final_rcx=1,
        final_rsi=0xdeadbeef00003000, final_rdi=0xcafebabe00004000,
        final_flags=2 | PF_ZF, outcome="harness_error", final_rip=0x100000)
    add("s2_19p_cmpsd_zero_budget", dword, rcx=1,
        rsi=0xdeadbeef00001000, rdi=0xcafebabe00002000,
        flags=2 | PF_ZF, regions=[], final_rcx=1,
        final_rsi=0xdeadbeef00001000, final_rdi=0xcafebabe00002000,
        final_flags=2 | PF_ZF, outcome="instruction_budget_exhausted",
        final_rip=0x100000, budget=0)
    add("s2_19p_cmpsw_limit_1025", word, rcx=0xaaaaaaaa00000401,
        rsi=0xbbbbbbbb00001000, rdi=0xcccccccc00003000,
        flags=2 | PF_ZF,
        regions=[region("data", 0x1000, bytes.fromhex("4100") * 1024 + bytes(0x1800) + bytes.fromhex("4100") * 1024)],
        final_rcx=1, final_rsi=0x1800, final_rdi=0x3800, final_flags=0x46,
        outcome="rep_iteration_budget_exhausted", final_rip=0x100000)
    add("s2_19p_cmp_to_cmpsd_success", bytes.fromhex("4839c067f3a7"),
        rcx=1, rsi=0x1000, rdi=0x1004, flags=0x8D7,
        regions=[region("data", 0x1000, bytes(8))], final_rcx=0,
        final_rsi=0x1004, final_rdi=0x1008, final_flags=0x46,
        outcome="stop_address", final_rip=0x100006, budget=2)
    add("s2_19p_test_to_cmpsw_undefined_af", bytes.fromhex("4885c067f366a7"),
        rcx=1, rsi=0x1000, rdi=0x1002, flags=0x0CC7,
        regions=[region("data", 0x1000, bytes(4))], final_rcx=1,
        final_rsi=0x1000, final_rdi=0x1002, final_flags=0x446,
        outcome="unsupported", final_rip=0x100003, budget=2, defined=0x0CC7, rax=0)
    add("s2_19p_cmpsq_zero_bad_gate", qword,
        rcx=0xaaaa000000000000, rsi=0xffff000080000000,
        rdi=0xeeee000080000000, flags=2 | 0x40, regions=[],
        final_rcx=0xaaaa000000000000, final_rsi=0xffff000080000000,
        final_rdi=0xeeee000080000000, final_flags=2 | 0x40,
        outcome="unsupported", final_rip=0x100000)
    add("s2_19p_cmpsd_duplicate_address_prefix_rejected", bytes.fromhex("f36767a7"),
        rcx=1, rsi=0x1000, rdi=0x1004, flags=2 | PF_ZF,
        regions=[region("data", 0x1000, bytes(8))], final_rcx=1,
        final_rsi=0x1000, final_rdi=0x1004, final_flags=2 | PF_ZF,
        outcome="unsupported", final_rip=0x100000)
    add("s2_19p_cmpsw_66_rex_overrides_to_qword", bytes.fromhex("f3676648a7"),
        rcx=1, rsi=0x1000, rdi=0x1008, flags=2 | PF_ZF,
        regions=[region("data", 0x1000, bytes(16))], final_rcx=0,
        final_rsi=0x1008, final_rdi=0x1010, final_flags=0x46,
        outcome="stop_address", final_rip=0x100005)
    add("s2_19p_rex_discarded_by_66_remains_word", bytes.fromhex("f3674866a7"),
        rcx=1, rsi=0x1000, rdi=0x1002, flags=2 | PF_ZF,
        regions=[region("data", 0x1000, bytes(4))], final_rcx=0,
        final_rsi=0x1002, final_rdi=0x1004, final_flags=0x46,
        outcome="stop_address", final_rip=0x100005)
    assert len(answers) == 14
    files["s2_19p_rep_cmps_wdq_addr32_expected.json"] = dump(answers)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    expected = materialize()
    mismatches = []
    for name, contents in expected.items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                mismatches.append(name)
        else:
            path.write_bytes(contents)
    if mismatches:
        print("out of date: " + ", ".join(mismatches))
        return 1
    print(f"{'checked' if args.check else 'wrote'} {len(expected) - 1} fixtures and literal answers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
