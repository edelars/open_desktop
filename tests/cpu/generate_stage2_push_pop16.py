#!/usr/bin/env python3
"""Materialize literal S2-17g 16-bit GPR PUSH/POP fixtures and answers."""
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


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rsp.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    cases = [
        ("push_ax", bytes.fromhex("6650"), 0, True, STACK_END, None),
        ("pop_ax", bytes.fromhex("6658"), 0, False, STACK_BASE + 0x80, 0xBEEF),
        ("push_r8w", bytes.fromhex("664150"), 8, True, STACK_END, None),
        ("pop_r9w", bytes.fromhex("664159"), 9, False, STACK_BASE + 0x80, 0x1357),
        ("push_sp", bytes.fromhex("6654"), 4, True, STACK_END, None),
        ("pop_sp", bytes.fromhex("665c"), 4, False, STACK_BASE + 0x80, 0xBEEF),
        ("pop_sp_carry", bytes.fromhex("665c"), 4, False, 0x0030_FFFF, 0x2468),
        ("push_fault_bottom", bytes.fromhex("6650"), 0, True, STACK_BASE, None),
        ("pop_fault_top", bytes.fromhex("6658"), 0, False, STACK_END, None),
        ("truncated_fetch", bytes.fromhex("66"), 0, True, STACK_END, None),
    ]

    for name, code, register, push, rsp, pop_value in cases:
        case_id = f"s2_17g_{name}"
        obj = copy.deepcopy(template)
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        initial[4] = rsp
        final = initial.copy()
        stack_base = rsp if name == "pop_sp_carry" else STACK_BASE
        stack_size = 2 if name == "pop_sp_carry" else STACK_SIZE
        initial_stack = bytearray([0xA5] * stack_size)
        if pop_value is not None:
            offset = rsp - stack_base
            initial_stack[offset:offset + 2] = pop_value.to_bytes(2, "little")
        expected_stack = bytearray(initial_stack)

        fault = None
        if name == "push_fault_bottom":
            fault = {"address": STACK_BASE - 2, "error_code": 6}
        elif name == "pop_fault_top":
            fault = {"address": STACK_END, "error_code": 5}
        elif name == "truncated_fetch":
            fault = {"address": CODE_BASE + 1, "error_code": 5, "access": "fetch"}

        if fault is None:
            if push:
                source = initial[register] & 0xFFFF
                address = rsp - 2
                offset = address - stack_base
                expected_stack[offset:offset + 2] = source.to_bytes(2, "little")
                final[4] = address
            elif register == 4:
                post_increment = rsp + 2
                final[4] = (post_increment & ~0xFFFF) | (pop_value & 0xFFFF)
            else:
                final[register] = (initial[register] & ~0xFFFF) | (pop_value & 0xFFFF)
                final[4] = rsp + 2

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{value:016x}" for value in initial]
        obj["initial_state"]["rip"] = f"0x{CODE_BASE:016x}"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 word PUSH/POP bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM document 325462 revision 093, Vol.2B PUSH pp. 4-522–4-525 and POP pp. 4-398–4-402: 66 selects a 16-bit stack item while long-mode stack addressing remains RSP-sized; PUSH SP reads the source before decrement and POP SP writes the destination after increment.",
            "specification_version": "Intel SDM revision 093; project S2-17g",
        }

        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["guest_address"] = f"0x{CODE_BASE:016x}"
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{stack_base:016x}"
        stack_region["length"] = stack_size
        stack_region["initial_bytes_hex"] = initial_stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * stack_size
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
            "gpr": [f"0x{value:016x}" for value in final],
            "rip": f"0x{CODE_BASE:016x}" if fault else f"0x{CODE_BASE + len(code):016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
            "exit": obj["expected_exit"],
            "exception": exception,
            "instructions_executed": 0 if fault else 1,
        }

    files["s2_17g_push_pop16_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17g PUSH/POP fixtures: " + ", ".join(stale))
        return 1
    print("7 S2-17g success fixtures, 3 fault fixtures, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
