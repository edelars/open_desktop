#!/usr/bin/env python3
"""Materialize literal S2-15 register-index BT-family memory fixtures."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"

# Handwritten offsets, selected element locations, bit positions, and literal
# old/new element values. Negative offsets exercise signed register offsets.
VECTORS = [
    (16, 0x0000, 0, 0, 0x0001, 0x0001, 0x0001, 0x0000, 0x0000),
    (16, 0x000f, 0, 15, 0x8000, 0x8000, 0x8000, 0x0000, 0x0000),
    (16, 0x0010, 2, 0, 0x0001, 0x0001, 0x0001, 0x0000, 0x0000),
    (16, 0xffff, -2, 15, 0x8000, 0x8000, 0x8000, 0x0000, 0x0000),
    (16, 0xfff0, -2, 0, 0x0001, 0x0001, 0x0001, 0x0000, 0x0000),
    (16, 0xffef, -4, 15, 0x8000, 0x8000, 0x8000, 0x0000, 0x0000),
    (32, 0x00000000, 0, 0, 0x00000001, 0x00000001, 0x00000001, 0x00000000, 0x00000000),
    (32, 0x0000001f, 0, 31, 0x80000000, 0x80000000, 0x80000000, 0x00000000, 0x00000000),
    (32, 0x00000020, 4, 0, 0x00000001, 0x00000001, 0x00000001, 0x00000000, 0x00000000),
    (32, 0xffffffff, -4, 31, 0x80000000, 0x80000000, 0x80000000, 0x00000000, 0x00000000),
    (32, 0xffffffe0, -4, 0, 0x00000001, 0x00000001, 0x00000001, 0x00000000, 0x00000000),
    (32, 0xffffffdf, -8, 31, 0x80000000, 0x80000000, 0x80000000, 0x00000000, 0x00000000),
    (64, 0x0000000000000000, 0, 0, 0x0000000000000001, 0x0000000000000001, 0x0000000000000001, 0x0000000000000000, 0x0000000000000000),
    (64, 0x000000000000003f, 0, 63, 0x8000000000000000, 0x8000000000000000, 0x8000000000000000, 0x0000000000000000, 0x0000000000000000),
    (64, 0x0000000000000040, 8, 0, 0x0000000000000001, 0x0000000000000001, 0x0000000000000001, 0x0000000000000000, 0x0000000000000000),
    (64, 0xffffffffffffffff, -8, 63, 0x8000000000000000, 0x8000000000000000, 0x8000000000000000, 0x0000000000000000, 0x0000000000000000),
    (64, 0xffffffffffffffc0, -8, 0, 0x0000000000000001, 0x0000000000000001, 0x0000000000000001, 0x0000000000000000, 0x0000000000000000),
    (64, 0xffffffffffffffbf, -16, 63, 0x8000000000000000, 0x8000000000000000, 0x8000000000000000, 0x0000000000000000, 0x0000000000000000),
]
KINDS = [("bt", 0xa3), ("bts", 0xab), ("btr", 0xb3), ("btc", 0xbb)]


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_15_bsf_mem_32_even.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    for width, offset, element_delta, bit, source, bt, bts, btr, btc in VECTORS:
        answers = {"bt": bt, "bts": bts, "btr": btr, "btc": btc}
        for name, opcode in KINDS:
            case_id = f"s2_15_btrm_{name}_{width}_off_{offset:x}"
            obj = copy.deepcopy(template)
            prefix = "6645" if width == 16 else ("4d" if width == 64 else "45")
            code = bytes.fromhex(f"{prefix}0f{opcode:02x}11")  # [R9], R10
            initial = list(obj["initial_state"]["gpr"])
            initial[9] = "0x0000000000300020"
            initial[10] = f"0x{offset:016x}"
            obj["case_id"] = case_id
            obj["initial_state"]["gpr"] = initial
            obj["initial_state"]["rflags"] = "0x0000000000000cd6"
            obj["provenance"] = {
                "kind": "project_fixture",
                "source": (
                    f"Handwritten S2-15 register-index BT-family memory bytes and literal "
                    f"old/new element answers; {case_id}. Materializer SHA256 {generator_hash}; "
                    "no assembler or emulator."
                ),
                "specification": (
                    "Intel SDM revision 093, Vol.2A BT pp. 3-112–3-113, BTC pp. 3-114–3-115, "
                    "BTR pp. 3-116–3-117, BTS pp. 3-118–3-119: memory bit-string register "
                    "offset is signed at operand width; signed DIV selects the element and "
                    "MOD selects the in-element bit; CF reports the old bit; ZF is "
                    "unaffected; OF/SF/AF/PF are undefined."
                ),
                "specification_version": "Intel SDM revision 093; project S2-15",
            }
            code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
            code_region["length"] = len(code)
            code_region["initial_bytes_hex"] = code.hex()
            code_region["comparison_mask_hex"] = "ff" * len(code)
            stack = next(region for region in obj["memory"] if region["region_id"] == "stack")
            stack_bytes = bytearray.fromhex(stack["initial_bytes_hex"])
            at = 0x20 + element_delta
            stack_bytes[at : at + width // 8] = source.to_bytes(width // 8, "little")
            stack["initial_bytes_hex"] = stack_bytes.hex()
            obj["stop_rip"] = f"0x{int(code_region['guest_address'], 16) + len(code):016x}"
            obj["instruction_budget"] = 1
            obj["rflags_mask"] = "0x0000000000000cd7"
            expected_value = answers[name]
            flags = 0xcd6 | (1 if source & (1 << bit) else 0)
            files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
            expected_memory = stack_bytes.copy()
            if name != "bt":
                expected_memory[at : at + width // 8] = expected_value.to_bytes(width // 8, "little")
            expected[case_id] = {
                "gpr": initial,
                "rflags": f"0x{flags:016x}",
                "rflags_defined": "0x0000000000000443",
                "memory": {"code": code.hex(), "stack": expected_memory.hex()},
            }
    files["s2_15_bitops_register_memory_expected.json"] = (
        json.dumps(expected, indent=2) + "\n"
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
        print("stale register-index BT-family memory fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} register-index BT-family memory fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
