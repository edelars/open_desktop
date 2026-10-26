#!/usr/bin/env python3
"""Serialize the literal S2-18g STOSW/D/Q fixture inputs and answers."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
INITIAL_DATA = "00112233445566778899aabbccddeeff"
BASE_GPRS = [
    0x1122334455667788, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
    0x0000000000200000, 0x0000000000300080, 0x0000000000300080,
    0x0000000000200009, 0x0000000000200003, 0x0123456789ABCDEF,
    0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]

# id, bytes, initial RDI, flags, exit, expected RDI, data permission, expected
# data image, exception vector. Expected state/memory values are hand-authored.
CASES = (
    ("s2_18g_stosw_df0", "66ab", 0x00200004, 0x08D7, "stop_address", 0x00200006, "rw", "00112233110066778899aabbccddeeff", None),
    ("s2_18g_stosw_df1", "66ab", 0x00200006, 0x0CD7, "stop_address", 0x00200004, "rw", "00112233445511008899aabbccddeeff", None),
    ("s2_18g_stosd_df0", "ab", 0x00200004, 0x08D7, "stop_address", 0x00200008, "rw", "001122331100ffee8899aabbccddeeff", None),
    ("s2_18g_stosd_df1", "ab", 0x00200008, 0x0CD7, "stop_address", 0x00200004, "rw", "00112233445566771100ffeeccddeeff", None),
    ("s2_18g_stosq_df0", "48ab", 0x00200004, 0x08D7, "stop_address", 0x0020000C, "rw", "001122331100ffeeddccbbaaccddeeff", None),
    ("s2_18g_stosq_df1", "48ab", 0x00200008, 0x0CD7, "stop_address", 0x00200000, "rw", "00112233445566771100ffeeddccbbaa", None),
    ("s2_18g_stos_unmapped_word", "66ab", 0x00201000, 0x08D7, "harness_error", 0x00201000, "rw", INITIAL_DATA, None),
    ("s2_18g_stos_unmapped_dword", "ab", 0x00201000, 0x08D7, "harness_error", 0x00201000, "rw", INITIAL_DATA, None),
    ("s2_18g_stos_unmapped_qword", "48ab", 0x00201000, 0x08D7, "harness_error", 0x00201000, "rw", INITIAL_DATA, None),
    ("s2_18g_stos_readonly_word", "66ab", 0x00200004, 0x08D7, "harness_error", 0x00200004, "ro", INITIAL_DATA, None),
    ("s2_18g_stos_readonly_dword", "ab", 0x00200004, 0x08D7, "harness_error", 0x00200004, "ro", INITIAL_DATA, None),
    ("s2_18g_stos_readonly_qword", "48ab", 0x00200004, 0x08D7, "harness_error", 0x00200004, "ro", INITIAL_DATA, None),
    ("s2_18g_stos_noncanonical_word", "66ab", 0x0000800000000000, 0x08D7, "exception", 0x0000800000000000, "rw", INITIAL_DATA, 13),
    ("s2_18g_stos_noncanonical_dword", "ab", 0x0000800000000000, 0x08D7, "exception", 0x0000800000000000, "rw", INITIAL_DATA, 13),
    ("s2_18g_stos_noncanonical_qword", "48ab", 0x0000800000000000, 0x08D7, "exception", 0x0000800000000000, "rw", INITIAL_DATA, 13),
    ("s2_18g_stos_budget_word", "66ab", 0x00200004, 0x08D7, "instruction_budget_exhausted", 0x00200004, "rw", INITIAL_DATA, None),
    ("s2_18g_stos_budget_dword", "ab", 0x00200004, 0x08D7, "instruction_budget_exhausted", 0x00200004, "rw", INITIAL_DATA, None),
    ("s2_18g_stos_budget_qword", "48ab", 0x00200004, 0x08D7, "instruction_budget_exhausted", 0x00200004, "rw", INITIAL_DATA, None),
    ("s2_18g_stos_prefix_4866ab", "4866ab", 0x00200004, 0x08D7, "stop_address", 0x00200006, "rw", "00112233110066778899aabbccddeeff", None),
    ("s2_18g_stos_prefix_6648ab", "6648ab", 0x00200004, 0x08D7, "stop_address", 0x0020000C, "rw", "001122331100ffeeddccbbaaccddeeff", None),
)


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    generated: dict[str, bytes] = {}
    answers: dict[str, object] = {}
    for case_id, code_hex, rdi, flags, outcome, out_rdi, permission, data, vector in CASES:
        code = bytes.fromhex(code_hex)
        before = BASE_GPRS.copy()
        before[0] = 0xAABBCCDDEEFF0011
        before[RegisterIndex.RDI] = rdi
        after = before.copy()
        after[RegisterIndex.RDI] = out_rdi
        code_length = len(code)
        budget = 0 if outcome == "instruction_budget_exhausted" else 1
        fixture = {
            "schema_version": 2,
            "case_id": case_id,
            "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten bytes and complete literal states/memory; tests/cpu/generate_stage2_stos_widths.py serializes these constants only.",
                "specification": "Intel SDM revision 093, Vol.2B STOS/STOSW/STOSD/STOSQ; ES is flat in 64-bit mode, DF controls the RDI stride, and REX.W overrides 66 for STOSQ.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-18g",
            },
            "seed": None,
            "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(value) for value in before], "rip": hx(CODE_BASE), "rflags": hx(flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(CODE_BASE), "length": code_length, "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code_hex, "comparison_mask_hex": "ff" * code_length},
                {"region_id": "data", "guest_address": hx(DATA_BASE), "length": 16, "permissions": {"read": True, "write": permission == "rw", "execute": False}, "initial_bytes_hex": INITIAL_DATA, "comparison_mask_hex": "ff" * 16},
            ],
            "stop_rip": hx(CODE_BASE + code_length) if outcome == "stop_address" else None,
            "instruction_budget": budget,
            "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        generated[f"{case_id}.case.json"] = dump(fixture)
        answer = {
            "gpr": [hx(value) for value in after],
            "rflags": hx(flags),
            "rflags_defined": hx(0x0CD7),
            "rip": hx(CODE_BASE + code_length if outcome == "stop_address" else CODE_BASE),
            "memory": {"code": code_hex, "data": data},
        }
        if vector is not None:
            answer["fault"] = {"vector": vector, "error_code": hx(0)}
        answers[case_id] = answer
    generated["s2_18g_stos_widths_expected.json"] = dump(answers)
    return generated


class RegisterIndex:
    RDI = 7


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
    print("S2-18g STOS width fixtures are fresh" if args.check else "wrote 21 S2-18g fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
