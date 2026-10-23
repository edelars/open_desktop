#!/usr/bin/env python3
"""Materialize literal S2-17d register near indirect CALL fixtures."""
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
INVALID_STACK_RSP = NONCANONICAL + 8


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17c_call_forward.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def add(case_id: str, code: bytes, target_register: int, target_offset: int,
            return_offset: int, stop: int | None, budget: int, *,
            exceptional: str | None = None, rsp: int = STACK_TOP) -> None:
        obj = copy.deepcopy(template)
        base = int(obj["initial_state"]["rip"], 16)
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        initial[4] = rsp
        if exceptional == "noncanonical_target":
            initial[target_register] = NONCANONICAL
        else:
            initial[target_register] = base + target_offset
        final = initial.copy()
        stack = bytearray([0xA5] * STACK_SIZE)
        expected_stack = bytearray(stack)
        exit_kind = "exception" if exceptional else "stop_address"
        instructions = 0 if exceptional else budget
        final_rip = base

        if exceptional == "stack_fault":
            final[4] = rsp
            vector = 12
        elif exceptional == "noncanonical_target":
            vector = 13
        else:
            stack_address = rsp - 8
            stack_offset = stack_address - STACK_BASE
            return_address = base + return_offset
            expected_stack[stack_offset:stack_offset + 8] = return_address.to_bytes(8, "little")
            assert stack[stack_offset:stack_offset + 8] != expected_stack[stack_offset:stack_offset + 8]
            if budget == 1 and stop is None:
                final[4] = rsp - 8
                final_rip = base + target_offset
                exit_kind = "instruction_budget_exhausted"
            else:
                final[4] = rsp
                final_rip = base + int(stop)

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{value:016x}" for value in initial]
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 FF /2 CALL bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM document 325462 revision 093, Vol.2A CALL—Call Procedure pp. 3-122–3-125: FF /2 is near indirect CALL; the register form selects r/m64 in long mode. The source register is read before the 64-bit return address is pushed.",
            "specification_version": "Intel SDM revision 093; project S2-17d",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{STACK_BASE:016x}"
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["initial_state"]["rip"] = f"0x{base:016x}"
        obj["stop_rip"] = None if stop is None else f"0x{base + stop:016x}"
        obj["instruction_budget"] = budget
        obj["expected_exit"] = exit_kind
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        expected[case_id] = {
            "gpr": [f"0x{value:016x}" for value in final],
            "rip": f"0x{final_rip:016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
            "exit": exit_kind,
            "exception": {
                "vector": vector,
                "error_code": 0,
                "fault_rip": f"0x{base:016x}",
            } if exceptional else None,
            "instructions_executed": instructions,
        }

    add("s2_17d_call_rax", bytes.fromhex("ffd0 9090 c3"), 0, 4, 2, 2, 2)
    add("s2_17d_call_r8", bytes.fromhex("41ffd0 9090 c3"), 8, 5, 3, 3, 2)
    add("s2_17d_call_budget", bytes.fromhex("ffd0 9090 c3"), 0, 4, 2, None, 1)
    add("s2_17d_call_noncanonical", bytes.fromhex("ffd0"), 0, 0, 2, None, 1, exceptional="noncanonical_target")
    add("s2_17d_call_stack_fault", bytes.fromhex("ffd0 9090 c3"), 0, 4, 2, None, 1, exceptional="stack_fault", rsp=INVALID_STACK_RSP)

    files["s2_17d_indirect_call_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17d indirect CALL fixtures: " + ", ".join(stale))
        return 1
    print("5 S2-17d indirect CALL fixtures and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
