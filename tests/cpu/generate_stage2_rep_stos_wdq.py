#!/usr/bin/env python3
"""Serialize handwritten literal S2-19b REP STOSW/D/Q fixtures."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE_GPRS = [
    0x1122334455667788,
    0,
    0xDEADBEEFFEDCBA98,
    0x0000000000200000,
    0x0000000000300080,
    0x0000000000300080,
    0x0000000000200009,
    0x0000000000200002,
    0x0123456789ABCDEF,
    0xFEDCBA9876543210,
    0x8877665544332211,
    0xAABBCCDDEEFF0011,
    0x1122112211221122,
    0x3344334433443344,
    0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]
DATA = "00112233445566778899aabbccddeeff"
WIDTHS = (("word", 2, bytes.fromhex("f366ab")), ("dword", 4, bytes.fromhex("f3ab")), ("qword", 8, bytes.fromhex("f348ab")))


def hx(value: int) -> str:
    return f"0x{value:016x}"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}

    def add(case_id: str, width: int, code: bytes, *, count: int, rdi: int,
            df: bool, data_len: int, data_in: str, data_out: str,
            outcome: str, out_count: int, out_rdi: int, budget: int = 1,
            fault: dict[str, object] | None = None, out_flags: int | None = None,
            out_defined: int = 0x0CD7, out_rip: int | None = None) -> None:
        gprs = BASE_GPRS.copy()
        gprs[1] = count
        gprs[7] = rdi
        flags = 0x0CD7 if df else 0x08D7
        expected_gprs = gprs.copy()
        expected_gprs[1] = out_count
        expected_gprs[7] = out_rdi
        stop_rip = 0x00100000 + len(code) if outcome == "stop_address" else None
        spec = "S2-19b REP STOSW/D/Q" if width in (2, 4, 8) else "S2-19a REP STOSB"
        fixture = {
            "schema_version": 2,
            "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": (
                    f"Handwritten {code.hex()} bytes and predetermined complete state/memory answers; "
                    "tests/cpu/generate_stage2_rep_stos_wdq.py serializes constants only. "
                    "No assembler or execution engine is used."
                ),
                "specification": (
                    "Intel SDM revision 093, Vol.2B REP and STOS entries: F3 repeats the selected "
                    "STOS width according to RCX; low RAX is stored at ES:[RDI], DF selects index "
                    "direction, and completed iterations record remaining RCX and next RDI."
                ),
                "specification_version": f"Intel SDM Vol.2B revision 093 (253667-093US); project {spec}",
            },
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {
                "mode": "long64",
                "gpr": [hx(value) for value in gprs],
                "rip": hx(0x00100000),
                "rflags": hx(flags),
            },
            "memory": [
                {
                    "region_id": "code",
                    "guest_address": hx(0x00100000),
                    "length": len(code),
                    "permissions": {"read": False, "write": False, "execute": True},
                    "initial_bytes_hex": code.hex(),
                    "comparison_mask_hex": "ff" * len(code),
                },
                {
                    "region_id": "data",
                    "guest_address": hx(0x00200000),
                    "length": data_len,
                    "permissions": {"read": True, "write": True, "execute": False},
                    "initial_bytes_hex": data_in,
                    "comparison_mask_hex": "ff" * data_len,
                },
            ],
            "stop_rip": hx(stop_rip) if stop_rip is not None else None,
            "instruction_budget": budget,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = json_bytes(fixture)
        answer: dict[str, object] = {
            "gpr": [hx(value) for value in expected_gprs],
            "rflags": hx(flags if out_flags is None else out_flags),
            "rflags_defined": hx(out_defined),
            "rip": hx(stop_rip if stop_rip is not None else (out_rip or 0x00100000)),
            "memory": {"code": code.hex(), "data": data_out},
        }
        if fault is not None:
            answer["fault"] = fault
        expected[case_id] = answer

    # These rows are hand-calculated literal results. The generator serializes
    # them; it does not derive stores, pointer steps, counts, or flags.
    cases_by_width = {
        "word": [
            ("zero", 0, 0x0000800000000000, False, 16, DATA, DATA, "stop_address", 0, 0x0000800000000000, 1, None),
            ("df0", 3, 0x00200002, False, 16, DATA, "00118877887788778899aabbccddeeff", "stop_address", 0, 0x00200008, 1, None),
            ("df1", 3, 0x00200004, True, 16, DATA, "88778877887766778899aabbccddeeff", "stop_address", 0, 0x001ffffe, 1, None),
            ("unmapped_first", 1, 0x00200010, False, 16, DATA, DATA, "harness_error", 1, 0x00200010, 1, None),
            ("unmapped_after_one", 2, 0x00200000, False, 2, "0000", "8877", "harness_error", 1, 0x00200002, 1, {"access": "write", "address": hx(0x00200002)}),
            ("noncanonical", 1, 0x0000800000000000, False, 16, DATA, DATA, "exception", 1, 0x0000800000000000, 1, {"vector": 13, "error_code": 0, "linear_address": None}),
            ("budget_zero", 1, 0x00200000, False, 16, DATA, DATA, "instruction_budget_exhausted", 1, 0x00200000, 0, None),
            ("limit_1025", 1025, 0x00200000, False, 2048, "8877" * 1024, "8877" * 1024, "rep_iteration_budget_exhausted", 1, 0x00200800, 1, None),
        ],
        "dword": [
            ("zero", 0, 0x0000800000000000, False, 16, DATA, DATA, "stop_address", 0, 0x0000800000000000, 1, None),
            ("df0", 3, 0x00200002, False, 16, DATA, "0011887766558877665588776655eeff", "stop_address", 0, 0x0020000e, 1, None),
            ("df1", 3, 0x00200008, True, 16, DATA, "887766558877665588776655ccddeeff", "stop_address", 0, 0x001ffffc, 1, None),
            ("unmapped_first", 1, 0x00200010, False, 16, DATA, DATA, "harness_error", 1, 0x00200010, 1, None),
            ("unmapped_after_one", 2, 0x00200000, False, 4, "00000000", "88776655", "harness_error", 1, 0x00200004, 1, {"access": "write", "address": hx(0x00200004)}),
            ("noncanonical", 1, 0x0000800000000000, False, 16, DATA, DATA, "exception", 1, 0x0000800000000000, 1, {"vector": 13, "error_code": 0, "linear_address": None}),
            ("budget_zero", 1, 0x00200000, False, 16, DATA, DATA, "instruction_budget_exhausted", 1, 0x00200000, 0, None),
            ("limit_1025", 1025, 0x00200000, False, 4096, "88776655" * 1024, "88776655" * 1024, "rep_iteration_budget_exhausted", 1, 0x00201000, 1, None),
        ],
        "qword": [
            ("zero", 0, 0x0000800000000000, False, 16, DATA, DATA, "stop_address", 0, 0x0000800000000000, 1, None),
            ("df0", 3, 0x00200002, False, 26, DATA + "00" * 10, "0011887766554433221188776655443322118877665544332211", "stop_address", 0, 0x0020001a, 1, None),
            ("df1", 3, 0x00200010, True, 40, DATA + "00" * 24, "887766554433221188776655443322118877665544332211" + "00" * 16, "stop_address", 0, 0x001ffff8, 1, None),
            ("unmapped_first", 1, 0x00200010, False, 16, DATA, DATA, "harness_error", 1, 0x00200010, 1, None),
            ("unmapped_after_one", 2, 0x00200000, False, 8, "0000000000000000", "8877665544332211", "harness_error", 1, 0x00200008, 1, {"access": "write", "address": hx(0x00200008)}),
            ("noncanonical", 1, 0x0000800000000000, False, 16, DATA, DATA, "exception", 1, 0x0000800000000000, 1, {"vector": 13, "error_code": 0, "linear_address": None}),
            ("budget_zero", 1, 0x00200000, False, 16, DATA, DATA, "instruction_budget_exhausted", 1, 0x00200000, 0, None),
            ("limit_1025", 1025, 0x00200000, False, 8192, "8877665544332211" * 1024, "8877665544332211" * 1024, "rep_iteration_budget_exhausted", 1, 0x00202000, 1, None),
        ],
    }
    for label, width, code in WIDTHS:
        for suffix, count, rdi, df, data_len, data_in, data_out, outcome, out_count, out_rdi, budget, fault in cases_by_width[label]:
            add(f"s2_19b_rep_stos_{label}_{suffix}", width, code, count=count,
                rdi=rdi, df=df, data_len=data_len, data_in=data_in, data_out=data_out,
                outcome=outcome, out_count=out_count, out_rdi=out_rdi, budget=budget,
                fault=fault)

    # Two separate prefix-normalization fixtures exercise a misplaced REX and
    # the effective REX.W overriding operand-size 66.
    for case_id, code, width, rdi, final_rdi, data_out in (
        ("s2_19b_rep_stos_normalize_misplaced_rex", bytes.fromhex("f34866ab"), 2, 0x00200000, 0x00200002, "88772233445566778899aabbccddeeff"),
        ("s2_19b_rep_stos_normalize_rexw", bytes.fromhex("f36648ab"), 8, 0x00200000, 0x00200008, "88776655443322118899aabbccddeeff"),
    ):
        add(case_id, width, code, count=1, rdi=rdi, df=False, data_len=16,
            data_in=DATA, data_out=data_out, outcome="stop_address", out_count=0,
            out_rdi=final_rdi)

    # TEST establishes undefined AF, then REP STOSD succeeds or stops after one
    # committed iteration. Definedness is observed within the same run only.
    for suffix, count, partial, length, data_input, data_output, outcome, out_count, out_rdi, out_rip in (
        ("success", 1, False, 16, DATA, "88776655445566778899aabbccddeeff", "stop_address", 0, 0x00200004, None),
        ("partial", 3, True, 4, "00112233", "88776655", "harness_error", 2, 0x00200004, 0x00100003),
    ):
        code = bytes.fromhex("4885c0f3ab")
        case_id = f"s2_19b_test_rep_stos_{suffix}"
        add(case_id, 4, code, count=count, rdi=0x00200000, df=False,
            data_len=length, data_in=data_input, data_out=data_output,
            outcome=outcome,
            budget=2,
            out_count=out_count, out_rdi=out_rdi,
            fault={"access": "write", "address": hx(0x00200004)} if partial else None,
            out_flags=0x16, out_defined=0x0CC7,
            out_rip=out_rip)

    files["s2_19b_rep_stos_wdq_expected.json"] = json_bytes(expected)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    mismatches = []
    generated = materialize()
    for name, contents in generated.items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                mismatches.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(contents)
    if mismatches:
        print("stale generated files: " + ", ".join(mismatches))
        return 1
    print("S2-19b REP STOSW/D/Q fixtures are fresh" if args.check else f"wrote {len(generated) - 1} S2-19b fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
