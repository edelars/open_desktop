#!/usr/bin/env python3
"""Materialize literal S2-17t near RET prefix fixtures."""
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
RSP = STACK_BASE + 0x80
CASES = (
    ("s2_17t_ret66", "66c3", CODE_BASE + 2, "operand-size prefix"),
    (
        "s2_17t_ret66_ignored_rex",
        "4866c3",
        CODE_BASE + 3,
        "ineffective REX before legacy operand-size prefix",
    ),
)


def _fixture(
    template: dict,
    generator_hash: str,
    case_id: str,
    code_hex: str,
    target: int,
    form: str,
) -> tuple[str, bytes, str, bytes]:
    obj = copy.deepcopy(template)
    code = bytes.fromhex(code_hex)
    stack = bytearray([0xA5] * STACK_SIZE)
    stack_offset = RSP - STACK_BASE
    stack[stack_offset : stack_offset + 8] = target.to_bytes(8, "little")

    obj["case_id"] = case_id
    obj["provenance"] = {
        "kind": "project_fixture",
        "source": (
            f"Handwritten x86 {code.hex(' ').upper()} bytes ({form}) and independent literal "
            f"full-state answer; {case_id}. "
            f"Materializer SHA256 {generator_hash}; no assembler or emulator."
        ),
        "specification": (
            "Intel SDM revision 093 Vol.1 §6.4.7 pp.6-12–6-13: near branches in 64-bit mode "
            "are forced to 64-bit operation size and operand-size prefixes are silently ignored; "
            "https://cdrdv2.intel.com/v1/dl/getContent/671436. "
            "Vol.2A §2.2.1 p.2-8: REX is ignored unless it immediately precedes the opcode or 0F; "
            "https://cdrdv2.intel.com/v1/dl/getContent/671199. "
            "Vol.2B RET—Return From Procedure p.4-566: C3 near RET pops the return target."
        ),
        "specification_version": "Intel SDM revision 093; project S2-17t",
    }
    obj["initial_state"]["gpr"][4] = f"0x{RSP:016x}"
    obj["initial_state"]["rip"] = f"0x{CODE_BASE:016x}"
    obj["memory"] = [
        {
            "region_id": "code",
            "guest_address": f"0x{CODE_BASE:016x}",
            "length": len(code),
            "permissions": {"read": True, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(),
            "comparison_mask_hex": "ff" * len(code),
        },
        {
            "region_id": "stack",
            "guest_address": f"0x{STACK_BASE:016x}",
            "length": STACK_SIZE,
            "permissions": {"read": True, "write": True, "execute": False},
            "initial_bytes_hex": stack.hex(),
            "comparison_mask_hex": "ff" * STACK_SIZE,
        },
    ]
    obj["stop_rip"] = f"0x{target:016x}"
    obj["instruction_budget"] = 1
    obj["expected_exit"] = "stop_address"

    expected = {
        case_id: {
            "gpr": [*obj["initial_state"]["gpr"]],
            "rip": f"0x{target:016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": stack.hex()},
            "exit": "stop_address",
            "exception": None,
            "instructions_executed": 1,
        }
    }
    expected[case_id]["gpr"][4] = f"0x{RSP + 8:016x}"
    return (
        f"{case_id}.case.json",
        (json.dumps(obj, indent=2) + "\n").encode(),
        f"{case_id}_expected.json",
        (json.dumps(expected, indent=2) + "\n").encode(),
    )


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_17c_ret_noncanonical.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files = {}
    for case_id, code_hex, target, form in CASES:
        case_name, case_bytes, expected_name, expected_bytes = _fixture(
            template, generator_hash, case_id, code_hex, target, form
        )
        files[case_name] = case_bytes
        files[expected_name] = expected_bytes
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
        print("stale S2-17t RET fixture: " + ", ".join(stale))
        return 1
    print("2 S2-17t RET fixtures and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
