#!/usr/bin/env python3
"""Materialize literal S2-17e memory-source near indirect CALL fixtures."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
DATA_BASE = 0x0020_0000
STACK_BASE = 0x0030_0000
STACK_SIZE = 256
STACK_TOP = STACK_BASE + STACK_SIZE
CODE_BASE = 0x0010_0000
TARGET_OFFSET = 16
TARGET_VALUE = 0x8877_6655_4433_2211
NONCANONICAL = 0x0000_8000_0000_0000
INVALID_RSP = NONCANONICAL + 8


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17d_call_rax.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def add(case_id: str, call: bytes, *, data: bytes | None = None,
            data_readable: bool = True, registers: dict[int, int] | None = None,
            stop: bool = True, budget: int = 8, fault: str | None = None,
            rip_relative_source: bool = False) -> None:
        obj = copy.deepcopy(template)
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        initial[4] = STACK_TOP
        for register, value in (registers or {}).items():
            initial[register] = value
        if fault == "stack":
            initial[4] = INVALID_RSP

        code = bytearray(call)
        return_offset = len(call)
        pointer = (CODE_BASE + TARGET_OFFSET).to_bytes(8, "little")
        if rip_relative_source:
            code.extend(pointer)
        if len(code) > TARGET_OFFSET:
            raise ValueError("source pointer overlaps target body")
        code.extend(b"\x90" * (TARGET_OFFSET - len(code)))
        code.extend(b"\x48\xb8" + TARGET_VALUE.to_bytes(8, "little") + b"\xc3")

        stack = bytearray([0xA5] * STACK_SIZE)
        expected_stack = bytearray(stack)
        if fault is None:
            address = initial[4] - 8
            offset = address - STACK_BASE
            expected_stack[offset:offset + 8] = (CODE_BASE + return_offset).to_bytes(8, "little")

        code_region = next(r for r in obj["memory"] if r["region_id"] == "code")
        stack_region = next(r for r in obj["memory"] if r["region_id"] == "stack")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{STACK_BASE:016x}"
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["memory"] = [r for r in obj["memory"] if r["region_id"] != "data"]
        if data is not None:
            obj["memory"].append({
                "region_id": "data",
                "guest_address": f"0x{DATA_BASE:016x}",
                "length": len(data),
                "permissions": {"read": data_readable, "write": False, "execute": False},
                "initial_bytes_hex": data.hex(),
                "comparison_mask_hex": "ff" * len(data),
            })
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{value:016x}" for value in initial]
        obj["initial_state"]["rip"] = f"0x{CODE_BASE:016x}"
        obj["stop_rip"] = (f"0x{CODE_BASE + return_offset:016x}" if stop and fault is None else None)
        obj["instruction_budget"] = budget
        obj["expected_exit"] = "exception" if fault else (
            "instruction_budget_exhausted" if not stop else "stop_address"
        )
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 FF /2 memory CALL bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM document 325462 revision 093, Vol.2A CALL—Call Procedure pp. 3-122–3-125: FF /2 is near indirect CALL; memory source is r/m64 in long mode. Source read precedes target validation and the 64-bit return address push.",
            "specification_version": "Intel SDM revision 093; project S2-17e",
        }
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final = initial.copy()
        final_rip = CODE_BASE
        exit_kind = obj["expected_exit"]
        exception = None
        retired = 0
        if fault is None:
            if stop:
                final[0] = TARGET_VALUE
                final_rip = CODE_BASE + return_offset
                retired = 3
            else:
                final[4] -= 8
                final_rip = CODE_BASE + TARGET_OFFSET
                retired = 1
        else:
            vector = {"read": 14, "target": 13, "stack": 12}[fault]
            exception = {
                "vector": vector,
                "error_code": {"read": 5, "target": 0, "stack": 0}[fault],
                "linear_address": f"0x{DATA_BASE:016x}" if fault == "read" else None,
                "fault_rip": f"0x{CODE_BASE:016x}",
            }
        regions = {r["region_id"]: r["initial_bytes_hex"] for r in obj["memory"]}
        regions["stack"] = expected_stack.hex()
        expected[case_id] = {
            "gpr": [f"0x{value:016x}" for value in final],
            "rip": f"0x{final_rip:016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": regions,
            "exit": exit_kind,
            "exception": exception,
            "instructions_executed": retired,
        }

    ptr = lambda value: value.to_bytes(8, "little")
    add("s2_17e_call_mem_base", bytes.fromhex("ff10"), data=ptr(CODE_BASE + TARGET_OFFSET), registers={0: DATA_BASE})
    add("s2_17e_call_mem_rex_base", bytes.fromhex("41ff10"), data=ptr(CODE_BASE + TARGET_OFFSET), registers={8: DATA_BASE})
    add("s2_17e_call_mem_sib", bytes.fromhex("ff548810"), data=ptr(CODE_BASE + TARGET_OFFSET), registers={0: DATA_BASE - 0x18, 1: 2})
    add("s2_17e_call_mem_rip_relative", bytes.fromhex("ff1500000000"), rip_relative_source=True)
    add("s2_17e_call_mem_addr32", bytes.fromhex("67ff10"), data=ptr(CODE_BASE + TARGET_OFFSET), registers={0: 0x1_0020_0000})
    add("s2_17e_call_mem_budget", bytes.fromhex("ff10"), data=ptr(CODE_BASE + TARGET_OFFSET), registers={0: DATA_BASE}, stop=False, budget=1)
    add("s2_17e_call_mem_read_fault", bytes.fromhex("ff10"), data=ptr(CODE_BASE + TARGET_OFFSET), data_readable=False, registers={0: DATA_BASE}, fault="read")
    add("s2_17e_call_mem_noncanonical", bytes.fromhex("ff10"), data=ptr(NONCANONICAL), registers={0: DATA_BASE}, fault="target")
    add("s2_17e_call_mem_stack_fault", bytes.fromhex("ff10"), data=ptr(CODE_BASE + TARGET_OFFSET), registers={0: DATA_BASE}, fault="stack")
    files["s2_17e_indirect_call_memory_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17e indirect CALL fixtures: " + ", ".join(stale))
        return 1
    print("6 S2-17e target fixtures, 3 fault fixtures, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
