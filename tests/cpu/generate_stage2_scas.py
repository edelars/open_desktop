#!/usr/bin/env python3
"""Serialize the literal S2-18h SCAS fixture inputs and answers."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
RAX = 0x8877665544332211
BASE_GPRS = [
    RAX, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98, DATA_BASE,
    0x0000000000300080, 0x0000000000300080, 0x0000000000200009,
    0x0000000000200004, 0x0123456789ABCDEF, 0xFEDCBA9876543210,
    0x8877665544332211, 0xAABBCCDDEEFF0011, 0x1122112211221122,
    0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF,
]
INITIAL_DATA = "00112233445566778899aabbccddeeff"

# id, bytes, width, RDI, input flags, outcome, expected RDI, expected flags,
# definedness, data image, exception vector. Every expected value is literal.
CASES = [
    ("s2_18h_scasb_df0", "ae", 1, 0x00200004, 0x08D7, "stop_address", 0x00200005, 0x0046, 0x0CD7, "00112233114466778899aabbccddeeff", None),
    ("s2_18h_scasb_df1", "ae", 1, 0x00200004, 0x0CD7, "stop_address", 0x00200003, 0x0446, 0x0CD7, "00112233114466778899aabbccddeeff", None),
    ("s2_18h_scasw_df0", "66af", 2, 0x00200004, 0x08D7, "stop_address", 0x00200006, 0x0046, 0x0CD7, "00112233112266778899aabbccddeeff", None),
    ("s2_18h_scasw_df1", "66af", 2, 0x00200004, 0x0CD7, "stop_address", 0x00200002, 0x0446, 0x0CD7, "00112233112266778899aabbccddeeff", None),
    ("s2_18h_scasd_df0", "af", 4, 0x00200004, 0x08D7, "stop_address", 0x00200008, 0x0046, 0x0CD7, "00112233112233448899aabbccddeeff", None),
    ("s2_18h_scasd_df1", "af", 4, 0x00200004, 0x0CD7, "stop_address", 0x00200000, 0x0446, 0x0CD7, "00112233112233448899aabbccddeeff", None),
    ("s2_18h_scasq_df0", "48af", 8, 0x00200004, 0x08D7, "stop_address", 0x0020000C, 0x0046, 0x0CD7, "001122331122334455667788ccddeeff", None),
    ("s2_18h_scasq_df1", "48af", 8, 0x00200004, 0x0CD7, "stop_address", 0x001FFFFC, 0x0446, 0x0CD7, "001122331122334455667788ccddeeff", None),
    ("s2_18h_scas_unmapped_b", "ae", 1, 0x00201000, 0x08D7, "harness_error", 0x00201000, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_unmapped_w", "66af", 2, 0x00201000, 0x08D7, "harness_error", 0x00201000, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_unmapped_d", "af", 4, 0x00201000, 0x08D7, "harness_error", 0x00201000, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_unmapped_q", "48af", 8, 0x00201000, 0x08D7, "harness_error", 0x00201000, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_noncanonical_b", "ae", 1, 0x0000800000000000, 0x08D7, "exception", 0x0000800000000000, 0x08D7, 0x0CD7, INITIAL_DATA, 13),
    ("s2_18h_scas_noncanonical_w", "66af", 2, 0x0000800000000000, 0x08D7, "exception", 0x0000800000000000, 0x08D7, 0x0CD7, INITIAL_DATA, 13),
    ("s2_18h_scas_noncanonical_d", "af", 4, 0x0000800000000000, 0x08D7, "exception", 0x0000800000000000, 0x08D7, 0x0CD7, INITIAL_DATA, 13),
    ("s2_18h_scas_noncanonical_q", "48af", 8, 0x0000800000000000, 0x08D7, "exception", 0x0000800000000000, 0x08D7, 0x0CD7, INITIAL_DATA, 13),
    ("s2_18h_scas_budget_b", "ae", 1, 0x00200004, 0x08D7, "instruction_budget_exhausted", 0x00200004, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_budget_w", "66af", 2, 0x00200004, 0x08D7, "instruction_budget_exhausted", 0x00200004, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_budget_d", "af", 4, 0x00200004, 0x08D7, "instruction_budget_exhausted", 0x00200004, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_budget_q", "48af", 8, 0x00200004, 0x08D7, "instruction_budget_exhausted", 0x00200004, 0x08D7, 0x0CD7, INITIAL_DATA, None),
    ("s2_18h_scas_normalized_4866af", "4866af", 2, 0x00200004, 0x08D7, "stop_address", 0x00200006, 0x0046, 0x0CD7, "00112233112266778899aabbccddeeff", None),
    ("s2_18h_scas_normalized_6648af", "6648af", 8, 0x00200004, 0x08D7, "stop_address", 0x0020000C, 0x0046, 0x0CD7, "001122331122334455667788ccddeeff", None),
    ("s2_18h_scas_test_composition", "4885c0ae", 1, 0x00200004, 0x0002, "stop_address", 0x00200005, 0x0046, 0x0CD7, "00112233010066778899aabbccddeeff", None),
    ("s2_18h_scas_test_fault", "4885c0ae", 1, 0x0000800000000000, 0x0012, "exception", 0x0000800000000000, 0x0012, 0x0CC7, INITIAL_DATA, 13),
]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    assert len(CASES) == 24
    generated: dict[str, bytes] = {}
    answers: dict[str, object] = {}
    for case_id, code_hex, width, rdi, flags, outcome, after_rdi, after_flags, defined, data, vector in CASES:
        code = bytes.fromhex(code_hex)
        before = BASE_GPRS.copy()
        if "test_composition" in case_id or "test_fault" in case_id:
            before[0] = 1
        before[7] = rdi
        after = before.copy()
        after[7] = after_rdi
        rip_after = (
            CODE_BASE + len(code)
            if outcome == "stop_address"
            else CODE_BASE + 3 if "test_fault" in case_id else CODE_BASE
        )
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten SCAS bytes and literal full states/memory; tests/cpu/generate_stage2_scas.py serializes these constants only.",
                "specification": "Intel SDM revision 093, Vol.2B SCAS; accumulator is compared with flat ES:[RDI], and DF selects the index stride.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-18h",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(v) for v in before], "rip": hx(CODE_BASE), "rflags": hx(flags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code), "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code_hex, "comparison_mask_hex": "ff" * len(code)},
                {"region_id": "data", "guest_address": hx(DATA_BASE), "length": 16, "permissions": {"read": True, "write": True, "execute": False}, "initial_bytes_hex": data, "comparison_mask_hex": "ff" * 16},
            ],
            "stop_rip": hx(CODE_BASE + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": 0 if "budget" in case_id else (2 if "composition" in case_id or "test_fault" in case_id else 1),
            "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        generated[f"{case_id}.case.json"] = dump(fixture)
        answer = {"gpr": [hx(v) for v in after], "rflags": hx(after_flags), "rflags_defined": hx(defined), "rip": hx(rip_after), "memory": {"code": code_hex, "data": data}}
        if vector is not None:
            answer["fault"] = {"vector": vector, "error_code": hx(0)}
        answers[case_id] = answer
    generated["s2_18h_scas_expected.json"] = dump(answers)
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
    print("S2-18h SCAS fixtures are fresh" if args.check else "wrote 25 S2-18h fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
