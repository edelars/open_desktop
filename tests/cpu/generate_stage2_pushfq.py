#!/usr/bin/env python3
"""Materialize literal S2-17p long-mode PUSHFQ fixture and answer."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x0010_0000
STACK_BASE = 0x0030_0000
STACK_SIZE = 256
STACK_RSP = STACK_BASE + 128
RFLAGS_MASK = 0x0000_0000_0000_0CD7
PUSH_IMAGE_MASK = 0x0000_0000_0003_0CD7


def h(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    case_id = "s2_17p_pushfq_masked_rflags_image"
    obj = copy.deepcopy(template)
    gpr = [int(value, 16) for value in obj["initial_state"]["gpr"]]
    gpr[4] = STACK_RSP
    code = bytes.fromhex("9c")
    obj["case_id"] = case_id
    obj["initial_state"]["gpr"] = [h(value) for value in gpr]
    obj["initial_state"]["rip"] = h(CODE_BASE)
    obj["initial_state"]["rflags"] = h(RFLAGS_MASK)
    obj["rflags_mask"] = h(RFLAGS_MASK)
    obj["stop_rip"] = h(CODE_BASE + len(code))
    obj["instruction_budget"] = 1
    obj["expected_exit"] = "stop_address"
    obj["provenance"] = {
        "kind": "project_fixture",
        "source": f"Handwritten bare PUSHFQ byte and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
        "specification": "Intel SDM revision 093, Vol.2B PUSHF/PUSHFD/PUSHFQ pp.4-528–4-529: in 64-bit mode PUSHFQ stores a qword RFLAGS image AND 00000000_00FCFFFFH; VM and RF are cleared in the image, and flags are not affected. The physical comparison mask covers modeled status flags, DF, fixed bit 1, and the required cleared RF/VM bits; IF/IOPL and other unmodeled bits are masked because the fixture runs in CPL3.",
        "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-17p",
    }

    code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
    code_region.update(
        guest_address=h(CODE_BASE),
        length=len(code),
        initial_bytes_hex=code.hex(),
        comparison_mask_hex="ff" * len(code),
    )
    stack = bytearray([0xA5] * STACK_SIZE)
    offset = STACK_RSP - STACK_BASE
    pushed = RFLAGS_MASK & 0x00FC_FFFF
    stack[offset - 8:offset] = pushed.to_bytes(8, "little")
    stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
    comparison = bytearray([0xFF] * STACK_SIZE)
    comparison[offset - 8:offset] = PUSH_IMAGE_MASK.to_bytes(8, "little")
    stack_region.update(
        guest_address=h(STACK_BASE),
        length=STACK_SIZE,
        initial_bytes_hex=bytes([0xA5] * STACK_SIZE).hex(),
        comparison_mask_hex=comparison.hex(),
    )
    obj["memory"] = [region for region in obj["memory"] if region["region_id"] != "data"]
    expected_stack = bytearray([0xA5] * STACK_SIZE)
    expected_stack[offset - 8:offset] = pushed.to_bytes(8, "little")
    final_gpr = gpr.copy()
    final_gpr[4] -= 8
    expected = {
        case_id: {
            "gpr": [h(value) for value in final_gpr],
            "rflags": h(RFLAGS_MASK),
            "rflags_defined": h(RFLAGS_MASK),
            "memory": {"code": code.hex(), "stack": expected_stack.hex()},
        }
    }
    return {
        f"{case_id}.case.json": (json.dumps(obj, indent=2) + "\n").encode(),
        "s2_17p_pushfq_expected.json": (json.dumps(expected, indent=2) + "\n").encode(),
    }


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
        print("stale S2-17p PUSHFQ fixtures: " + ", ".join(stale))
        return 1
    print("1 PUSHFQ fixture and literal answer are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
