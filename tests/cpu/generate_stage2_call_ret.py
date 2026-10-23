#!/usr/bin/env python3
"""Materialize literal S2-17c near CALL rel32 and RET fixtures and answers."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
STACK_BASE = 0x300000
STACK_SIZE = 256
STACK_TOP = STACK_BASE + STACK_SIZE
NONCANONICAL = 0x0000_8000_0000_0000
CALL_HIGH = 0x0000_7fff_ffff_fffb


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def add(case_id: str, code: bytes, *, entry: int = 0, stop: int | None,
            budget: int, stack: bytes | None = None, rsp: int = STACK_TOP,
            exit_kind: str = "stop_address", exceptional: bool = False,
            high_code: bool = False) -> None:
        obj = copy.deepcopy(template)
        base = CALL_HIGH if high_code else 0x100000
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        final = initial.copy()
        initial[4] = rsp
        final[4] = rsp
        stack_bytes = bytearray(stack if stack is not None else bytes([0xA5] * STACK_SIZE))
        expected_stack = bytearray(stack_bytes)
        next_rip = base + entry + (5 if code[entry] == 0xE8 else len(code))

        if case_id == "s2_17c_ret_noncanonical":
            stack_bytes[-8:] = NONCANONICAL.to_bytes(8, "little")
            expected_stack = bytearray(stack_bytes)
            initial[4] = rsp - 8
            final[4] = rsp - 8
        elif exit_kind == "stop_address":
            if case_id == "s2_17c_call_forward":
                final[4] = rsp - 8
                final[4] += 8
                expected_stack[-8:] = (base + 5).to_bytes(8, "little")
            elif case_id == "s2_17c_call_nested":
                expected_stack[-16:] = (base + 15).to_bytes(8, "little") + (base + 5).to_bytes(8, "little")
            elif case_id == "s2_17c_call_negative_rel32":
                expected_stack[-8:] = (base + 15).to_bytes(8, "little")
        elif exit_kind == "instruction_budget_exhausted":
            final[4] = rsp - 16
            expected_stack[-16:] = (base + 15).to_bytes(8, "little") + (base + 5).to_bytes(8, "little")
            next_rip = base + 20

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{value:016x}" for value in initial]
        obj["initial_state"]["rip"] = f"0x{base + entry:016x}"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 CALL/RET bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM revision 093, Vol.2A CALL—Call Procedure pp. 3-122–3-123 and Vol.2B RET—Return From Procedure: E8 rel32 is sign-extended from the next RIP in 64-bit mode; near CALL pushes that next RIP as a 64-bit stack item, and C3 near RET pops a 64-bit target.",
            "specification_version": "Intel SDM revision 093; project S2-17c",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["guest_address"] = f"0x{base:016x}"
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{STACK_BASE:016x}"
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = stack_bytes.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["stop_rip"] = None if stop is None else f"0x{base + stop:016x}"
        obj["instruction_budget"] = budget
        obj["expected_exit"] = "exception" if exceptional else exit_kind
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        if exceptional:
            final = initial.copy()
            exit_kind = "exception"
        elif case_id == "s2_17c_call_negative_rel32":
            final[4] = rsp
        elif case_id == "s2_17c_call_forward":
            final[4] = rsp
        elif case_id == "s2_17c_call_nested":
            final[4] = rsp
        expected[case_id] = {
            "gpr": [f"0x{value:016x}" for value in final],
            "rip": f"0x{base + entry:016x}" if exceptional and high_code else (
                f"0x{base + entry:016x}" if exceptional else
                f"0x{base + stop:016x}" if stop is not None else f"0x{next_rip:016x}"
            ),
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
            "exit": exit_kind,
            "exception": {"vector": 13, "error_code": 0, "fault_rip": f"0x{base + entry:016x}"} if exceptional else None,
            "instructions_executed": 0 if exceptional else budget,
        }

    add("s2_17c_call_forward", bytes.fromhex("e805000000 9090909090 c3"), stop=5, budget=2)
    add("s2_17c_call_nested", bytes.fromhex("e805000000 9090909090 e805000000 c3 90909090 c3"), stop=5, budget=4)
    add("s2_17c_call_negative_rel32", bytes.fromhex("9090909090 c3 90909090 e8f6ffffff"), entry=10, stop=15, budget=2)
    add("s2_17c_call_budget", bytes.fromhex("e805000000 9090909090 e805000000 c3 90909090 c3"), stop=None, budget=2, exit_kind="instruction_budget_exhausted")
    add("s2_17c_call_noncanonical", bytes.fromhex("e800000000"), stop=None, budget=1, exceptional=True, high_code=True)
    add("s2_17c_ret_noncanonical", bytes.fromhex("c3"), stop=None, budget=1, rsp=STACK_TOP, exceptional=True)

    files["s2_17c_call_ret_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
    return files


def main() -> int:
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    generated = materialize()
    stale = []
    for name, content in generated.items():
        path = DEST / name
        if args.check:
            if not path.exists() or path.read_bytes() != content:
                stale.append(name)
        else:
            path.write_bytes(content)
    if stale:
        print("stale S2-17c CALL/RET fixtures: " + ", ".join(stale))
        return 1
    print("6 S2-17c CALL/RET fixtures and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
