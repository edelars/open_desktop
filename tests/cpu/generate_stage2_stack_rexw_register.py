#!/usr/bin/env python3
"""Materialize literal S2-17n register stack fixtures for mixed prefixes."""
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
WORD_VALUE = 0x2468
QWORD_VALUE = 0x8877_6655_4433_2211


def h(value: int) -> str:
    return f"0x{value:016x}"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17a_push_rax.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    outputs: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    cases = [
        ("push_qword_rax", "664850", "push", 0, 8),
        ("push_qword_r8", "664950", "push", 8, 8),
        ("push_word_last_rex_clears_w_rax", "66484050", "push", 0, 2),
        ("pop_qword_r9", "664959", "pop", 9, 8),
        ("pop_word_last_rex_clears_w_r9", "6649414159", "pop", 9, 2),
        ("pop_word_last_rex_clears_w_rax", "66484058", "pop", 0, 2),
    ]

    for name, encoded, operation, register, width in cases:
        code = bytes.fromhex(encoded)
        case_id = f"s2_17n_{name}"
        obj = copy.deepcopy(template)
        gpr = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        gpr[4] = STACK_RSP
        value = QWORD_VALUE if width == 8 else WORD_VALUE
        if operation == "push":
            gpr[register] = value
        else:
            # POP to a 16-bit alias preserves the target's upper 48 bits.
            gpr[register] = 0xAABB_CCDD_EEFF_0011
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [h(item) for item in gpr]
        obj["initial_state"]["rip"] = h(CODE_BASE)
        obj["initial_state"]["rflags"] = h(0x802)
        obj["stop_rip"] = h(CODE_BASE + len(code))
        obj["instruction_budget"] = 1
        obj["expected_exit"] = "stop_address"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 register PUSH/POP mixed-prefix bytes and independent literal full-state answer; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM revision 093, Vol.2A §2.2.1.2 pp. 2-8–2-9: REX.W=1 overrides 66H for non-byte operands; only the final REX prefix immediately before the opcode is effective. Vol.2B PUSH pp. 4-522–4-523 and POP pp. 4-398–4-399 specify stack operand behavior.",
            "specification_version": "Intel SDM Vol.2A/2B revision 093 (253666-093US/253667-093US); project S2-17n",
        }

        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region.update(guest_address=h(CODE_BASE), length=len(code), initial_bytes_hex=code.hex(), comparison_mask_hex="ff" * len(code))
        stack = bytearray([0xA5] * STACK_SIZE)
        if operation == "pop":
            stack[STACK_RSP - STACK_BASE:STACK_RSP - STACK_BASE + width] = value.to_bytes(width, "little")
        stack_region = next(region for region in obj["memory"] if region["region_id"] == "stack")
        stack_region.update(guest_address=h(STACK_BASE), length=STACK_SIZE, initial_bytes_hex=stack.hex(), comparison_mask_hex="ff" * STACK_SIZE)
        obj["memory"] = [region for region in obj["memory"] if region["region_id"] != "data"]
        outputs[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        final_gpr = gpr.copy()
        if operation == "push":
            final_gpr[4] -= width
            stack[final_gpr[4] - STACK_BASE:final_gpr[4] - STACK_BASE + width] = value.to_bytes(width, "little")
        else:
            final_gpr[register] = ((gpr[register] & ~((1 << (width * 8)) - 1)) | value)
            final_gpr[4] += width
        expected[case_id] = {
            "gpr": [h(item) for item in final_gpr],
            "rflags": obj["initial_state"]["rflags"], "rflags_defined": "0x0000000000000cd7",
            "memory": {"stack": stack.hex()},
        }

    outputs["s2_17n_register_stack_rexw_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-17n register stack fixtures: " + ", ".join(stale))
        return 1
    print("6 mixed-prefix register PUSH/POP fixtures and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
