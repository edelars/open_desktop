#!/usr/bin/env python3
"""Serialize literal S2-19m address-size-32 REPE SCASB fixtures."""
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
BASE = 0x300000
MASK = (1 << 64) - 1
PF_ZF = 0x44


def hx(value: int) -> str:
    return f"0x{value & MASK:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    # Rows are hand-authored input and output state. This helper only serializes them.
    def add(case_id: str, code: bytes, *, rcx: int, rdi: int, rax: int,
            flags: int, data_base: int, data: bytes, final_rcx: int,
            final_rdi: int, final_flags: int, outcome: str, final_rip: int,
            budget: int = 1, defined: int = 0x0CD7) -> None:
        initial = BASE_GPR.copy()
        initial[0], initial[1], initial[7] = rax, rcx, rdi
        final = initial.copy()
        final[1], final[7] = final_rcx, final_rdi
        code_region = {
            "region_id": "code", "guest_address": hx(0x100000), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }
        regions = [code_region]
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
                "source": "Handwritten prefix bytes and literal complete snapshots; this generator serializes constants only. No assembler, CPU model, or executor is used.",
                "specification": "Intel SDM revision 093, Vol.2B SCAS and REP/REPE entries; project S2-19m bounded address-size-32 REPE SCASB.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19m",
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

    add("s2_19m_scasb32_zero_f3_67", bytes.fromhex("f367ae"),
        rcx=0xdeadbeef00000000, rdi=0xfeedface80000000, rax=BASE_GPR[0],
        flags=2 | PF_ZF, data_base=BASE, data=b"A", final_rcx=0xdeadbeef00000000,
        final_rdi=0xfeedface80000000, final_flags=2 | PF_ZF, outcome="stop_address",
        final_rip=0x100003)
    add("s2_19m_scasb32_zero_67f3", bytes.fromhex("67f3ae"),
        rcx=0x1234567800000000, rdi=0xabcdef0180000000, rax=BASE_GPR[0],
        flags=2 | PF_ZF | 0x400, data_base=BASE, data=b"A", final_rcx=0x1234567800000000,
        final_rdi=0xabcdef0180000000, final_flags=2 | PF_ZF | 0x400,
        outcome="stop_address", final_rip=0x100003)
    add("s2_19m_scasb32_equal_ecx_uses_low_pointer", bytes.fromhex("f367ae"),
        rcx=0xfacecafe00000001, rdi=0xdeadbeef00001000, rax=0xabcdef0123456741,
        flags=2 | PF_ZF, data_base=0x1000, data=b"A", final_rcx=0,
        final_rdi=0x1001, final_flags=0x46, outcome="stop_address", final_rip=0x100003)
    add("s2_19m_scasb32_mismatch_df", bytes.fromhex("67f3ae"),
        rcx=0x1234567800000002, rdi=0xabcdef0100001001, rax=0x1122334455667741,
        flags=2 | PF_ZF | 0x400, data_base=0x1001, data=b"B", final_rcx=1,
        final_rdi=0x1000, final_flags=0x497, outcome="stop_address", final_rip=0x100003)
    add("s2_19m_scasb32_unmapped_first", bytes.fromhex("f367ae"),
        rcx=1, rdi=0xfacecafe00002000, rax=BASE_GPR[0], flags=2 | PF_ZF,
        data_base=-1, data=b"", final_rcx=1, final_rdi=0xfacecafe00002000,
        final_flags=2 | PF_ZF, outcome="harness_error", final_rip=0x100000)
    add("s2_19m_scasb32_unmapped_after_equal", bytes.fromhex("67f3ae"),
        rcx=2, rdi=0xfacecafe00001000, rax=BASE_GPR[0], flags=2 | PF_ZF,
        data_base=0x1000, data=b"A", final_rcx=1, final_rdi=0x1001,
        final_flags=0x46, outcome="harness_error", final_rip=0x100000)
    add("s2_19m_scasb32_limit_1025", bytes.fromhex("f367ae"),
        rcx=0xaaaaaaaa00000401, rdi=0xbbbbbbbb00001000, rax=BASE_GPR[0],
        flags=2 | PF_ZF, data_base=0x1000, data=b"A" * 1024, final_rcx=1,
        final_rdi=0x1400, final_flags=0x46, outcome="rep_iteration_budget_exhausted",
        final_rip=0x100000)
    add("s2_19m_scasb32_budget_zero", bytes.fromhex("67f3ae"),
        rcx=1, rdi=0x1000, rax=BASE_GPR[0], flags=2 | PF_ZF,
        data_base=0x1000, data=b"A", final_rcx=1, final_rdi=0x1000,
        final_flags=2 | PF_ZF, outcome="instruction_budget_exhausted", final_rip=0x100000,
        budget=0)
    composed = bytes.fromhex("4839c067f3ae")
    add("s2_19m_cmp_rax_rax_rep_scasb32_success", composed,
        rcx=1, rdi=0x1000, rax=0, flags=0x8D7, data_base=0x1000, data=b"\0",
        final_rcx=0, final_rdi=0x1001, final_flags=0x46, outcome="stop_address",
        final_rip=0x100006, budget=2)
    test = bytes.fromhex("4885c067f3ae")
    add("s2_19m_test_rax_rep_scasb32_undefined_af", test,
        rcx=1, rdi=0x1000, rax=0, flags=0x0CC7, data_base=0x1000, data=b"\0",
        final_rcx=1, final_rdi=0x1000, final_flags=0x446, outcome="unsupported",
        final_rip=0x100003, budget=2, defined=0x0CC7)
    add("s2_19m_scasb32_zero_bad_gate", bytes.fromhex("f367ae"),
        rcx=0xaaaa000000000000, rdi=0xffff000080000000, rax=BASE_GPR[0],
        flags=2 | 0x40, data_base=BASE, data=b"A", final_rcx=0xaaaa000000000000,
        final_rdi=0xffff000080000000, final_flags=2 | 0x40, outcome="unsupported",
        final_rip=0x100000)
    add("s2_19m_scasb32_extra_prefix_rejected", bytes.fromhex("f36767ae"),
        rcx=1, rdi=0x1000, rax=BASE_GPR[0], flags=2 | PF_ZF,
        data_base=0x1000, data=b"A", final_rcx=1, final_rdi=0x1000,
        final_flags=2 | PF_ZF, outcome="unsupported", final_rip=0x100000)
    add("s2_19m_cmpsd32_duplicate_66_rejected", bytes.fromhex("f3676666a7"),
        rcx=1, rdi=0x1000, rax=BASE_GPR[0], flags=2 | PF_ZF,
        data_base=0x1000, data=b"A", final_rcx=1, final_rdi=0x1000,
        final_flags=2 | PF_ZF, outcome="unsupported", final_rip=0x100000)
    assert len(answers) == 13
    files["s2_19m_rep_scasb_addr32_expected.json"] = dump(answers)
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
