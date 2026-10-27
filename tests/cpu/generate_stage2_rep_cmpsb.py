#!/usr/bin/env python3
"""Serialize hand-authored S2-19f REP CMPSB fixtures and literal answers."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE = 0x300000
RIP = 0x100000
CODE = bytes.fromhex("f3a6")
PF_ZF = 0x44
EQUAL = 0x46
MISMATCH = 0x97
BASE_GPR = [0x1122334455667741, 0, 0xDEADBEEFFEDCBA98, 0x200000,
            0x300080, 0x300080, BASE, BASE + 0x100, 0x0123456789ABCDEF,
            0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
            0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
            0xFFFFFFFFFFFFFFFF]


def hx(value: int) -> str:
    return f"0x{value & ((1 << 64) - 1):016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    def add(name: str, *, count: int, source: int, destination: int,
            regions: list[tuple[str, int, bytes]], initial_flags: int,
            final_count: int, final_source: int, final_destination: int,
            final_flags: int, outcome: str = "stop_address", code: bytes = CODE,
            rip: int | None = None, budget: int = 1, defined: int = 0x0CD7,
            fault: dict | None = None, initial_rax: int | None = None) -> None:
        gpr = BASE_GPR.copy()
        gpr[0] = BASE_GPR[0] if initial_rax is None else initial_rax
        gpr[1], gpr[6], gpr[7] = count, source, destination
        final = gpr.copy()
        final[1], final[6], final[7] = final_count, final_source, final_destination
        memory = [{
            "region_id": "code", "guest_address": hx(RIP), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }]
        for region_id, address, data in regions:
            if not data:
                continue
            memory.append({
                "region_id": region_id, "guest_address": hx(address), "length": len(data),
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data),
            })
        fixture = {
            "schema_version": 2, "case_id": name, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten F3 A6 bytes and literal state, flag, and memory answers; tests/cpu/generate_stage2_rep_cmpsb.py serializes constants only. No assembler, oracle, or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B CMPS and REP/REPE entries; CMPSB compares DS:[RSI] with ES:[RDI], adjusts both indices by DF, and REPE continues while ZF is set.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19f F3 A6 REP CMPSB",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gpr],
                              "rip": hx(RIP), "rflags": hx(initial_flags)},
            "memory": memory, "stop_rip": hx(RIP + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        files[f"{name}.case.json"] = dump(fixture)
        answer: dict[str, object] = {
            "gpr": [hx(x) for x in final], "rflags": hx(final_flags),
            "rflags_defined": hx(defined),
            "rip": hx(rip if rip is not None else
                      (RIP + len(code) if outcome == "stop_address" else RIP)),
            "memory": {region_id: data.hex() for region_id, _, data in
                       [("code", RIP, code), *regions] if data},
        }
        if fault is not None:
            answer["fault"] = fault
        answers[name] = answer

    # Zero count with invalid indices still fetches/decodes and retires normally.
    for df in (False, True):
        flags = 2 | PF_ZF | (0x400 if df else 0)
        add(f"s2_19f_rep_cmpsb_zero_df{int(df)}", count=0,
            source=0x0000800000000000, destination=0xffff800000000000,
            regions=[], initial_flags=flags, final_count=0,
            final_source=0x0000800000000000, final_destination=0xffff800000000000,
            final_flags=flags)

    for df in (False, True):
        step = -1 if df else 1
        src, dst = BASE + (2 if df else 0), BASE + 0x100 + (2 if df else 0)
        src_data, dst_data = bytearray(b"\x41" * 3), bytearray(b"\x41" * 3)
        for equal, label, value, flags in ((True, "equal", 0x41, EQUAL),
                                            (False, "mismatch", 0x42, MISMATCH)):
            d = bytearray(dst_data)
            d[2 if df else 0] = value
            add(f"s2_19f_rep_cmpsb_one_{label}_df{int(df)}", count=1,
                source=src, destination=dst,
                regions=[("source", BASE, bytes(src_data)), ("destination", BASE + 0x100, bytes(d))],
                initial_flags=2 | PF_ZF | (0x400 if df else 0), final_count=0,
                final_source=src + step, final_destination=dst + step,
                final_flags=flags | (0x400 if df else 0))

        for position in (1, 2, 3):
            d = bytearray(b"\x41" * 3)
            d[2 - (position - 1) if df else position - 1] = 0x42
            add(f"s2_19f_rep_cmpsb_mismatch_{position}_df{int(df)}", count=3,
                source=src, destination=dst,
                regions=[("source", BASE, b"\x41" * 3), ("destination", BASE + 0x100, bytes(d))],
                initial_flags=2 | PF_ZF | (0x400 if df else 0), final_count=3-position,
                final_source=src + step * position, final_destination=dst + step * position,
                final_flags=MISMATCH | (0x400 if df else 0))

        add(f"s2_19f_rep_cmpsb_all_equal_df{int(df)}", count=3,
            source=src, destination=dst,
            regions=[("source", BASE, b"\x41" * 3), ("destination", BASE + 0x100, b"\x41" * 3)],
            initial_flags=2 | PF_ZF | (0x400 if df else 0), final_count=0,
            final_source=src + step * 3, final_destination=dst + step * 3,
            final_flags=EQUAL | (0x400 if df else 0))

    for successes in range(3):
        for destination_fault in (False, True):
            source = BASE
            destination = BASE + 0x100
            regions = [("source", source, b"\x41" * (successes + (1 if destination_fault else 0))),
                       ("destination", destination, b"\x41" * successes)]
            address = (destination + successes) if destination_fault else (source + successes)
            add(f"s2_19f_rep_cmpsb_unmapped_after_{successes}_{'destination' if destination_fault else 'source'}",
                count=3, source=source, destination=destination, regions=regions,
                initial_flags=2 | PF_ZF, final_count=3-successes,
                final_source=source+successes, final_destination=destination+successes,
                final_flags=EQUAL if successes else 2 | PF_ZF, outcome="harness_error",
                fault={"access": "read", "address": hx(address)})

    for destination_fault in (False, True):
        add(f"s2_19f_rep_cmpsb_noncanonical_{'destination' if destination_fault else 'source'}",
            count=1, source=BASE if destination_fault else 0x0000800000000000,
            destination=0x0000800000000000 if destination_fault else BASE + 0x100,
            regions=[("source", BASE, b"\x41"), ("destination", BASE + 0x100, b"\x41")],
            initial_flags=2 | PF_ZF, final_count=1,
            final_source=BASE if destination_fault else 0x0000800000000000,
            final_destination=0x0000800000000000 if destination_fault else BASE + 0x100,
            final_flags=2 | PF_ZF, outcome="exception",
            fault={"vector": 13, "error_code": 0, "linear_address": None})

    for df in (False, True):
        source = BASE + (1024 if df else 0)
        destination = BASE + 4096 + (1024 if df else 0)
        add(f"s2_19f_rep_cmpsb_limit_1025_df{int(df)}", count=1025,
            source=source, destination=destination,
            regions=[("source", BASE, b"\x41"*2049), ("destination", BASE+4096, b"\x41"*2049)],
            initial_flags=2 | PF_ZF | (0x400 if df else 0), final_count=1,
            final_source=source + (-1024 if df else 1024),
            final_destination=destination + (-1024 if df else 1024),
            final_flags=EQUAL | (0x400 if df else 0), outcome="rep_iteration_budget_exhausted")

    add("s2_19f_rep_cmpsb_budget_zero", count=1, source=BASE, destination=BASE+0x100,
        regions=[("source", BASE, b"\x41"), ("destination", BASE+0x100, b"\x41")],
        initial_flags=2 | PF_ZF, final_count=1, final_source=BASE,
        final_destination=BASE+0x100, final_flags=2 | PF_ZF,
        outcome="instruction_budget_exhausted", budget=0, rip=RIP)

    composed = bytes.fromhex("4839c0f3a6")
    for suffix, count, source_data, destination_data, outcome, fc, fs, fd, ff, frip, fault in [
        ("success", 1, b"\x41", b"\x41", "stop_address", 0, BASE+1, BASE+0x101, EQUAL, RIP+5, None),
        ("first_fault", 2, b"", b"\x41", "harness_error", 2, BASE, BASE+0x100, 0x46, RIP+3, {"access":"read","address":hx(BASE)}),
        ("second_fault", 2, b"\x41", b"", "harness_error", 2, BASE, BASE+0x100, 0x46, RIP+3, {"access":"read","address":hx(BASE+0x100)}),
    ]:
        add(f"s2_19f_cmp_rax_rax_rep_cmpsb_{suffix}", code=composed,
            count=count, source=BASE, destination=BASE+0x100,
            regions=[("source", BASE, source_data), ("destination", BASE+0x100, destination_data)],
            initial_flags=0x08D7, final_count=fc, final_source=fs, final_destination=fd,
            final_flags=ff, outcome=outcome, rip=frip, budget=2, fault=fault)

    test_code = bytes.fromhex("4885c0f3a6")
    add("s2_19f_test_rax_rep_cmpsb_undefined_af_reject", code=test_code,
        count=1, source=BASE, destination=BASE+0x100,
        regions=[("source", BASE, b"\x41"), ("destination", BASE+0x100, b"\x41")],
        initial_flags=0x0CC7, final_count=1, final_source=BASE,
        final_destination=BASE+0x100, final_flags=0x446, outcome="unsupported",
        defined=0x0CC7, rip=RIP+3, budget=2, initial_rax=0)

    for suffix, source, destination in (("same", BASE, BASE), ("overlap", BASE, BASE+1)):
        count = 1 if suffix == "same" else 2
        add(f"s2_19f_rep_cmpsb_alias_{suffix}", count=count, source=source,
            destination=destination, regions=[("data", BASE, b"\x41"*4)],
            initial_flags=2 | PF_ZF, final_count=0, final_source=source+count,
            final_destination=destination+count, final_flags=EQUAL)

    assert len(answers) == 31, len(answers)
    files["s2_19f_rep_cmpsb_expected.json"] = dump(answers)
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
    print(f"{'checked' if args.check else 'wrote'} {len(expected)-1} fixtures and literal answers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
