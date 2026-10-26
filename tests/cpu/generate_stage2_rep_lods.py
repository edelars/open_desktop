#!/usr/bin/env python3
"""Serialize handwritten literal S2-19c REP LODS fixtures."""
from __future__ import annotations
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE = [0x1122334455667788, 0, 0xDEADBEEFFEDCBA98, 0x200000, 0x300080, 0x300080, 0x200009, 0x200002,
        0x0123456789ABCDEF, 0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
        0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
DATA = bytes.fromhex("0102030405060708090a0b0c0d0e0f101112131415161718")
WIDTHS = {
    "byte": (1, bytes.fromhex("f3ac"), 0x1122334455667703, 0x1122334455667701),
    "word": (2, bytes.fromhex("f366ad"), 0x1122334455660605, 0x1122334455660201),
    "dword": (4, bytes.fromhex("f3ad"), 0x0C0B0A09, 0x04030201),
    "qword": (8, bytes.fromhex("f348ad"), 0x1817161514131211, 0x0807060504030201),
}
def hx(v: int) -> str: return f"0x{v:016x}"
def dump(v: object) -> bytes: return (json.dumps(v, indent=2, ensure_ascii=False) + "\n").encode()

def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected: dict[str, object] = {}
    def add(cid: str, width: int, code: bytes, *, count: int, rsi: int, df: bool,
            data: bytes, outcome: str, out_count: int, out_rsi: int, out_rax: int,
            budget: int = 1, fault: dict | None = None,
            flags: int | None = None, defined: int = 0x0CD7, rip: int | None = None) -> None:
        gprs = BASE.copy(); gprs[1] = count; gprs[6] = rsi
        rflags = (0x0CD7 if df else 0x08D7) if flags is None else flags
        out = gprs.copy(); out[0] = out_rax; out[1] = out_count; out[6] = out_rsi
        stop_rip = 0x00100000 + len(code) if outcome == "stop_address" else None
        fixture = {
            "schema_version": 2, "case_id": cid, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten instruction bytes and predetermined complete state answers; tests/cpu/generate_stage2_rep_lods.py serializes constants only. No assembler or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B LODS and REP entries: F3 repeats the selected LODS width from DS:[RSI], DF selects index direction, and completed iterations record remaining RCX and next RSI.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19c REP LODS B/W/D/Q",
            }, "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gprs], "rip": hx(0x00100000), "rflags": hx(rflags)},
            "memory": [
                {"region_id": "code", "guest_address": hx(0x00100000), "length": len(code), "permissions": {"read": False, "write": False, "execute": True}, "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)},
                {"region_id": "data", "guest_address": hx(0x00200000), "length": len(data), "permissions": {"read": True, "write": False, "execute": False}, "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data)},
            ], "stop_rip": hx(stop_rip) if stop_rip is not None else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        files[f"{cid}.case.json"] = dump(fixture)
        ans = {"gpr": [hx(x) for x in out], "rflags": hx(rflags), "rflags_defined": hx(defined),
               "rip": hx(stop_rip if stop_rip is not None else (rip if rip is not None else 0x00100000)),
               "memory": {"code": code.hex(), "data": data.hex()}}
        if fault is not None: ans["fault"] = fault
        expected[cid] = ans

    for label, (width, code, rax0, rax1) in WIDTHS.items():
        count = width * 3
        # Fixed outcomes are literal, including state after partial read faults.
        rows = [
            ("zero", 0, 0x0000800000000000, False, DATA[:max(16,count)], "stop_address", 0, 0x0000800000000000, BASE[0], 1, None),
            ("df0", 3, 0x00200000, False, DATA[:max(16,count)], "stop_address", 0, 0x00200000 + count, rax0, 1, None),
            ("df1", 3, 0x00200000 + 2*width, True, DATA[:max(16,count)], "stop_address", 0, 0x00200000 - width, rax1, 1, None),
            ("unmapped_first", 1, 0x00200000 + max(16,count), False, DATA[:max(16,count)], "harness_error", 1, 0x00200000 + max(16,count), BASE[0], 1, None),
            ("unmapped_after_one", 2, 0x00200000, False, DATA[:width], "harness_error", 1, 0x00200000 + width, {1:0x1122334455667701, 2:0x1122334455660201, 4:0x0000000004030201, 8:0x0807060504030201}[width], 1, {"access":"read", "address":hx(0x00200000+width)}),
            ("noncanonical", 1, 0x0000800000000000, False, DATA[:max(16,count)], "exception", 1, 0x0000800000000000, BASE[0], 1, {"vector":13,"error_code":0,"linear_address":None}),
            ("budget_zero", 1, 0x00200000, False, DATA[:max(16,count)], "instruction_budget_exhausted", 1, 0x00200000, BASE[0], 0, None),
            ("limit_1025", 1025, 0x00200000, False, (DATA * ((1024*width + len(DATA)-1)//len(DATA)))[:1024*width], "rep_iteration_budget_exhausted", 1, 0x00200000+1024*width, {1:0x1122334455667710, 2:0x1122334455660807, 4:0x00000000100f0e0d, 8:0x0807060504030201}[width], 1, None),
        ]
        for suffix, n, rsi, df, data, outcome, out_count, out_rsi, out_rax, budget, fault in rows:
            add(f"s2_19c_rep_lods_{label}_{suffix}", width, code, count=n, rsi=rsi, df=df, data=data,
                outcome=outcome, out_count=out_count, out_rsi=out_rsi, out_rax=out_rax, budget=budget, fault=fault)

    # Prefix-order normalization: ignored misplaced REX before 66, and effective REX.W after 66.
    for cid, code, width, rsi, rax in (
        ("s2_19c_rep_lods_normalize_misplaced_rex", bytes.fromhex("f34866ad"), 2, 0x200000, 0x1122334455660201),
        ("s2_19c_rep_lods_normalize_rexw", bytes.fromhex("f36648ad"), 8, 0x200000, 0x0807060504030201),
    ):
        add(cid, width, code, count=1, rsi=rsi, df=False, data=DATA, outcome="stop_address", out_count=0, out_rsi=rsi+width, out_rax=rax)

    # TEST; REP LODSD proves definedness remains meaningful in the same run.
    code = bytes.fromhex("4885c0f3ad")
    for suffix, count, data, outcome, out_count, out_rsi, rax, rip in (
        ("success", 1, DATA, "stop_address", 0, 0x200004, 0x0000000004030201, None),
        ("partial", 3, DATA[:4], "harness_error", 2, 0x200004, 0x0000000004030201, 0x00100003),
    ):
        add(f"s2_19c_test_rep_lods_{suffix}", 4, code, count=count, rsi=0x200000, df=False, data=data,
            outcome=outcome, out_count=out_count, out_rsi=out_rsi, out_rax=rax, budget=2,
            flags=0x16, defined=0x0CC7, rip=rip,
            fault={"access":"read", "address":hx(0x200004)} if suffix == "partial" else None)
    files["s2_19c_rep_lods_expected.json"] = dump(expected)
    return files

def main() -> int:
    p=argparse.ArgumentParser(); p.add_argument("--check", action="store_true"); args=p.parse_args()
    mismatches=[]; generated=materialize()
    for name, data in generated.items():
        path=FIXTURES/name
        if args.check:
            if not path.exists() or path.read_bytes()!=data: mismatches.append(str(path.relative_to(ROOT)))
        else: path.write_bytes(data)
    if mismatches: print("stale generated files: "+", ".join(mismatches)); return 1
    print("S2-19c REP LODS fixtures are fresh" if args.check else f"wrote {len(generated)-1} S2-19c fixture files")
    return 0
if __name__ == "__main__": raise SystemExit(main())
