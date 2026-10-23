#!/usr/bin/env python3
"""Materialize literal S2-17k 16-bit memory-source PUSH fixtures."""
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
SOURCE_WORD = 0xBEEF


def h(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    digest = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    outputs: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    cases = [
        ("base", bytes.fromhex("66ff30"), {0: DATA_BASE}, "data"),
        ("rex_base", bytes.fromhex("6641ff30"), {8: DATA_BASE}, "data"),
        ("sib", bytes.fromhex("66ff348b"), {3: DATA_BASE - 8, 1: 2}, "data"),
        ("rip_relative", None, {}, "data"),
        ("addr32", bytes.fromhex("6766ff30"), {0: 0x1_0020_0000}, "data"),
        ("rsp_source", bytes.fromhex("66ff3424"), {}, "stack"),
        ("read_fault", bytes.fromhex("66ff30"), {0: DATA_BASE}, "read_fault"),
        ("stack_fault", bytes.fromhex("66ff30"), {0: DATA_BASE}, "stack_fault"),
    ]
    for name, raw_code, registers, source_kind in cases:
        code = bytearray(raw_code or b"")
        if name == "rip_relative":
            next_rip = CODE_BASE + 7
            code.extend(b"\x66\xff\x35" + (DATA_BASE - next_rip).to_bytes(4, "little", signed=True))
        case_id = f"s2_17k_push_mem_{name}"
        obj = copy.deepcopy(template)
        gpr = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        gpr[4] = STACK_BASE + 1 if source_kind == "stack_fault" else STACK_RSP
        for register, value in registers.items():
            gpr[register] = value
        stack = bytearray([0xA5] * STACK_SIZE)
        source_address = DATA_BASE
        if source_kind == "stack":
            source_address = STACK_RSP
            stack[STACK_RSP - STACK_BASE:STACK_RSP - STACK_BASE + 2] = SOURCE_WORD.to_bytes(2, "little")
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [h(value) for value in gpr]
        obj["initial_state"]["rip"] = h(CODE_BASE)
        obj["initial_state"]["rflags"] = h(0x802)
        fault = source_kind in ("read_fault", "stack_fault")
        obj["stop_rip"] = None if fault else h(CODE_BASE + len(code))
        obj["instruction_budget"] = 1
        obj["expected_exit"] = "exception" if fault else "stop_address"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 66 FF /6 memory PUSH bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {digest}; no assembler or emulator.",
            "specification": "Intel SDM order number 253667-093US, Vol.2B PUSH pp. 4-522–4-525: r/m16 pushes a 16-bit item; source memory EA is computed before stack-pointer decrement (description, p. 4-523).",
            "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-17k",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region.update(guest_address=h(CODE_BASE), length=len(code), initial_bytes_hex=code.hex(), comparison_mask_hex="ff" * len(code))
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        stack_region.update(guest_address=h(STACK_BASE), length=STACK_SIZE, initial_bytes_hex=stack.hex(), comparison_mask_hex="ff" * STACK_SIZE)
        obj["memory"] = [region for region in obj["memory"] if region["region_id"] != "data"]
        if source_kind != "stack":
            obj["memory"].append({
                "region_id": "data", "guest_address": h(DATA_BASE), "length": 2,
                "permissions": {"read": source_kind != "read_fault", "write": False, "execute": False},
                "initial_bytes_hex": SOURCE_WORD.to_bytes(2, "little").hex(),
                "comparison_mask_hex": "ff" * 2,
            })
        outputs[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final_gpr = gpr.copy()
        final_rsp = gpr[4] - 2
        final_gpr[4] = final_rsp
        final_rip = CODE_BASE
        retired = 0
        if not fault:
            stack[final_rsp - STACK_BASE:final_rsp - STACK_BASE + 2] = SOURCE_WORD.to_bytes(2, "little")
            final_rip = CODE_BASE + len(code)
            retired = 1
        exception = None
        if fault:
            stack_fault = source_kind == "stack_fault"
            exception = {
                "vector": 14,
                "error_code": 6 if stack_fault else 5,
                "linear_address": h(STACK_BASE - 1) if stack_fault else h(source_address),
                "fault_rip": h(CODE_BASE),
            }
            final_gpr = gpr.copy()
        regions = {region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]}
        regions["stack"] = stack.hex()
        expected[case_id] = {
            "gpr": [h(value) for value in final_gpr], "rip": h(final_rip),
            "rflags": obj["initial_state"]["rflags"], "rflags_defined": "0x0000000000000cd7",
            "memory": regions, "exit": obj["expected_exit"], "exception": exception,
            "instructions_executed": retired,
        }
    outputs["s2_17k_push_memory16_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17k word memory PUSH fixtures: " + ", ".join(stale))
        return 1
    print("6 success fixtures, 2 injected-fault fixtures, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
