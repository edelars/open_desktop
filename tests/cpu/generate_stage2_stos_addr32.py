#!/usr/bin/env python3
"""Serialize literal S2-18l address-size-32 STOS fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
INITIAL = "00112233445566778899aabbccddeeff"
BASE_GPRS = [
    0x1122334455667788, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
    0x0000000000200000, 0x0000000000300080, 0x0000000000300080,
    0x0000000000200009, 0x0000000000200003, 0x0123456789ABCDEF,
    0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]

# id, bytes, initial RDI, expected RDI, initial flags, final flags,
# final defined flags, exit, retired, data base, initial data, final data,
# data permissions, budget. Every expected RDI and byte string is hand-written.
CASES = [
    ("s2_18l_stosb_df0", "67aa", 0x1234567800200004, 0x0000000000200005, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "00112233885566778899aabbccddeeff", "rw", 1),
    ("s2_18l_stosb_df1", "67aa", 0xFFFFFFFF00200008, 0x0000000000200007, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "00112233445566778899aabbccddeeff", "rw", 1),
    ("s2_18l_stosw_df0", "6766ab", 0x1234567800200004, 0x0000000000200006, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "00112233887766778899aabbccddeeff", "rw", 1),
    ("s2_18l_stosw_df1", "6766ab", 0xFFFFFFFF00200008, 0x0000000000200006, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "00112233445566778877aabbccddeeff", "rw", 1),
    ("s2_18l_stosd_df0", "67ab", 0x1234567800200004, 0x0000000000200008, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "00112233887766558899aabbccddeeff", "rw", 1),
    ("s2_18l_stosd_df1", "67ab", 0xFFFFFFFF00200008, 0x0000000000200004, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "001122334455667788776655ccddeeff", "rw", 1),
    ("s2_18l_stosq_df0", "6748ab", 0x1234567800200004, 0x000000000020000C, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "001122338877665544332211ccddeeff", "rw", 1),
    ("s2_18l_stosq_df1", "6748ab", 0xFFFFFFFF00200008, 0x0000000000200000, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "00112233445566778877665544332211", "rw", 1),
    ("s2_18l_unmapped_b", "67aa", 0x0000000000300000, 0x0000000000300000, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "rw", 1),
    ("s2_18l_unmapped_w", "6766ab", 0x0000000000300000, 0x0000000000300000, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "rw", 1),
    ("s2_18l_unmapped_d", "67ab", 0x0000000000300000, 0x0000000000300000, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "rw", 1),
    ("s2_18l_unmapped_q", "6748ab", 0x0000000000300000, 0x0000000000300000, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "rw", 1),
    ("s2_18l_readonly_b", "67aa", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "ro", 1),
    ("s2_18l_readonly_w", "6766ab", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "ro", 1),
    ("s2_18l_readonly_d", "67ab", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "ro", 1),
    ("s2_18l_readonly_q", "6748ab", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "harness_error", 0, 0x00200000, INITIAL, INITIAL, "ro", 1),
    ("s2_18l_budget0_b", "67aa", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "instruction_budget_exhausted", 0, 0x00200000, INITIAL, INITIAL, "rw", 0),
    ("s2_18l_budget0_w", "6766ab", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "instruction_budget_exhausted", 0, 0x00200000, INITIAL, INITIAL, "rw", 0),
    ("s2_18l_budget0_d", "67ab", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "instruction_budget_exhausted", 0, 0x00200000, INITIAL, INITIAL, "rw", 0),
    ("s2_18l_budget0_q", "6748ab", 0x00200004, 0x00200004, 0x08D7, 0x08D7, 0x0CD7, "instruction_budget_exhausted", 0, 0x00200000, INITIAL, INITIAL, "rw", 0),
    ("s2_18l_wrap_b_up", "67aa", 0xFFFFFFFF, 0x0000000000000000, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0xFFFFFFF8, "0011223344556677", "0011223344556688", "rw", 1),
    ("s2_18l_wrap_b_down", "67aa", 0x0000000000000000, 0x00000000FFFFFFFF, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00000000, INITIAL, "88112233445566778899aabbccddeeff", "rw", 1),
    ("s2_18l_wrap_w_up", "6766ab", 0xFFFFFFFE, 0x0000000000000000, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0xFFFFFFF8, "0011223344556677", "0011223344558877", "rw", 1),
    ("s2_18l_wrap_w_down", "6766ab", 0x0000000000000001, 0x00000000FFFFFFFF, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00000000, INITIAL, "00887733445566778899aabbccddeeff", "rw", 1),
    ("s2_18l_wrap_d_up", "67ab", 0xFFFFFFFC, 0x0000000000000000, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0xFFFFFFF8, "0011223344556677", "0011223388776655", "rw", 1),
    ("s2_18l_wrap_d_down", "67ab", 0x0000000000000003, 0x00000000FFFFFFFF, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00000000, INITIAL, "00112288776655778899aabbccddeeff", "rw", 1),
    ("s2_18l_wrap_q_up", "6748ab", 0xFFFFFFF8, 0x0000000000000000, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0xFFFFFFF8, "0011223344556677", "8877665544332211", "rw", 1),
    ("s2_18l_wrap_q_down", "6748ab", 0x0000000000000007, 0x00000000FFFFFFFF, 0x0CD7, 0x0CD7, 0x0CD7, "stop_address", 1, 0x00000000, INITIAL, "001122334455668877665544332211ff", "rw", 1),
    ("s2_18l_cross_w", "6766ab", 0xFFFFFFFF, 0x0000000000000001, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0xFFFFFFF8, INITIAL, "00112233445566887799aabbccddeeff", "rw", 1),
    ("s2_18l_cross_d", "67ab", 0xFFFFFFFF, 0x0000000000000003, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0xFFFFFFF8, INITIAL, "0011223344556688776655bbccddeeff", "rw", 1),
    ("s2_18l_cross_q", "6748ab", 0xFFFFFFFF, 0x0000000000000007, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0xFFFFFFF8, INITIAL, "001122334455668877665544332211ff", "rw", 1),
    ("s2_18l_normalize_word", "674866ab", 0x1234567800200004, 0x0000000000200006, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "00112233887766778899aabbccddeeff", "rw", 1),
    ("s2_18l_normalize_qword", "676648ab", 0x1234567800200004, 0x000000000020000C, 0x08D7, 0x08D7, 0x0CD7, "stop_address", 1, 0x00200000, INITIAL, "001122338877665544332211ccddeeff", "rw", 1),
    ("s2_18l_test_dword", "4885c067ab", 0x1234567800200004, 0x0000000000200008, 0x08D7, 0x0016, 0x0CC7, "stop_address", 2, 0x00200000, INITIAL, "00112233887766558899aabbccddeeff", "rw", 2),
    ("s2_18l_test_qword", "4885c06748ab", 0x1234567800200004, 0x000000000020000C, 0x08D7, 0x0016, 0x0CC7, "stop_address", 2, 0x00200000, INITIAL, "001122338877665544332211ccddeeff", "rw", 2),
    ("s2_18l_cld_b", "fc67aa", 0xFFFFFFFF00200008, 0x0000000000200009, 0x0CD7, 0x08D7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "00112233445566778899aabbccddeeff", "rw", 2),
    ("s2_18l_std_b", "fd67aa", 0x1234567800200004, 0x0000000000200003, 0x08D7, 0x0CD7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "00112233885566778899aabbccddeeff", "rw", 2),
    ("s2_18l_cld_w", "fc6766ab", 0xFFFFFFFF00200008, 0x000000000020000A, 0x0CD7, 0x08D7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "00112233445566778877aabbccddeeff", "rw", 2),
    ("s2_18l_std_w", "fd6766ab", 0x1234567800200006, 0x0000000000200004, 0x08D7, 0x0CD7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "00112233445588778899aabbccddeeff", "rw", 2),
    ("s2_18l_cld_d", "fc67ab", 0xFFFFFFFF00200008, 0x000000000020000C, 0x0CD7, 0x08D7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "001122334455667788776655ccddeeff", "rw", 2),
    ("s2_18l_std_d", "fd67ab", 0x1234567800200008, 0x0000000000200004, 0x08D7, 0x0CD7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "001122334455667788776655ccddeeff", "rw", 2),
    ("s2_18l_cld_q", "fc6748ab", 0xFFFFFFFF00200008, 0x0000000000200010, 0x0CD7, 0x08D7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "00112233445566778877665544332211", "rw", 2),
    ("s2_18l_std_q", "fd6748ab", 0x1234567800200008, 0x0000000000200000, 0x08D7, 0x0CD7, 0x0CD7, "stop_address", 2, 0x00200000, INITIAL, "00112233445566778877665544332211", "rw", 2),
]

# Literal backend error expectations: case id -> (typed error, guest address,
# access kind). The guest address is the zero-extended EDI used for the write.
HARNESS_ERRORS = {
    "s2_18l_unmapped_b": ("Unmapped", 0x00300000, "Write"),
    "s2_18l_unmapped_w": ("Unmapped", 0x00300000, "Write"),
    "s2_18l_unmapped_d": ("Unmapped", 0x00300000, "Write"),
    "s2_18l_unmapped_q": ("Unmapped", 0x00300000, "Write"),
    "s2_18l_readonly_b": ("PermissionDenied", 0x00200004, "Write"),
    "s2_18l_readonly_w": ("PermissionDenied", 0x00200004, "Write"),
    "s2_18l_readonly_d": ("PermissionDenied", 0x00200004, "Write"),
    "s2_18l_readonly_q": ("PermissionDenied", 0x00200004, "Write"),
}


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    assert len(CASES) == 43
    generated: dict[str, bytes] = {}
    answers: dict[str, object] = {}
    manifest: dict[str, str] = {}
    for (case_id, code_hex, in_rdi, out_rdi, in_flags, flags, defined, outcome,
         retired, data_base, data_before, data_after, permission, budget) in CASES:
        assert len(data_before) == len(data_after) and len(data_before) % 2 == 0
        code = bytes.fromhex(code_hex)
        before = BASE_GPRS.copy()
        before[0] = 0x1122334455667788
        before[7] = in_rdi
        after = BASE_GPRS.copy()
        after[0] = 0x1122334455667788
        after[7] = out_rdi
        regions = [
            {"region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code), "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code_hex, "comparison_mask_hex": "ff" * len(code)},
            {"region_id": "data", "guest_address": hx(data_base), "length": len(data_before) // 2, "permissions": {"read": True, "write": permission == "rw", "execute": False}, "initial_bytes_hex": data_before, "comparison_mask_hex": "ff" * (len(data_before) // 2)},
        ]
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Hand-authored encoding, register snapshots, and memory bytes; generator serializes literal rows only.", "specification": "Intel SDM Vol.2B STOS; address-size override in Long64 and DF-controlled pointer stride. ISSUE-016 specifies the local all-or-error memory-write policy.", "specification_version": "Intel SDM Vol.2B revision 093; project S2-18l local model policy"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(v) for v in before], "rip": hx(CODE_BASE), "rflags": hx(in_flags)},
            "memory": regions,
            "stop_rip": hx(CODE_BASE + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        generated[f"{case_id}.case.json"] = dump(fixture)
        answer = {"gpr": [hx(v) for v in after], "rflags": hx(flags), "rflags_defined": hx(defined), "rip": hx(CODE_BASE + len(code) if outcome == "stop_address" else CODE_BASE), "memory": {"code": code_hex, "data": data_after}, "retired": retired}
        if case_id in HARNESS_ERRORS:
            kind, address, access = HARNESS_ERRORS[case_id]
            answer["harness_error"] = {"kind": kind, "address": hx(address), "access": access}
        answers[case_id] = answer
        manifest[f"{case_id}.case.json"] = hashlib.sha256(generated[f"{case_id}.case.json"]).hexdigest()
    generated["s2_18l_stos_addr32_expected.json"] = dump(answers)
    generated["s2_18l_stos_addr32_manifest.json"] = dump({"fixture_ids": sorted(answers), "fixture_sha256": manifest, "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()})
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
    print("S2-18l fixtures are fresh" if args.check else "wrote 45 S2-18l fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
