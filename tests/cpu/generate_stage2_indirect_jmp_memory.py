#!/usr/bin/env python3
"""Materialize literal S2-16e indirect memory JMP fixtures and answers."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
DATA_BASE = 0x0020_0000
CODE_BASE = 0x0010_0000
TARGET_VALUE = 0x8877_6655_4433_2211
NONCANONICAL = 0x0000_8000_0000_0000


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_16d_jmp_rax_target.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def add(case_id: str, code: bytes, registers: dict[int, int],
            data: bytes | None = None, read_fault: bool = False,
            noncanonical_fault: bool = False) -> None:
        obj = copy.deepcopy(template)
        initial = list(obj["initial_state"]["gpr"])
        base = int(obj["initial_state"]["rip"], 16)
        for register, value in registers.items():
            initial[register] = f"0x{value:016x}"
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = initial
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 bytes and literal expected state; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": "Intel SDM document 671199, revision 093, Vol.2A, JMP pp. 3-504–3-511: near indirect 64-bit source and target; effective address follows the selected existing long-mode address profile.",
            "specification_version": "Intel SDM revision 093; project S2-16e",
        }
        code_region = next(r for r in obj["memory"] if r["region_id"] == "code")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        obj["memory"] = [r for r in obj["memory"] if r["region_id"] != "data"]
        if data is not None:
            obj["memory"].append({
                "region_id": "data",
                "guest_address": f"0x{DATA_BASE:016x}",
                "length": len(data),
                "permissions": {"read": not read_fault, "write": False, "execute": False},
                "initial_bytes_hex": data.hex(),
                "comparison_mask_hex": "ff" * len(data),
            })
        fault = read_fault or noncanonical_fault
        obj["stop_rip"] = None if fault else f"0x{base + len(code):016x}"
        obj["instruction_budget"] = 1 if fault else 2
        obj["expected_exit"] = "exception" if fault else "stop_address"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        expected_gpr = list(initial)
        if not fault:
            expected_gpr[0] = f"0x{TARGET_VALUE:016x}"
        region_answers = {r["region_id"]: r["initial_bytes_hex"] for r in obj["memory"]}
        if noncanonical_fault:
            exception = {"vector": 13, "error_code": 0, "linear_address": None, "fault_rip": f"0x{base:016x}"}
        elif read_fault:
            exception = {"vector": 14, "error_code": 5, "linear_address": f"0x{DATA_BASE:016x}", "fault_rip": f"0x{base:016x}"}
        else:
            exception = None
        expected[case_id] = {
            "gpr": expected_gpr,
            "rip": f"0x{base:016x}" if fault else f"0x{base + len(code):016x}",
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": region_answers,
            "exit": "exception" if fault else "stop_address",
            "exception": exception,
            "instructions_executed": 0 if fault else 2,
        }

    def code_to_target(prefix: bytes, target_delta: int) -> bytes:
        jump = prefix
        mov = bytes.fromhex("48b8") + TARGET_VALUE.to_bytes(8, "little")
        padding = bytes.fromhex("90") * (target_delta - len(jump))
        return jump + padding + mov

    ptr = lambda address: address.to_bytes(8, "little")
    data_base = ptr(CODE_BASE + 5)
    add("s2_16e_jmp_mem_base", code_to_target(bytes.fromhex("ff20"), 5), {0: DATA_BASE}, data_base)
    add("s2_16e_jmp_mem_rex_base", code_to_target(bytes.fromhex("41ff20"), 6), {8: DATA_BASE}, ptr(CODE_BASE + 6))
    sib_data = bytearray(32)
    sib_data[24:32] = ptr(CODE_BASE + 7)
    add("s2_16e_jmp_mem_sib_disp", code_to_target(bytes.fromhex("ff648810"), 7), {0: DATA_BASE, 1: 2}, bytes(sib_data))

    base = int(template["initial_state"]["rip"], 16)
    inline_pointer = ptr(base + 14)
    mov = bytes.fromhex("48b8") + TARGET_VALUE.to_bytes(8, "little")
    add("s2_16e_jmp_mem_rip_relative", bytes.fromhex("ff2500000000") + inline_pointer + mov, {})
    add("s2_16e_jmp_mem_addr32", code_to_target(bytes.fromhex("67ff20"), 6), {0: 0x1_0020_0000}, ptr(CODE_BASE + 6))

    fault_data = ptr(CODE_BASE + 6)
    add("s2_16e_jmp_mem_read_fault", bytes.fromhex("ff20"), {0: DATA_BASE}, fault_data, read_fault=True)
    add("s2_16e_jmp_mem_noncanonical", bytes.fromhex("ff20"), {0: DATA_BASE}, ptr(NONCANONICAL), noncanonical_fault=True)
    files["s2_16e_indirect_jmp_memory_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-16e indirect memory JMP fixtures: " + ", ".join(stale))
        return 1
    print("5 S2-16e target fixtures, 2 fault fixtures, and literal answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
