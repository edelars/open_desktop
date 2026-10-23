#!/usr/bin/env python3
"""Materialize literal S2-16d indirect register JMP fixtures and answers."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
NONCANONICAL = 0x0000_8000_0000_0000
TARGET_VALUE = 0x8877_6655_4433_2211


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_16a_jmp_short_forward.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def add(case_id: str, code: bytes, target_register: int, target_delta: int,
            stop_delta: int | None, budget: int, fault: bool = False) -> None:
        obj = copy.deepcopy(template)
        initial = list(obj["initial_state"]["gpr"])
        base = int(obj["initial_state"]["rip"], 16)
        if fault:
            initial[target_register] = f"0x{NONCANONICAL:016x}"
        else:
            initial[target_register] = f"0x{base + target_delta:016x}"
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = initial
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 bytes and literal expected state; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM document 671199, revision 093, Vol.2A, JMP pp. 3-504–3-511: near indirect 64-bit target and canonicality pp. 3-506–3-507; target canonicality is checked before RIP commits.",
            "specification_version": "Intel SDM revision 093; project S2-16d",
        }
        region = next(r for r in obj["memory"] if r["region_id"] == "code")
        region["length"] = len(code)
        region["initial_bytes_hex"] = code.hex()
        region["comparison_mask_hex"] = "ff" * len(code)
        obj["stop_rip"] = None if stop_delta is None else f"0x{base + stop_delta:016x}"
        obj["instruction_budget"] = budget
        obj["expected_exit"] = "exception" if fault else "stop_address"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
        expected_gpr = list(initial)
        if not fault:
            expected_gpr[0] = f"0x{TARGET_VALUE:016x}"
        expected[case_id] = {
            "gpr": expected_gpr,
            "rip": f"0x{base:016x}" if fault else f"0x{base + stop_delta:016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": next(r["initial_bytes_hex"] for r in obj["memory"] if r["region_id"] == "stack")},
            "exit": "exception" if fault else "stop_address",
            "exception": {"vector": 13, "error_code": 0, "fault_rip": f"0x{base:016x}"} if fault else None,
            "instructions_executed": 0 if fault else budget,
        }

    for case_id, prefix, register, delta in (
        ("s2_16d_jmp_rax_target", bytes.fromhex("ffe0"), 0, 5),
        ("s2_16d_jmp_r8_target", bytes.fromhex("41ffe0"), 8, 6),
    ):
        target = bytes.fromhex("48b8") + TARGET_VALUE.to_bytes(8, "little")
        code = prefix + bytes.fromhex("909090") + target
        add(case_id, code, register, delta, len(code), 2)

    add("s2_16d_jmp_noncanonical", bytes.fromhex("ffe0"), 0, 0, None, 1, fault=True)
    files["s2_16d_indirect_jmp_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-16d indirect JMP fixtures: " + ", ".join(stale))
        return 1
    print("2 S2-16d target fixtures, 1 fault fixture, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
