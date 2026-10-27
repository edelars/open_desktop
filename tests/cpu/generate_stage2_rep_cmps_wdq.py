#!/usr/bin/env python3
"""Serialize hand-authored S2-19h REP CMPSW/D/Q cases and literal answers."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE = 0x300000
RIP = 0x100000
PF_ZF = 0x44
EQUAL = 0x46
MISMATCH = 0x97
DEFINED = 0x0CD7
BASE_GPR = [0x1122334455667741, 0, 0xDEADBEEFFEDCBA98, 0x200000,
            0x300080, 0x300080, BASE, BASE + 0x10000,
            0x0123456789ABCDEF, 0xFEDCBA9876543210, 0x8877665544332211,
            0xAABBCCDDEEFF0011, 0x1122112211221122, 0x3344334433443344,
            0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
FORMS = {
    "w": (2, bytes.fromhex("f366a7"), "CMPSW", "F3 66 A7"),
    "d": (4, bytes.fromhex("f3a7"), "CMPSD", "F3 A7"),
    "q": (8, bytes.fromhex("f348a7"), "CMPSQ", "F3 REX.W A7"),
}


def hx(value: int) -> str:
    return f"0x{value & ((1 << 64) - 1):016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    def add(name: str, *, code: bytes, width: int, count: int, source: int,
            destination: int, regions: list[tuple[str, int, bytes]],
            initial_flags: int = 2 | PF_ZF, final_count: int | None = None,
            final_source: int | None = None, final_destination: int | None = None,
            final_flags: int | None = None, defined: int = DEFINED,
            outcome: str = "stop_address", budget: int = 1, rip: int | None = None,
            fault: dict | None = None, initial_rax: int | None = None) -> None:
        gpr = BASE_GPR.copy()
        if initial_rax is not None:
            gpr[0] = initial_rax
        gpr[1], gpr[6], gpr[7] = count, source, destination
        final = gpr.copy()
        final[1] = count if final_count is None else final_count
        final[6] = source if final_source is None else final_source
        final[7] = destination if final_destination is None else final_destination
        memory = [{
            "region_id": "code", "guest_address": hx(RIP), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }]
        for region_id, address, data in regions:
            if data:
                memory.append({
                    "region_id": region_id, "guest_address": hx(address), "length": len(data),
                    "permissions": {"read": True, "write": False, "execute": False},
                    "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data),
                })
        provenance = (
            "Handwritten instruction bytes and literal state/flag/memory answers; "
            "tests/cpu/generate_stage2_rep_cmps_wdq.py serializes constants only. "
            "No assembler, oracle, or execution engine is used."
        )
        fixture = {
            "schema_version": 2, "case_id": name, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture", "source": provenance,
                "specification": "Intel SDM revision 093, Vol.2B CMPS and REP/REPE entries; CMPS compares source DS:[RSI] minus destination ES:[RDI] in the stated order and both indices follow DF.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19h REPE CMPSW/D/Q",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gpr], "rip": hx(RIP), "rflags": hx(initial_flags)},
            "memory": memory,
            "stop_rip": hx(RIP + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        answer: dict[str, object] = {
            "gpr": [hx(x) for x in final],
            "rflags": hx(initial_flags if final_flags is None else final_flags),
            "rflags_defined": hx(defined),
            "rip": hx(rip if rip is not None else (RIP + len(code) if outcome == "stop_address" else RIP)),
            "memory": {rid: data.hex() for rid, _, data in [("code", RIP, code), *regions] if data},
        }
        if fault is not None:
            answer["fault"] = fault
        files[f"{name}.case.json"] = dump(fixture)
        answers[name] = answer

    for suffix, (width, opcode, _label, _encoding) in FORMS.items():
        code = bytes([0xF3, 0x66, 0xA7]) if suffix == "w" else bytes([0xF3, 0xA7]) if suffix == "d" else bytes([0xF3, 0x48, 0xA7])
        delta = width
        for df in (False, True):
            flags = 2 | PF_ZF | (0x400 if df else 0)
            invalid_source, invalid_destination = 0x0000800000000000, 0x0000800000000000
            add(f"s2_19h_rep_cmps{suffix}_zero_df{int(df)}", code=code, width=width,
                count=0, source=invalid_source, destination=invalid_destination, regions=[],
                initial_flags=flags)

            for label, mismatch in (("equal", False), ("mismatch", True)):
                source = BASE + (2 * delta if df else 0)
                destination = BASE + 0x10000 + (2 * delta if df else 0)
                source_data = bytearray(b"\x41" * (3 * delta))
                destination_data = bytearray(b"\x41" * (3 * delta))
                if mismatch:
                    index = (2 * delta if df else 0)
                    destination_data[index] = 0x42
                flags_out = (MISMATCH if mismatch else EQUAL) | (0x400 if df else 0)
                step = -delta if df else delta
                add(f"s2_19h_rep_cmps{suffix}_one_{label}_df{int(df)}", code=code,
                    width=width, count=1, source=source, destination=destination,
                    regions=[("source", BASE, bytes(source_data)), ("destination", BASE + 0x10000, bytes(destination_data))],
                    initial_flags=flags, final_count=0, final_source=source + step,
                    final_destination=destination + step, final_flags=flags_out)

            source = BASE + (2 * delta if df else 0)
            destination = BASE + 0x10000 + (2 * delta if df else 0)
            source_data = bytearray(b"\x41" * (3 * delta))
            destination_data = bytearray(b"\x41" * (3 * delta))
            destination_data[delta if df else delta] = 0x42
            final_flags = MISMATCH | (0x400 if df else 0)
            distance = 2 * delta
            step = -distance if df else distance
            add(f"s2_19h_rep_cmps{suffix}_mismatch_middle_df{int(df)}", code=code,
                width=width, count=3, source=source, destination=destination,
                regions=[("source", BASE, bytes(source_data)), ("destination", BASE + 0x10000, bytes(destination_data))],
                initial_flags=flags, final_count=1, final_source=source + step,
                final_destination=destination + step, final_flags=final_flags)

            equal_source = BASE + (2 * delta if df else 0)
            equal_destination = BASE + 0x10000 + (2 * delta if df else 0)
            step = -3 * delta if df else 3 * delta
            add(f"s2_19h_rep_cmps{suffix}_all_equal_df{int(df)}", code=code,
                width=width, count=3, source=equal_source, destination=equal_destination,
                regions=[("source", BASE, b"\x41" * (3 * delta)), ("destination", BASE + 0x10000, b"\x41" * (3 * delta))],
                initial_flags=flags, final_count=0, final_source=equal_source + step,
                final_destination=equal_destination + step, final_flags=EQUAL | (0x400 if df else 0))

        # First source/destination faults and faults after one complete equal element.
        for successes in (0, 1):
            for destination_fault in (False, True):
                source = BASE
                destination = BASE + 0x10000
                source_length = successes * delta + (delta if destination_fault else 0)
                destination_length = successes * delta
                regions = [("source", source, b"\x41" * source_length), ("destination", destination, b"\x41" * destination_length)]
                fault_address = destination + successes * delta if destination_fault else source + successes * delta
                add(f"s2_19h_rep_cmps{suffix}_unmapped_{'destination' if destination_fault else 'source'}_after{successes}",
                    code=code, width=width, count=3, source=source, destination=destination,
                    regions=regions, initial_flags=2 | PF_ZF, final_count=3 - successes,
                    final_source=source + successes * delta, final_destination=destination + successes * delta,
                    final_flags=EQUAL if successes else 2 | PF_ZF, outcome="harness_error",
                    fault={"access": "read", "address": hx(fault_address)})

        # One noncanonical source and destination case per width.
        for destination_fault in (False, True):
            source = BASE if destination_fault else 0x0000800000000000
            destination = 0x0000800000000000 if destination_fault else BASE + 0x10000
            add(f"s2_19h_rep_cmps{suffix}_noncanonical_{'destination' if destination_fault else 'source'}",
                code=code, width=width, count=1, source=source, destination=destination,
                regions=[("source", BASE, b"\x41" * width), ("destination", BASE + 0x10000, b"\x41" * width)],
                initial_flags=2 | PF_ZF, outcome="exception",
                fault={"vector": 13, "error_code": 0, "linear_address": None})

        # 1024 committed equal iterations leave one element and preserve REP RIP.
        for df in (False, True):
            source = BASE + (1024 * width if df else 0)
            destination = BASE + 0x10000 + (1024 * width if df else 0)
            movement = -1024 * width if df else 1024 * width
            add(f"s2_19h_rep_cmps{suffix}_limit1025_df{int(df)}", code=code,
                width=width, count=1025, source=source, destination=destination,
                regions=[("source", BASE, b"\x41" * (2049 * width)), ("destination", BASE + 0x10000, b"\x41" * (2049 * width))],
                initial_flags=2 | PF_ZF | (0x400 if df else 0), final_count=1,
                final_source=source + movement, final_destination=destination + movement,
                final_flags=EQUAL | (0x400 if df else 0), outcome="rep_iteration_budget_exhausted")

        # Zero instruction budget does not enter the REP body.
        add(f"s2_19h_rep_cmps{suffix}_budget_zero", code=code, width=width,
            count=1, source=BASE, destination=BASE + 0x10000,
            regions=[("source", BASE, b"\x41" * width), ("destination", BASE + 0x10000, b"\x41" * width)],
            initial_flags=2 | PF_ZF, outcome="instruction_budget_exhausted", budget=0, rip=RIP)

    # Width normalization with the effective REX before/after 66.
    for name, code, width in (
        ("s2_19h_rep_cmpsw_rex_discarded_by_66", bytes.fromhex("f34866a7"), 2),
        ("s2_19h_rep_cmpsq_66_overridden_by_rex", bytes.fromhex("f36648a7"), 8),
    ):
        add(name, code=code, width=width, count=1, source=BASE, destination=BASE + 0x10000,
            regions=[("source", BASE, b"\x41" * width), ("destination", BASE + 0x10000, b"\x41" * width)],
            initial_flags=2 | PF_ZF, final_count=0, final_source=BASE + width,
            final_destination=BASE + 0x10000 + width, final_flags=EQUAL)

    # CMP producer compositions: success, fault on either ordered read, and TEST gate reject.
    for suffix, (width, rep, _label, _encoding) in FORMS.items():
        code = bytes.fromhex("4839c0") + rep
        length = len(code)
        for case, source_data, destination_data, outcome, final_count, final_source, final_destination, final_flags, final_rip, fault in (
            ("success", b"\x41" * width, b"\x41" * width, "stop_address", 0, BASE + width, BASE + 0x10000 + width, EQUAL, RIP + length, None),
            ("first_fault", b"", b"\x41" * width, "harness_error", 2, BASE, BASE + 0x10000, 0x46, RIP + 3, {"access": "read", "address": hx(BASE)}),
            ("second_fault", b"\x41" * width, b"", "harness_error", 2, BASE, BASE + 0x10000, 0x46, RIP + 3, {"access": "read", "address": hx(BASE + 0x10000)}),
        ):
            add(f"s2_19h_cmp_rax_rax_rep_cmps{suffix}_{case}", code=code, width=width,
                count=1 if case == "success" else 2, source=BASE, destination=BASE + 0x10000,
                regions=[("source", BASE, source_data), ("destination", BASE + 0x10000, destination_data)],
                initial_flags=0x08D7, final_count=final_count, final_source=final_source,
                final_destination=final_destination, final_flags=final_flags, outcome=outcome,
                budget=2, rip=final_rip, fault=fault)
        test_code = bytes.fromhex("4885c0") + rep
        add(f"s2_19h_test_rax_rep_cmps{suffix}_undefined_af_reject", code=test_code,
            width=width, count=1, source=BASE, destination=BASE + 0x10000,
            regions=[("source", BASE, b"\x41" * width), ("destination", BASE + 0x10000, b"\x41" * width)],
            initial_flags=0x0CC7, final_flags=0x446, defined=0x0CC7,
            outcome="unsupported", budget=2, rip=RIP + 3, initial_rax=0)

    assert len(answers) == 71, len(answers)
    files["s2_19h_rep_cmps_wdq_expected.json"] = dump(answers)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    mismatches = []
    for name, contents in materialize().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                mismatches.append(name)
        else:
            path.write_bytes(contents)
    if mismatches:
        print("out of date: " + ", ".join(mismatches))
        return 1
    print(f"{'checked' if args.check else 'wrote'} 71 fixtures and literal answers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
