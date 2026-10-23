#!/usr/bin/env python3
"""Materialize literal S2-17a/17b 64-bit PUSH fixtures and answers."""
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
POP_VALUE = 0x8877665544332211

# Each tuple is an instruction byte sequence and its opcode-selected register.
CASES = [
    ("push_rax", bytes.fromhex("50"), 0, True),
    ("push_rsp", bytes.fromhex("54"), 4, True),
    ("push_r8", bytes.fromhex("4150"), 8, True),
    ("push_r15", bytes.fromhex("4157"), 15, True),
    ("pop_rax", bytes.fromhex("58"), 0, False),
    ("pop_rsp", bytes.fromhex("5c"), 4, False),
    ("pop_r8", bytes.fromhex("4158"), 8, False),
    ("pop_r15", bytes.fromhex("415f"), 15, False),
]


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_16b_seto_true_al.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    for name, code, register, push in CASES:
        case_id = f"s2_17a_{name}"
        obj = copy.deepcopy(template)
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        final = initial.copy()
        initial[4] = STACK_TOP if push else STACK_TOP - 8
        final[4] = initial[4]
        initial_stack = bytearray([0xA5] * STACK_SIZE)
        if not push:
            initial_stack[STACK_SIZE - 8:] = POP_VALUE.to_bytes(8, "little")
        expected_stack = bytearray(initial_stack)
        if push:
            value = initial[register]
            if register == 4:
                value = STACK_TOP
            expected_stack[STACK_SIZE - 8:] = value.to_bytes(8, "little")
            final[4] = STACK_TOP - 8
        elif register == 4:
            final[4] = POP_VALUE
        else:
            final[register] = POP_VALUE
            final[4] = STACK_TOP

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{value:016x}" for value in initial]
        obj["initial_state"]["rip"] = "0x0000000000100000"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 PUSH/POP opcode bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM revision 093, Vol.2B PUSH pp. 4-522–4-525 (description and ordering pp. 4-522–4-523) and POP pp. 4-398–4-402 (description and RSP ordering pp. 4-398–4-399): long-mode default 64-bit opcode-register forms, REX.B extension, stack update ordering, PUSH RSP source value, POP RSP destination overwrite.",
            "specification_version": "Intel SDM revision 093; project S2-17a",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{STACK_BASE:016x}"
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = initial_stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["stop_rip"] = f"0x{0x100000 + len(code):016x}"
        obj["instruction_budget"] = 1
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
        expected[case_id] = {
            "gpr": [f"0x{value:016x}" for value in final],
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
        }

    files["s2_17a_push_pop_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()

    immediate_cases = [
        ("push_imm8_zero", bytes.fromhex("6a00"), 0x0000_0000_0000_0000),
        ("push_imm8_max", bytes.fromhex("6a7f"), 0x0000_0000_0000_007f),
        ("push_imm8_min", bytes.fromhex("6a80"), 0xffff_ffff_ffff_ff80),
        ("push_imm8_minus_one", bytes.fromhex("6aff"), 0xffff_ffff_ffff_ffff),
        ("push_imm32_zero", bytes.fromhex("6800000000"), 0x0000_0000_0000_0000),
        ("push_imm32_max", bytes.fromhex("68ffffff7f"), 0x0000_0000_7fff_ffff),
        ("push_imm32_min", bytes.fromhex("6800000080"), 0xffff_ffff_8000_0000),
        ("push_imm32_minus_one", bytes.fromhex("68ffffffff"), 0xffff_ffff_ffff_ffff),
    ]
    immediate_expected: dict[str, dict] = {}
    for name, code, value in immediate_cases:
        case_id = f"s2_17b_{name}"
        obj = copy.deepcopy(template)
        initial = [int(item, 16) for item in obj["initial_state"]["gpr"]]
        final = initial.copy()
        initial[4] = STACK_TOP
        final[4] = STACK_TOP - 8
        initial_stack = bytes([0xA5] * STACK_SIZE)
        expected_stack = bytearray(initial_stack)
        expected_stack[-8:] = value.to_bytes(8, "little")
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{item:016x}" for item in initial]
        obj["initial_state"]["rip"] = "0x0000000000100000"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 PUSH immediate bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM revision 093, Vol.2B PUSH pp. 4-522–4-525: 6A sign-extends imm8 and 68 sign-extends imm32 to the 64-bit stack item in long mode.",
            "specification_version": "Intel SDM revision 093; project S2-17b",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stack_region["guest_address"] = f"0x{STACK_BASE:016x}"
        stack_region["length"] = STACK_SIZE
        stack_region["initial_bytes_hex"] = initial_stack.hex()
        stack_region["comparison_mask_hex"] = "ff" * STACK_SIZE
        obj["stop_rip"] = f"0x{0x100000 + len(code):016x}"
        obj["instruction_budget"] = 1
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
        immediate_expected[case_id] = {
            "gpr": [f"0x{item:016x}" for item in final],
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
        }
    files["s2_17b_push_immediate_expected.json"] = (
        json.dumps(immediate_expected, indent=2) + "\n"
    ).encode()
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
        print("stale S2-17a PUSH/POP fixtures: " + ", ".join(stale))
        return 1
    fixture_count = sum(name.endswith(".case.json") for name in generated)
    print(f"{fixture_count} S2-17a/17b PUSH fixtures and answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
