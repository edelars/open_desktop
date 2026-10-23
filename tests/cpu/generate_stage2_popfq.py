#!/usr/bin/env python3
"""Materialize literal S2-17o long-mode POPFQ fixtures and answers."""
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
STACK_RSP = STACK_BASE + 120
PROFILE_FLAGS = 0x0CD5  # CF/PF/AF/ZF/SF/OF plus DF; bit 1 is fixed.


def h(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    outputs: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    cases = [
        ("loads_status_flags_and_df", PROFILE_FLAGS | (3 << 12), 0x2),
        ("clears_status_flags_and_df", 0, 0xCD7),
    ]

    for name, stack_flags, initial_flags in cases:
        case_id = f"s2_17o_{name}"
        obj = copy.deepcopy(template)
        gpr = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        gpr[4] = STACK_RSP
        code = bytes.fromhex("9d")
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [h(value) for value in gpr]
        obj["initial_state"]["rip"] = h(CODE_BASE)
        obj["initial_state"]["rflags"] = h(initial_flags)
        obj["stop_rip"] = h(CODE_BASE + len(code))
        obj["instruction_budget"] = 1
        obj["expected_exit"] = "stop_address"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten bare POPFQ byte and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM revision 093, Vol.2B POPF pp.4-407–4-410: long-mode POPFQ reads 64 bits; at CPL3/IOPL0 the status flags and DF may be loaded while IF/IOPL remain unchanged. The project fixture format compares its six status flags, DF, and fixed RFLAGS bit 1. Stack value keeps TF and AC clear.",
            "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-17o",
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
        stack[offset:offset + 8] = stack_flags.to_bytes(8, "little")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        stack_region.update(
            guest_address=h(STACK_BASE),
            length=STACK_SIZE,
            initial_bytes_hex=stack.hex(),
            comparison_mask_hex="ff" * STACK_SIZE,
        )
        obj["memory"] = [region for region in obj["memory"] if region["region_id"] != "data"]
        outputs[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final_gpr = gpr.copy()
        final_gpr[4] += 8
        modeled = stack_flags & PROFILE_FLAGS
        final_flags = (initial_flags & ~PROFILE_FLAGS) | modeled
        expected[case_id] = {
            "gpr": [h(value) for value in final_gpr],
            "rflags": h(final_flags),
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": stack.hex()},
        }

    outputs["s2_17o_popfq_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17o POPFQ fixtures: " + ", ".join(stale))
        return 1
    print("2 POPFQ fixtures and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
