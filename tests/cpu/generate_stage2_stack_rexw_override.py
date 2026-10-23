#!/usr/bin/env python3
"""Materialize literal S2-17m memory stack fixtures for 66 + REX.W."""
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
STACK_RSP = STACK_BASE + 120
VALUE = 0x8877_6655_4433_2211


def h(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    outputs: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    cases = [
        ("push_base", bytes.fromhex("6648ff30"), DATA_BASE),
        ("push_rsp_source", bytes.fromhex("6648ff3424"), STACK_RSP),
        ("pop_base", bytes.fromhex("66488f00"), DATA_BASE),
        ("pop_rsp_post_increment", bytes.fromhex("66488f0424"), STACK_RSP + 8),
    ]

    for name, raw_code, operand_address in cases:
        code = bytearray(raw_code)
        case_id = f"s2_17m_{name}"
        obj = copy.deepcopy(template)
        gpr = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        gpr[4] = STACK_RSP
        if name in ("push_base", "pop_base"):
            gpr[0] = DATA_BASE
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [h(value) for value in gpr]
        obj["initial_state"]["rip"] = h(CODE_BASE)
        obj["initial_state"]["rflags"] = h(0x802)
        obj["stop_rip"] = h(CODE_BASE + len(code))
        obj["instruction_budget"] = 1
        obj["expected_exit"] = "stop_address"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 66 + REX.W memory PUSH/POP bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM revision 093, Vol.2A §2.2.1.2 pp. 2-8–2-9: with REX.W=1, a preceding 66H is ignored for non-byte operations, and REX.W selects 64-bit operand size. Vol.2B PUSH pp. 4-522–4-523 and POP pp. 4-398–4-399 specify source/destination EA ordering relative to RSP. Duplicate legacy prefix groups remain outside this decoder profile; repeated REX prefixes use only the final REX immediately before the opcode.",
            "specification_version": "Intel SDM Vol.2A/2B revision 093 (253666-093US/253667-093US); project S2-17m",
        }

        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region.update(guest_address=h(CODE_BASE), length=len(code), initial_bytes_hex=code.hex(), comparison_mask_hex="ff" * len(code))
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        stack = bytearray([0xA5] * STACK_SIZE)
        if name in ("push_rsp_source", "pop_base", "pop_rsp_post_increment"):
            stack[STACK_RSP - STACK_BASE:STACK_RSP - STACK_BASE + 8] = VALUE.to_bytes(8, "little")
        stack_region.update(guest_address=h(STACK_BASE), length=STACK_SIZE, initial_bytes_hex=stack.hex(), comparison_mask_hex="ff" * STACK_SIZE)
        obj["memory"] = [region for region in obj["memory"] if region["region_id"] != "data"]
        if name in ("push_base", "pop_base"):
            obj["memory"].append({
                "region_id": "data", "guest_address": h(DATA_BASE), "length": 8,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": (VALUE.to_bytes(8, "little") if name == "push_base" else bytes([0xA5]) * 8).hex(),
                "comparison_mask_hex": "ff" * 8,
            })
        outputs[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final_gpr = gpr.copy()
        expected_regions = {region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]}
        if name.startswith("push"):
            final_gpr[4] -= 8
            if name == "push_rsp_source":
                stack[final_gpr[4] - STACK_BASE:final_gpr[4] - STACK_BASE + 8] = VALUE.to_bytes(8, "little")
                expected_regions["stack"] = stack.hex()
            else:
                stack[final_gpr[4] - STACK_BASE:final_gpr[4] - STACK_BASE + 8] = VALUE.to_bytes(8, "little")
                expected_regions["stack"] = stack.hex()
        else:
            final_gpr[4] += 8
            if name == "pop_rsp_post_increment":
                stack[operand_address - STACK_BASE:operand_address - STACK_BASE + 8] = VALUE.to_bytes(8, "little")
                expected_regions["stack"] = stack.hex()
            else:
                expected_regions["data"] = VALUE.to_bytes(8, "little").hex()
        expected[case_id] = {
            "gpr": [h(value) for value in final_gpr], "rip": h(CODE_BASE + len(code)),
            "rflags": obj["initial_state"]["rflags"], "rflags_defined": "0x0000000000000cd7",
            "memory": expected_regions, "exit": "stop_address", "exception": None,
            "instructions_executed": 1,
        }

    outputs["s2_17m_memory_stack_rexw_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
    return outputs


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
        print("stale S2-17m REX.W memory stack fixtures: " + ", ".join(stale))
        return 1
    print("4 REX.W memory PUSH/POP fixtures and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
