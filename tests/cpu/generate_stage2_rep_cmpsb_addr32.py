#!/usr/bin/env python3
"""Serialize hand-authored S2-19n REPE CMPSB address-size-32 fixtures."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE_GPR = [0x1122334455667741, 0, 0, 0x200000, 0x1000, 0x2000, 0x200000,
            0x300000, 0x0123456789ABCDEF, 0xFEDCBA9876543210,
            0x8877665544332211, 0xAABBCCDDEEFF0011, 0x1122112211221122,
            0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
MASK = (1 << 64) - 1
PF_ZF = 0x44


def hx(value: int) -> str:
    return f"0x{value & MASK:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    # This function serializes fixed inputs and literal answers; it performs no
    # instruction modeling or execution.
    def add(case_id: str, code: bytes, *, rcx: int, rsi: int, rdi: int,
            flags: int, data_base: int, data: bytes, final_rcx: int,
            final_rsi: int, final_rdi: int, final_flags: int, outcome: str,
            final_rip: int, budget: int = 1, defined: int = 0x0CD7,
            rax: int = BASE_GPR[0]) -> None:
        initial = BASE_GPR.copy()
        initial[0] = rax
        initial[1], initial[6], initial[7] = rcx, rsi, rdi
        final = initial.copy()
        final[1], final[6], final[7] = final_rcx, final_rsi, final_rdi
        regions = [{
            "region_id": "code", "guest_address": hx(0x100000), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }]
        if data_base >= 0:
            regions.append({
                "region_id": "data", "guest_address": hx(data_base), "length": len(data),
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data),
            })
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten bytes and literal complete snapshots; tests/cpu/generate_stage2_rep_cmpsb_addr32.py serializes constants only. No assembler, CPU model, or executor is used.",
                "specification": "Intel SDM revision 093, Vol.2B CMPS and REP/REPE entries; project S2-19n local address-size-32 profile.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19n",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in initial],
                              "rip": hx(0x100000), "rflags": hx(flags)},
            "memory": regions,
            "stop_rip": hx(0x100000 + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x8D5), "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answers[case_id] = {
            "gpr": [hx(x) for x in final], "rflags": hx(final_flags),
            "rflags_defined": hx(defined), "rip": hx(final_rip),
            "memory": {region["region_id"]: region["initial_bytes_hex"] for region in regions},
        }

    for code, suffix in [(bytes.fromhex("f367a6"), "f3_67"),
                         (bytes.fromhex("67f3a6"), "67_f3")]:
        add(f"s2_19n_cmpsb32_zero_{suffix}", code,
            rcx=0xdeadbeef00000000, rsi=0xabcdef0180000000,
            rdi=0xfeedface80000000, flags=2 | PF_ZF, data_base=-1, data=b"",
            final_rcx=0xdeadbeef00000000, final_rsi=0xabcdef0180000000,
            final_rdi=0xfeedface80000000, final_flags=2 | PF_ZF,
            outcome="stop_address", final_rip=0x100003)
    add("s2_19n_cmpsb32_equal_high_halves", bytes.fromhex("f367a6"),
        rcx=0xfacecafe00000002, rsi=0xdeadbeef00001000,
        rdi=0xcafebabe00001001, flags=2 | PF_ZF, data_base=0x1000,
        data=b"AAA", final_rcx=0, final_rsi=0x1002, final_rdi=0x1003,
        final_flags=0x46, outcome="stop_address", final_rip=0x100003)
    add("s2_19n_cmpsb32_mismatch_position2", bytes.fromhex("f367a6"),
        rcx=0x1234567800000002, rsi=0xdeadbeef00001000,
        rdi=0xcafebabe00002000, flags=2 | PF_ZF, data_base=0x1000,
        data=b"A" * 0x1001 + b"B", final_rcx=0, final_rsi=0x1002, final_rdi=0x2002,
        final_flags=0x97, outcome="stop_address", final_rip=0x100003)
    add("s2_19n_cmpsb32_source_unmapped", bytes.fromhex("f367a6"),
        rcx=1, rsi=0xdeadbeef00003000, rdi=0xcafebabe00004000,
        flags=2 | PF_ZF, data_base=-1, data=b"", final_rcx=1,
        final_rsi=0xdeadbeef00003000, final_rdi=0xcafebabe00004000,
        final_flags=2 | PF_ZF, outcome="harness_error", final_rip=0x100000)
    add("s2_19n_cmpsb32_destination_unmapped", bytes.fromhex("67f3a6"),
        rcx=1, rsi=0xdeadbeef00001000, rdi=0xcafebabe00002000,
        flags=2 | PF_ZF, data_base=0x1000, data=b"A", final_rcx=1,
        final_rsi=0xdeadbeef00001000, final_rdi=0xcafebabe00002000,
        final_flags=2 | PF_ZF, outcome="harness_error", final_rip=0x100000)
    add("s2_19n_cmpsb32_limit_1025", bytes.fromhex("f367a6"),
        rcx=0xaaaaaaaa00000401, rsi=0xbbbbbbbb00001000,
        rdi=0xcccccccc00002000, flags=2 | PF_ZF, data_base=0x1000,
        data=b"A" * 0x1400, final_rcx=1,
        final_rsi=0x1400, final_rdi=0x2400, final_flags=0x46,
        outcome="rep_iteration_budget_exhausted", final_rip=0x100000)
    add("s2_19n_cmpsb32_budget_zero", bytes.fromhex("67f3a6"),
        rcx=1, rsi=0x1000, rdi=0x2000, flags=2 | PF_ZF,
        data_base=-1, data=b"", final_rcx=1, final_rsi=0x1000,
        final_rdi=0x2000, final_flags=2 | PF_ZF,
        outcome="instruction_budget_exhausted", final_rip=0x100000, budget=0)
    cmp_rep = bytes.fromhex("4839c0f367a6")
    add("s2_19n_cmp_rax_rax_rep_cmpsb32_success", cmp_rep,
        rcx=1, rsi=0x1000, rdi=0x1001, flags=0x8D7, data_base=0x1000,
        data=b"AA", final_rcx=0, final_rsi=0x1001, final_rdi=0x1002,
        final_flags=0x46, outcome="stop_address", final_rip=0x100006, budget=2)
    test_rep = bytes.fromhex("4885c067f3a6")
    add("s2_19n_test_rax_rep_cmpsb32_undefined_af", test_rep,
        rcx=1, rsi=0x1000, rdi=0x1001, flags=0x0CC7, data_base=0x1000,
        data=b"AA", final_rcx=1, final_rsi=0x1000, final_rdi=0x1001,
        final_flags=0x446, outcome="unsupported", final_rip=0x100003,
        budget=2, defined=0x0CC7, rax=0)
    add("s2_19n_cmpsb32_zero_bad_gate", bytes.fromhex("f367a6"),
        rcx=0xaaaa000000000000, rsi=0xffff000080000000,
        rdi=0xeeee000080000000, flags=2 | 0x40, data_base=-1, data=b"",
        final_rcx=0xaaaa000000000000, final_rsi=0xffff000080000000,
        final_rdi=0xeeee000080000000, final_flags=2 | 0x40,
        outcome="unsupported", final_rip=0x100000)
    add("s2_19n_cmpsb32_extra_prefix_rejected", bytes.fromhex("f36767a6"),
        rcx=1, rsi=0x1000, rdi=0x1001, flags=2 | PF_ZF,
        data_base=0x1000, data=b"AA", final_rcx=1, final_rsi=0x1000,
        final_rdi=0x1001, final_flags=2 | PF_ZF,
        outcome="unsupported", final_rip=0x100000)
    add("s2_19n_cmpsb32_af_rejected", bytes.fromhex("f367a7"),
        rcx=1, rsi=0x1000, rdi=0x1001, flags=2 | PF_ZF,
        data_base=0x1000, data=b"AA", final_rcx=1, final_rsi=0x1000,
        final_rdi=0x1001, final_flags=2 | PF_ZF,
        outcome="unsupported", final_rip=0x100000)
    assert len(answers) == 13
    files["s2_19n_rep_cmpsb_addr32_expected.json"] = dump(answers)
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
