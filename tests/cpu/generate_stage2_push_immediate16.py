#!/usr/bin/env python3
"""Materialize literal S2-17h 16-bit immediate PUSH fixtures and answers."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
STACK_BASE = 0x0030_0000
STACK_SIZE = 256
STACK_END = STACK_BASE + STACK_SIZE
CODE_BASE = 0x0010_0000

SUCCESS_CASES = [
    ("imm8_zero", bytes.fromhex("666a00"), 0x0000),
    ("imm8_max", bytes.fromhex("666a7f"), 0x007f),
    ("imm8_min", bytes.fromhex("666a80"), 0xff80),
    ("imm8_minus_one", bytes.fromhex("666aff"), 0xffff),
    ("imm16_zero", bytes.fromhex("66680000"), 0x0000),
    ("imm16_max", bytes.fromhex("6668ff7f"), 0x7fff),
    ("imm16_min", bytes.fromhex("66680080"), 0x8000),
    ("imm16_minus_one", bytes.fromhex("6668ffff"), 0xffff),
]


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rsp.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    cases = [
        *((name, code, value, STACK_END, None) for name, code, value in SUCCESS_CASES),
        ("fault_bottom", bytes.fromhex("66683412"), 0x1234, STACK_BASE, {"address": STACK_BASE - 2, "error_code": 6}),
        ("truncated_imm8", bytes.fromhex("666a"), 0, STACK_END, {"address": CODE_BASE + 2, "error_code": 5}),
        ("truncated_imm16", bytes.fromhex("666834"), 0, STACK_END, {"address": CODE_BASE + 3, "error_code": 5}),
    ]

    for name, code, value, rsp, fault in cases:
        case_id = f"s2_17h_push_{name}"
        obj = copy.deepcopy(template)
        initial = [int(item, 16) for item in obj["initial_state"]["gpr"]]
        initial[4] = rsp
        final = initial.copy()
        initial_stack = bytearray([0xA5] * STACK_SIZE)
        expected_stack = bytearray(initial_stack)
        if fault is None:
            final[4] = rsp - 2
            expected_stack[-2:] = value.to_bytes(2, "little")

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{item:016x}" for item in initial]
        obj["initial_state"]["rip"] = f"0x{CODE_BASE:016x}"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 16-bit immediate PUSH bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM order number 253667-093US, Vol.2B PUSH pp. 4-522–4-525: with 66 in long mode, 6A sign-extends imm8 to the 16-bit stack item; 68 reads imm16 as a literal word; RSP remains the 64-bit stack pointer and moves by two.",
            "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-17h",
        }

        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["guest_address"] = f"0x{CODE_BASE:016x}"
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{STACK_BASE:016x}"
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = initial_stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["stop_rip"] = f"0x{CODE_BASE + len(code):016x}"
        obj["instruction_budget"] = 1
        obj["expected_exit"] = "exception" if fault else "stop_address"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        exception = None
        if fault:
            exception = {
                "vector": 14,
                "error_code": fault["error_code"],
                "linear_address": f"0x{fault['address']:016x}",
                "fault_rip": f"0x{CODE_BASE:016x}",
            }
        expected[case_id] = {
            "gpr": [f"0x{item:016x}" for item in final],
            "rip": f"0x{CODE_BASE:016x}" if fault else f"0x{CODE_BASE + len(code):016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
            "exit": obj["expected_exit"],
            "exception": exception,
            "instructions_executed": 0 if fault else 1,
        }

    files["s2_17h_push_immediate16_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
    return files


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    for name, content in materialize().items():
        path = DEST / name
        if args.check:
            if not path.exists() or path.read_bytes() != content:
                stale.append(name)
        else:
            path.write_bytes(content)
    if stale:
        print("stale S2-17h immediate PUSH fixtures: " + ", ".join(stale))
        return 1
    print("8 S2-17h success fixtures, 3 fault fixtures, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
