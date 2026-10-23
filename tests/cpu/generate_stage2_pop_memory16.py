#!/usr/bin/env python3
"""Materialize literal S2-17l 16-bit memory-destination POP fixtures."""
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
VALUE = 0xBEEF


def h(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    digest = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    outputs: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    cases = [
        ("base", bytes.fromhex("668f00"), {0: DATA_BASE}, DATA_BASE),
        ("rex_base", bytes.fromhex("66418f00"), {8: DATA_BASE}, DATA_BASE),
        ("sib", bytes.fromhex("668f048b"), {3: DATA_BASE - 8, 1: 2}, DATA_BASE),
        ("rip_relative", None, {}, DATA_BASE),
        ("addr32", bytes.fromhex("67668f00"), {0: 0x1_0020_0000}, DATA_BASE),
        ("rsp_post_increment", bytes.fromhex("668f0424"), {}, STACK_RSP + 2),
    ]
    for name, raw_code, registers, destination in cases:
        code = bytearray(raw_code or b"")
        if name == "rip_relative":
            next_rip = CODE_BASE + 7
            code.extend(b"\x66\x8f\x05" + (DATA_BASE - next_rip).to_bytes(4, "little", signed=True))
        case_id = f"s2_17l_pop_mem_{name}"
        obj = copy.deepcopy(template)
        gpr = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        gpr[4] = STACK_RSP
        for register, value in registers.items():
            gpr[register] = value
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [h(value) for value in gpr]
        obj["initial_state"]["rip"] = h(CODE_BASE)
        obj["stop_rip"] = h(CODE_BASE + len(code))
        obj["instruction_budget"] = 1
        obj["expected_exit"] = "stop_address"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 66 8F /0 memory POP bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {digest}; no assembler or emulator.",
            "specification": "Intel SDM order number 253667-093US, Vol.2B POP pp. 4-398–4-402: r/m16 pops a word and advances RSP by two; a destination EA using RSP is computed after that increment (pp. 4-398–4-399).",
            "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-17l",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region.update(guest_address=h(CODE_BASE), length=len(code), initial_bytes_hex=code.hex(), comparison_mask_hex="ff" * len(code))
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        stack = bytearray([0xA5] * STACK_SIZE)
        stack[STACK_RSP - STACK_BASE:STACK_RSP - STACK_BASE + 2] = VALUE.to_bytes(2, "little")
        stack_region.update(guest_address=h(STACK_BASE), length=STACK_SIZE, initial_bytes_hex=stack.hex(), comparison_mask_hex="ff" * STACK_SIZE)
        data_region = next((region for region in obj["memory"] if region["region_id"] == "data"), None)
        if destination != STACK_RSP + 2:
            if data_region is None:
                data_region = copy.deepcopy(stack_region)
                data_region["region_id"] = "data"
                obj["memory"].append(data_region)
            data_region.update(guest_address=h(DATA_BASE), length=2, permissions={"read": True, "write": True, "execute": False}, initial_bytes_hex="a5" * 2, comparison_mask_hex="ff" * 2)
        obj["memory"] = [region for region in obj["memory"] if region["region_id"] != "data" or destination != STACK_RSP + 2]
        outputs[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final_gpr = gpr.copy()
        final_gpr[4] += 2
        memories = {region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]}
        if destination == STACK_RSP + 2:
            stack[STACK_RSP + 2 - STACK_BASE:STACK_RSP + 4 - STACK_BASE] = VALUE.to_bytes(2, "little")
            memories["stack"] = stack.hex()
        else:
            memories["data"] = VALUE.to_bytes(2, "little").hex()
        expected[case_id] = {
            "gpr": [h(value) for value in final_gpr], "rip": h(CODE_BASE + len(code)),
            "rflags": obj["initial_state"]["rflags"], "rflags_defined": "0x0000000000000cd7",
            "memory": memories, "exit": "stop_address", "exception": None, "instructions_executed": 1,
        }

    # Intel documents the store #PF but does not establish whether RSP's staged
    # increment is architecturally committed when the destination store faults.
    fault_id = "s2_17l_pop_mem_store_pf"
    fault_obj = copy.deepcopy(template)
    fgpr = [int(value, 16) for value in fault_obj["initial_state"]["gpr"]]
    fgpr[0], fgpr[4] = DATA_BASE, STACK_RSP
    fault_obj["case_id"] = fault_id
    fault_obj["initial_state"]["gpr"] = [h(value) for value in fgpr]
    fault_obj["initial_state"]["rip"] = h(CODE_BASE)
    fault_obj["stop_rip"] = None
    fault_obj["instruction_budget"] = 1
    fault_obj["expected_exit"] = "exception"
    fault_obj["provenance"] = {
        "kind": "project_fixture", "source": f"Handwritten x86 66 8F /0 destination-store #PF case; {fault_id}. Materializer SHA256 {digest}; no assembler or emulator.",
        "specification": "Intel SDM order number 253667-093US, Vol.2B POP pp. 4-398–4-402: #PF is listed; architectural RSP post-state on destination-store fault is not specified in the cited operation text.",
        "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-17l",
    }
    code_region = next(region for region in fault_obj["memory"] if region["region_id"] == "code")
    code_region.update(guest_address=h(CODE_BASE), length=3, initial_bytes_hex="668f00", comparison_mask_hex="ffffff")
    stack_region = next(region for region in fault_obj["memory"] if region["region_id"] == "stack")
    fault_stack = bytearray([0xA5] * STACK_SIZE)
    fault_stack[STACK_RSP - STACK_BASE:STACK_RSP - STACK_BASE + 2] = VALUE.to_bytes(2, "little")
    stack_region.update(guest_address=h(STACK_BASE), length=STACK_SIZE, initial_bytes_hex=fault_stack.hex(), comparison_mask_hex="ff" * STACK_SIZE)
    data = next((region for region in fault_obj["memory"] if region["region_id"] == "data"), None)
    if data is None:
        data = copy.deepcopy(stack_region)
        data["region_id"] = "data"
        fault_obj["memory"].append(data)
    data.update(guest_address=h(DATA_BASE), length=2, permissions={"read": True, "write": False, "execute": False}, initial_bytes_hex="a5" * 2, comparison_mask_hex="ff" * 2)
    outputs[f"{fault_id}.case.json"] = (json.dumps(fault_obj, indent=2) + "\n").encode()
    expected[fault_id] = {
        "exit": "exception", "exception": {"vector": 14, "error_code": 7, "linear_address": h(DATA_BASE), "fault_rip": h(CODE_BASE)},
        "memory": {region["region_id"]: region["initial_bytes_hex"] for region in fault_obj["memory"]},
    }
    outputs["s2_17l_pop_memory16_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17l word memory POP fixtures: " + ", ".join(stale))
        return 1
    print("6 success fixtures, 1 store-fault fixture, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
