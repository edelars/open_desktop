#!/usr/bin/env python3
"""Serialize hand-authored S2-19d REP MOVS fixtures; no execution is performed."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE = [0x1122334455667788, 0, 0xDEADBEEFFEDCBA98, 0x200000, 0x300080,
        0x300080, 0x200009, 0x200002, 0x0123456789ABCDEF, 0xFEDCBA9876543210,
        0x8877665544332211, 0xAABBCCDDEEFF0011, 0x1122112211221122,
        0x3344334433443344, 0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
WIDTHS = {
    "byte": (1, bytes.fromhex("f3a4")),
    "word": (2, bytes.fromhex("f366a5")),
    "dword": (4, bytes.fromhex("f3a5")),
    "qword": (8, bytes.fromhex("f348a5")),
}
PAYLOAD = bytes.fromhex("0102030405060708090a0b0c0d0e0f101112131415161718")
DIR_ROWS = {
    ("byte", False): (0x200000, 0x300000, 0x200003, 0x300003),
    ("byte", True):  (0x200002, 0x300002, 0x1fffff, 0x2fffff),
    ("word", False): (0x200000, 0x300000, 0x200006, 0x300006),
    ("word", True):  (0x200004, 0x300004, 0x1ffffe, 0x2ffffe),
    ("dword", False): (0x200000, 0x300000, 0x20000c, 0x30000c),
    ("dword", True):  (0x200008, 0x300008, 0x1ffffc, 0x2ffffc),
    ("qword", False): (0x200000, 0x300000, 0x200018, 0x300018),
    ("qword", True):  (0x200010, 0x300010, 0x1ffff8, 0x2ffff8),
}
FINAL_DESTINATIONS = {
    "byte": bytes.fromhex("010203") + b"\xa5" * 21,
    "word": bytes.fromhex("010203040506") + b"\xa5" * 18,
    "dword": bytes.fromhex("0102030405060708090a0b0c") + b"\xa5" * 12,
    "qword": bytes.fromhex("0102030405060708090a0b0c0d0e0f101112131415161718"),
}
FIRST_DESTINATIONS = {
    "byte": bytes.fromhex("01"), "word": bytes.fromhex("0102"),
    "dword": bytes.fromhex("01020304"), "qword": bytes.fromhex("0102030405060708"),
}
LIMIT_POINTERS = {
    "byte": (0x200400, 0x300400), "word": (0x200800, 0x300800),
    "dword": (0x201000, 0x301000), "qword": (0x202000, 0x302000),
}
OVERLAPS = {
    ("byte", False): (0x200018, 0x200019, 0x20001B, 0x20001C, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f3f3f3f73808d9aa7b4c1cedbe8f5020f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a"),
    ("byte", True):  (0x20001A, 0x20001B, 0x200017, 0x200018, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f3f4c5973808d9aa7b4c1cedbe8f5020f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a"),
    ("word", False): (0x200018, 0x20001A, 0x20001E, 0x200020, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c3f4c3f4c3f4ca7b4c1cedbe8f5020f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a"),
    ("word", True):  (0x20001C, 0x20001E, 0x200016, 0x200018, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c3f4c59667380a7b4c1cedbe8f5020f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a"),
    ("dword", False): (0x200018, 0x20001C, 0x200024, 0x200028, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c59663f4c59663f4c59663f4c59660f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a"),
    ("dword", True):  (0x200020, 0x200024, 0x200014, 0x200018, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c59663f4c596673808d9aa7b4c1ce0f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a"),
    ("qword", False): (0x200018, 0x200020, 0x200030, 0x200038, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c596673808d9a3f4c596673808d9a3f4c596673808d9a3f4c596673808d9adfecf90613202d3a"),
    ("qword", True):  (0x200028, 0x200030, 0x200010, 0x200018, "0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c596673808d9a3f4c596673808d9aa7b4c1cedbe8f5020f1c293643505d6adfecf90613202d3a"),
}


def hx(value: int) -> str:
    return f"0x{value & ((1 << 64) - 1):016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    def add(case_id: str, *, code: bytes, count: int, src: int, dst: int,
            source: bytes, destination: bytes, width: int, outcome: str,
            out_count: int, out_src: int, out_dst: int, budget: int = 1,
            df: bool = False, source_perm: tuple[bool, bool] = (True, False),
            dest_perm: tuple[bool, bool] = (True, True), rip: int | None = None,
            flags: int | None = None, defined: int = 0x0CD7,
            fault: dict | None = None, shared: bytes | None = None,
            shared_initial: bytes | None = None, destination_initial: bytes | None = None,
            initial_rax: int | None = None,
            shared_base: int = 0x00200000) -> None:
        gpr = BASE.copy()
        if initial_rax is not None:
            gpr[0] = initial_rax
        gpr[1], gpr[6], gpr[7] = count, src, dst
        rflags = (0x0CD7 if df else 0x08D7) if flags is None else flags
        final = gpr.copy()
        final[1], final[6], final[7] = out_count, out_src, out_dst
        code_region = {
            "region_id": "code", "guest_address": hx(0x100000), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }
        if shared is None:
            regions = [
                {"region_id": "code", "guest_address": hx(0x100000), "length": len(code),
                 "permissions": {"read": False, "write": False, "execute": True},
                 "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)},
                {"region_id": "source", "guest_address": hx(0x200000), "length": len(source),
                 "permissions": {"read": source_perm[0], "write": source_perm[1], "execute": False},
                 "initial_bytes_hex": source.hex(), "comparison_mask_hex": "ff" * len(source)},
                {"region_id": "destination", "guest_address": hx(0x300000), "length": len(destination),
                 "permissions": {"read": dest_perm[0], "write": dest_perm[1], "execute": False},
                 "initial_bytes_hex": (destination_initial if destination_initial is not None else b"\xa5" * len(destination)).hex(),
                 "comparison_mask_hex": "ff" * len(destination)},
            ]
            out_memory = {"code": code.hex(), "source": source.hex(), "destination": destination.hex()}
        else:
            regions = [code_region, {"region_id": "data", "guest_address": hx(shared_base), "length": len(shared),
                       "permissions": {"read": True, "write": True, "execute": False},
                       "initial_bytes_hex": (shared_initial if shared_initial is not None else shared).hex(), "comparison_mask_hex": "ff" * len(shared)}]
            out_memory = {"code": code.hex(), "data": shared.hex()}
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten REP MOVS bytes and literal state/memory answers; tests/cpu/generate_stage2_rep_movs.py serializes constants only. No assembler or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B MOVS and REP entries: each transfer reads DS:[RSI], writes ES:[RDI], and uses DF to update both indices.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19d REP MOVSB/W/D/Q",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gpr], "rip": hx(0x100000), "rflags": hx(rflags)},
            "memory": regions, "stop_rip": hx(0x100000 + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7),
            "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answer: dict[str, object] = {"gpr": [hx(x) for x in final], "rflags": hx(rflags),
            "rflags_defined": hx(defined), "rip": hx(rip if rip is not None else (0x100000 + len(code) if outcome == "stop_address" else 0x100000)),
            "memory": out_memory}
        if fault is not None:
            answer["fault"] = fault
        answers[case_id] = answer

    # Eleven literal scenarios per width.
    for label, (width, code) in WIDTHS.items():
        payload = PAYLOAD
        base_src, base_dst = 0x200000, 0x300000
        add(f"s2_19d_rep_movs_{label}_zero_invalid", code=code, width=width, count=0,
            src=0x0000800000000000, dst=0xffff800000000000, source=b"\0", destination=b"\0",
            destination_initial=b"\0",
            outcome="stop_address", out_count=0, out_src=0x0000800000000000, out_dst=0xffff800000000000)
        for df in (False, True):
            start_src, start_dst, out_src, out_dst = DIR_ROWS[(label, df)]
            add(f"s2_19d_rep_movs_{label}_df{int(df)}", code=code, width=width, count=3,
                src=start_src, dst=start_dst, source=payload, destination=FINAL_DESTINATIONS[label],
                outcome="stop_address", out_count=0, out_src=out_src, out_dst=out_dst, df=df)
        # First and after-one source/destination failures.
        add(f"s2_19d_rep_movs_{label}_source_fault_first", code=code, width=width, count=1,
            src=0x200040, dst=base_dst, source=payload, destination=b"\xa5" * 24,
            outcome="harness_error", out_count=1, out_src=0x200040, out_dst=base_dst,
            fault={"access": "read", "address": hx(0x200040)})
        next_src, next_dst = {"byte":(0x200001,0x300001), "word":(0x200002,0x300002),
                              "dword":(0x200004,0x300004), "qword":(0x200008,0x300008)}[label]
        add(f"s2_19d_rep_movs_{label}_source_fault_after_one", code=code, width=width, count=2,
            src=base_src, dst=base_dst, source=payload[:width], destination=FIRST_DESTINATIONS[label],
            outcome="harness_error", out_count=1, out_src=next_src, out_dst=next_dst,
            fault={"access": "read", "address": hx(next_src)})
        add(f"s2_19d_rep_movs_{label}_destination_fault_first", code=code, width=width, count=1,
            src=base_src, dst=base_dst, source=payload, destination=b"\xa5" * 24,
            outcome="harness_error", out_count=1, out_src=base_src, out_dst=base_dst,
            dest_perm=(True, False), fault={"access": "write", "address": hx(base_dst)})
        add(f"s2_19d_rep_movs_{label}_destination_fault_after_one", code=code, width=width, count=2,
            src=base_src, dst=base_dst, source=payload, destination=FIRST_DESTINATIONS[label],
            outcome="harness_error", out_count=1, out_src=next_src, out_dst=next_dst,
            fault={"access": "write", "address": hx(next_dst)})
        add(f"s2_19d_rep_movs_{label}_noncanonical_source", code=code, width=width, count=1,
            src=0x0000800000000000, dst=base_dst, source=payload, destination=b"\xa5" * 24,
            outcome="exception", out_count=1, out_src=0x0000800000000000, out_dst=base_dst,
            fault={"vector": 13, "error_code": 0, "linear_address": None})
        add(f"s2_19d_rep_movs_{label}_noncanonical_destination", code=code, width=width, count=1,
            src=base_src, dst=0x0000800000000000, source=payload, destination=b"\xa5" * 24,
            outcome="exception", out_count=1, out_src=base_src, out_dst=0x0000800000000000,
            fault={"vector": 13, "error_code": 0, "linear_address": None})
        add(f"s2_19d_rep_movs_{label}_budget_zero", code=code, width=width, count=1,
            src=base_src, dst=base_dst, source=payload, destination=b"\xa5" * 24,
            outcome="instruction_budget_exhausted", out_count=1, out_src=base_src, out_dst=base_dst,
            budget=0)
        limit_src, limit_dst = LIMIT_POINTERS[label]
        add(f"s2_19d_rep_movs_{label}_limit_1025", code=code, width=width, count=1025,
            src=base_src, dst=base_dst, source=b"\x3c" * (width * 1024), destination=b"\x3c" * (width * 1024),
            outcome="rep_iteration_budget_exhausted", out_count=1, out_src=limit_src,
            out_dst=limit_dst, flags=0x08D7, rip=0x100000)

    # Eight overlap images and all four pointers are literal expected values.
    overlap_initial = bytes.fromhex("0714212e3b4855626f7c8996a3b0bdcad7e4f1fe0b1825323f4c596673808d9aa7b4c1cedbe8f5020f1c293643505d6a7784919eabb8c5d2dfecf90613202d3a")
    for (label, df), (src, dst, out_src, out_dst, final_hex) in OVERLAPS.items():
        width, code = WIDTHS[label]
        add(f"s2_19d_rep_movs_{label}_overlap_df{int(df)}", code=code, width=width, count=3,
            src=src, dst=dst, source=b"", destination=b"", outcome="stop_address",
            out_count=0, out_src=out_src, out_dst=out_dst, df=df,
            shared=bytes.fromhex(final_hex), shared_initial=overlap_initial)

    for case_id, code, width, final_destination, out_src, out_dst in (
        ("s2_19d_rep_movs_normalize_misplaced_rex", bytes.fromhex("f34866a5"), 2,
         bytes.fromhex("0102") + b"\xa5" * 22, 0x200002, 0x300002),
        ("s2_19d_rep_movs_normalize_rexw", bytes.fromhex("f36648a5"), 8,
         bytes.fromhex("0102030405060708") + b"\xa5" * 16, 0x200008, 0x300008),
    ):
        add(case_id, code=code, width=width, count=1, src=0x200000, dst=0x300000,
            source=PAYLOAD, destination=final_destination, outcome="stop_address", out_count=0,
            out_src=out_src, out_dst=out_dst)

    # TEST is retired before the REP instruction, which remains at RIP on the partial fault.
    code = bytes.fromhex("4885c0f3a5")
    payload = bytes.fromhex("0102030405060708090a0b0c")
    for suffix, count, outcome, out_count, out_src, out_dst, destination, rip, fault in (
        ("success", 1, "stop_address", 0, 0x200004, 0x300004, bytes.fromhex("01020304"), None, None),
        ("destination_fault", 3, "harness_error", 2, 0x200004, 0x300004, bytes.fromhex("01020304"), 0x100003, {"access":"write", "address":hx(0x300004)}),
    ):
        add(f"s2_19d_test_rep_movs_{suffix}", code=code, width=4, count=count, src=0x200000,
            dst=0x300000, source=payload, destination=destination, outcome=outcome,
            out_count=out_count, out_src=out_src, out_dst=out_dst, budget=2,
            flags=0x46, defined=0x0CC7, rip=rip, fault=fault, initial_rax=0)

    assert len(answers) == 56, len(answers)
    files["s2_19d_rep_movs_expected.json"] = dump(answers)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    generated = materialize()
    stale = []
    for name, contents in generated.items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                stale.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(contents)
    if stale:
        print("stale generated files: " + ", ".join(stale))
        return 1
    print("S2-19d REP MOVS fixtures are fresh" if args.check else f"wrote {len(generated) - 1} S2-19d fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
