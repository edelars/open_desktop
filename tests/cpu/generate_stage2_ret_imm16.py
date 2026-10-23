#!/usr/bin/env python3
"""Materialize literal S2-17f near RET imm16 fixtures and answers."""
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
STACK_TOP = STACK_END
CODE_BASE = 0x0010_0000
NONCANONICAL = 0x0000_8000_0000_0000


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17c_call_forward.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def add(case_id: str, code: bytes, *, immediate: int = 0, rsp: int = STACK_BASE + 0x80,
            target: int | None = None, stop: int | None = None, budget: int = 1,
            fault: str | None = None, nested: bool = False) -> None:
        obj = copy.deepcopy(template)
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        initial[4] = rsp
        stack = bytearray([0xA5] * STACK_SIZE)
        expected_stack = bytearray(stack)

        if target is not None and not nested:
            stack_offset = rsp - STACK_BASE
            stack[stack_offset:stack_offset + 8] = target.to_bytes(8, "little")
            expected_stack = bytearray(stack)
        if fault == "target":
            target = NONCANONICAL
            stack_offset = rsp - STACK_BASE
            stack[stack_offset:stack_offset + 8] = target.to_bytes(8, "little")
            expected_stack = bytearray(stack)
        if fault == "stack":
            initial[4] = NONCANONICAL + 8

        if nested:
            outer_return = CODE_BASE + 5
            inner_return = CODE_BASE + 19
            outer_offset = rsp - 8 - STACK_BASE
            inner_offset = rsp - 16 - STACK_BASE
            expected_stack[outer_offset:outer_offset + 8] = outer_return.to_bytes(8, "little")
            expected_stack[inner_offset:inner_offset + 8] = inner_return.to_bytes(8, "little")

        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["guest_address"] = f"0x{CODE_BASE:016x}"
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{STACK_BASE:016x}"
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{value:016x}" for value in initial]
        obj["initial_state"]["rip"] = f"0x{CODE_BASE:016x}"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 RET imm16 bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM document 325462 revision 093, Vol.2B RET—Return From Procedure p.4-566: C2 iw is a near return that pops imm16 bytes after the return address; in 64-bit mode the return target is 64-bit.",
            "specification_version": "Intel SDM revision 093; project S2-17f",
        }
        obj["stop_rip"] = None if stop is None else f"0x{CODE_BASE + stop:016x}"
        obj["instruction_budget"] = budget
        obj["expected_exit"] = "exception" if fault else (
            "instruction_budget_exhausted" if stop is None else "stop_address"
        )
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final = initial.copy()
        final_rip = CODE_BASE
        exit_kind = obj["expected_exit"]
        exception = None
        retired = 0
        if fault:
            vector, error_code, linear = {
                "target": (13, 0, None),
                "stack": (12, 0, None),
                "read": (14, 5, STACK_END),
                "truncated": (14, 5, CODE_BASE + 2),
            }[fault]
            exception = {
                "vector": vector,
                "error_code": error_code,
                "linear_address": None if linear is None else f"0x{linear:016x}",
                "fault_rip": f"0x{CODE_BASE:016x}",
            }
        elif nested:
            final[4] = rsp + 16
            final_rip = CODE_BASE + (stop if stop is not None else 0)
            retired = 5
        else:
            final[4] = rsp + 8 + immediate
            final_rip = CODE_BASE + len(code) if stop is None else CODE_BASE + stop
            retired = 1

        expected[case_id] = {
            "gpr": [f"0x{value:016x}" for value in final],
            "rip": f"0x{final_rip:016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
            "exit": exit_kind,
            "exception": exception,
            "instructions_executed": retired,
        }

    def ret_code(value: int) -> bytes:
        return bytes((0xC2, value & 0xff, (value >> 8) & 0xff))

    add("s2_17f_ret_imm0", ret_code(0), target=CODE_BASE + 3, stop=3)
    add("s2_17f_ret_imm_small", ret_code(0x20), immediate=0x20, target=CODE_BASE + 3, stop=3, rsp=STACK_BASE + 0x70)
    add("s2_17f_ret_imm_max", ret_code(0xffff), immediate=0xffff, target=CODE_BASE + 3, stop=3, rsp=STACK_BASE + 8)
    add("s2_17f_ret_imm_nested_cleanup", bytes.fromhex("e809000000e90f00000090909090e805000000c210000000c3"), stop=25, budget=6, nested=True)
    add("s2_17f_ret_imm_budget", ret_code(0x20), immediate=0x20, target=CODE_BASE + 3, stop=None, budget=1)
    add("s2_17f_ret_imm_noncanonical", ret_code(0x20), immediate=0x20, stop=None, budget=1, fault="target")
    add("s2_17f_ret_imm_stack_fault", ret_code(0), stop=None, budget=1, fault="stack")
    add("s2_17f_ret_imm_read_fault", ret_code(0), stop=None, budget=1, rsp=STACK_END, fault="read")
    add("s2_17f_ret_imm_truncated", bytes.fromhex("c234"), stop=None, budget=1, fault="truncated")

    files["s2_17f_ret_imm16_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17f RET imm16 fixtures: " + ", ".join(stale))
        return 1
    print("5 S2-17f success fixtures, 4 fault fixtures, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
