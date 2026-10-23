#!/usr/bin/env python3
"""Materialize literal S2-17i qword memory-source PUSH fixtures."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x0010_0000
DATA_BASE = 0x0020_0000
STACK_BASE = 0x0030_0000
STACK_SIZE = 256
STACK_TOP = STACK_BASE + STACK_SIZE
SOURCE_VALUE = 0x8877_6655_4433_2211
NONCANONICAL = 0x0000_8000_0000_0000


def _hex64(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    cases = [
        ("base", bytes.fromhex("ff30"), {0: DATA_BASE}, "data", "stop", 1),
        ("rex_base", bytes.fromhex("41ff30"), {8: DATA_BASE}, "data", "stop", 1),
        ("sib", bytes.fromhex("ff348b"), {3: DATA_BASE - 8, 1: 2}, "data", "stop", 1),
        ("rip_relative", None, {}, "data", "stop", 1),
        ("addr32", bytes.fromhex("67ff30"), {0: 0x1_0020_0000}, "data", "stop", 1),
        ("rsp_source", bytes.fromhex("ff3424"), {}, "stack", "stop", 1),
        ("budget", bytes.fromhex("ff30"), {0: DATA_BASE}, "data", "budget", 1),
        ("read_fault", bytes.fromhex("ff30"), {0: DATA_BASE}, "read_fault", "fault", 1),
        ("stack_fault", bytes.fromhex("ff30"), {0: DATA_BASE}, "stack_fault", "fault", 1),
    ]

    for name, encoded, registers, source_kind, outcome, budget in cases:
        case_id = f"s2_17i_push_mem_{name}"
        code = bytearray(encoded or b"")
        if name == "rip_relative":
            next_rip = CODE_BASE + 6
            code.extend(b"\xff\x35" + (DATA_BASE - next_rip).to_bytes(4, "little", signed=True))

        obj = copy.deepcopy(template)
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        initial[4] = STACK_TOP - (8 if source_kind == "stack" else 0)
        for register, value in registers.items():
            initial[register] = value
        if source_kind == "stack_fault":
            initial[4] = NONCANONICAL + 8

        stack = bytearray([0xA5] * STACK_SIZE)
        source_value = SOURCE_VALUE
        source_address = DATA_BASE
        if source_kind == "stack":
            stack[-8:] = SOURCE_VALUE.to_bytes(8, "little")
            source_address = initial[4]
        expected_stack = bytearray(stack)
        fault = outcome == "fault"
        if not fault:
            stack_offset = initial[4] - 8 - STACK_BASE
            expected_stack[stack_offset:stack_offset + 8] = source_value.to_bytes(8, "little")

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [_hex64(value) for value in initial]
        obj["initial_state"]["rip"] = _hex64(CODE_BASE)
        obj["initial_state"]["rflags"] = _hex64(0x802)
        obj["stop_rip"] = _hex64(CODE_BASE + len(code)) if outcome == "stop" else None
        obj["instruction_budget"] = budget
        obj["expected_exit"] = {
            "stop": "stop_address",
            "budget": "instruction_budget_exhausted",
            "fault": "exception",
        }[outcome]
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 FF /6 memory PUSH bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM order number 253667-093US, Vol.2B PUSH pp. 4-522–4-525: PUSH r/m64 reads the source operand and writes a 64-bit stack item; ordinary effective-address forms are covered here.",
            "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-17i",
        }

        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["guest_address"] = _hex64(CODE_BASE)
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = _hex64(STACK_BASE)
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["memory"] = [region for region in obj["memory"] if region["region_id"] != "data"]
        if source_kind not in ("stack",):
            obj["memory"].append({
                "region_id": "data",
                "guest_address": _hex64(DATA_BASE),
                "length": 8,
                "permissions": {
                    "read": source_kind != "read_fault",
                    "write": False,
                    "execute": False,
                },
                "initial_bytes_hex": SOURCE_VALUE.to_bytes(8, "little").hex(),
                "comparison_mask_hex": "ff" * 8,
            })
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final = initial.copy()
        final_rip = CODE_BASE
        retired = 0
        if outcome == "stop":
            final[4] -= 8
            final_rip = CODE_BASE + len(code)
            retired = 1
        elif outcome == "budget":
            final[4] -= 8
            final_rip = CODE_BASE + len(code)
            retired = 1
        exception = None
        if outcome == "fault":
            is_stack = source_kind == "stack_fault"
            exception = {
                "vector": 12 if is_stack else 14,
                "error_code": 0 if is_stack else 5,
                "linear_address": None if is_stack else _hex64(source_address),
                "fault_rip": _hex64(CODE_BASE),
            }
        regions = {region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]}
        regions["stack"] = expected_stack.hex()
        expected[case_id] = {
            "gpr": [_hex64(value) for value in final],
            "rip": _hex64(final_rip),
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": regions,
            "exit": obj["expected_exit"],
            "exception": exception,
            "instructions_executed": retired,
        }

    files["s2_17i_push_memory_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17i memory PUSH fixtures: " + ", ".join(stale))
        return 1
    print("7 S2-17i success fixtures, 2 fault fixtures, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
