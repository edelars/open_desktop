#!/usr/bin/env python3
"""Serialize hand-authored S2-19e REP SCASB fixtures and literal answers."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE_GPR = [0x1122334455667741, 0, 0xDEADBEEFFEDCBA98, 0x200000,
            0x300080, 0x300080, 0x200000, 0x300000, 0x0123456789ABCDEF,
            0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
            0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
            0xFFFFFFFFFFFFFFFF]
BASE = 0x200000
CODE = bytes.fromhex("f3ae")
PF_ZF = 0x44
EQUAL_FLAGS = 0x46
MISMATCH_FLAGS = 0x97


def hx(value: int) -> str:
    return f"0x{value & ((1 << 64) - 1):016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    def add(case_id: str, *, code: bytes = CODE, count: int, rdi: int,
            data: bytes, initial_flags: int, outcome: str, final_count: int,
            final_rdi: int, final_flags: int, rip: int | None = None,
            budget: int = 1, defined: int = 0x0CD7, fault: dict | None = None,
            data_base: int = BASE, initial_rax: int | None = None) -> None:
        gpr = BASE_GPR.copy()
        if initial_rax is not None:
            gpr[0] = initial_rax
        gpr[1], gpr[7] = count, rdi
        final = gpr.copy()
        final[1], final[7] = final_count, final_rdi
        code_region = {
            "region_id": "code", "guest_address": hx(0x100000), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }
        data_region = {
            "region_id": "data", "guest_address": hx(data_base), "length": len(data),
            "permissions": {"read": True, "write": False, "execute": False},
            "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data),
        }
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten F3 AE bytes and literal state, flag, and memory answers; tests/cpu/generate_stage2_rep_scasb.py serializes constants only. No assembler, oracle, or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B SCAS and REP/REPE entries; SCASB compares AL with ES:[RDI], adjusts RDI by DF, and REPE continues while ZF is set.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19e F3 AE REP SCASB",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gpr],
                              "rip": hx(0x100000), "rflags": hx(initial_flags)},
            "memory": [code_region, data_region],
            "stop_rip": hx(0x100000 + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answer: dict[str, object] = {
            "gpr": [hx(x) for x in final], "rflags": hx(final_flags),
            "rflags_defined": hx(defined),
            "rip": hx(rip if rip is not None else
                      (0x100000 + len(code) if outcome == "stop_address" else 0x100000)),
            "memory": {"code": code.hex(), "data": data.hex()},
        }
        if fault is not None:
            answer["fault"] = fault
        answers[case_id] = answer

    # Zero-count admission still requires the profile flags but never checks RDI.
    for df in (False, True):
        add(f"s2_19e_rep_scasb_zero_df{int(df)}", count=0,
            rdi=0x0000800000000000, data=b"\x41", initial_flags=2 | PF_ZF | (0x400 if df else 0),
            outcome="stop_address", final_count=0, final_rdi=0x0000800000000000,
            final_flags=2 | PF_ZF | (0x400 if df else 0))

    # Count-one equality/mismatch for either direction.
    for df in (False, True):
        direction = -1 if df else 1
        start = BASE + (2 if df else 0)
        for equal, suffix, byte, flags in [
            (True, "equal", 0x41, EQUAL_FLAGS),
            (False, "mismatch", 0x42, MISMATCH_FLAGS),
        ]:
            data = bytearray(b"\x41" * (3 if df else 1))
            data[2 if df else 0] = byte
            add(f"s2_19e_rep_scasb_one_{suffix}_df{int(df)}", count=1,
                rdi=start, data=bytes(data), initial_flags=2 | PF_ZF | (0x400 if df else 0),
                outcome="stop_address", final_count=0, final_rdi=start + direction,
                final_flags=flags | (0x400 if df else 0))

    # Three-count mismatches at each 1-based comparison position and each DF.
    for df in (False, True):
        direction = -1 if df else 1
        start = BASE + (2 if df else 0)
        for position in (1, 2, 3):
            values = [0x41] * 3
            values[2 - (position - 1) if df else position - 1] = 0x42
            add(f"s2_19e_rep_scasb_mismatch_{position}_df{int(df)}", count=3,
                rdi=start, data=bytes(values), initial_flags=2 | PF_ZF | (0x400 if df else 0),
                outcome="stop_address", final_count=3 - position,
                final_rdi=start + direction * position,
                final_flags=MISMATCH_FLAGS | (0x400 if df else 0))

    for df in (False, True):
        direction = -1 if df else 1
        start = BASE + (2 if df else 0)
        add(f"s2_19e_rep_scasb_all_equal_df{int(df)}", count=3,
            rdi=start, data=b"\x41\x41\x41", initial_flags=2 | PF_ZF | (0x400 if df else 0),
            outcome="stop_address", final_count=0, final_rdi=start + direction * 3,
            final_flags=EQUAL_FLAGS | (0x400 if df else 0))

    # Mapping ends after 0, 1, or 2 successful equal reads.
    for successes in (0, 1, 2):
        start = BASE if successes else BASE + 0x100
        data = b"\x41" * max(successes, 1)
        add(f"s2_19e_rep_scasb_unmapped_after_{successes}", count=3,
            rdi=start, data=data, initial_flags=2 | PF_ZF,
            outcome="harness_error", final_count=3 - successes,
            final_rdi=start + successes,
            final_flags=EQUAL_FLAGS if successes else 2 | PF_ZF,
            fault={"access": "read", "address": hx(start + successes)})

    add("s2_19e_rep_scasb_noncanonical_first", count=1,
        rdi=0x0000800000000000, data=b"\x41", initial_flags=2 | PF_ZF,
        outcome="exception", final_count=1, final_rdi=0x0000800000000000,
        final_flags=2 | PF_ZF, fault={"vector": 13, "error_code": 0, "linear_address": None})

    for df in (False, True):
        start = BASE + (1024 if df else 0)
        add(f"s2_19e_rep_scasb_limit_1025_df{int(df)}", count=1025,
            rdi=start, data=b"\x41" * 1025,
        initial_flags=2 | PF_ZF | (0x400 if df else 0),
            outcome="rep_iteration_budget_exhausted", final_count=1,
            final_rdi=start + (-1024 if df else 1024),
            final_flags=EQUAL_FLAGS | (0x400 if df else 0))

    add("s2_19e_rep_scasb_budget_zero", count=1, rdi=BASE, data=b"\x41",
        initial_flags=2 | PF_ZF, outcome="instruction_budget_exhausted", final_count=1,
        final_rdi=BASE, final_flags=2 | PF_ZF, budget=0, rip=0x100000)

    composed = bytes.fromhex("4839c0f3ae")
    add("s2_19e_cmp_rax_rax_rep_scasb_success", code=composed, count=1,
        rdi=BASE, data=b"\x41", initial_flags=0x08D7,
        outcome="stop_address", final_count=0, final_rdi=BASE + 1,
        final_flags=EQUAL_FLAGS, rip=0x100005, budget=2)
    add("s2_19e_cmp_rax_rax_rep_scasb_fault_after_one", code=composed, count=2,
        rdi=BASE, data=b"\x41", initial_flags=0x08D7,
        outcome="harness_error", final_count=1, final_rdi=BASE + 1,
        final_flags=EQUAL_FLAGS, rip=0x100003, budget=2,
        fault={"access": "read", "address": hx(BASE + 1)})

    test_code = bytes.fromhex("4885c0f3ae")
    add("s2_19e_test_rax_rep_scasb_undefined_af_reject", code=test_code,
        count=1, rdi=BASE, data=b"\x41", initial_flags=0x0CC7,
        outcome="unsupported", final_count=1, final_rdi=BASE,
        final_flags=0x446, defined=0x0CC7, rip=0x100003, budget=2,
        initial_rax=0)

    assert len(answers) == 24
    files["s2_19e_rep_scasb_expected.json"] = dump(answers)
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
