#!/usr/bin/env python3
"""Materialize handwritten S2-14 byte fixtures and literal result boundaries.

This packages test data; it does not encode instructions or evaluate x86
arithmetic. Register outputs, defined flags, and memory are listed explicitly
below as independent expected values.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
HEX = lambda value: f"0x{value:016x}"

BASE_GPRS = [
    0x1122334455667788, 0x2233445566778899, 0x33445566778899AA,
    0x200000, 0x3000F8, 0x5566778899AABBCC, 0x66778899AABBCCDD,
    0x778899AABBCCDDEE, 0x8899AABBCCDDEEFF, 0x99AABBCCDDEEFF00,
    0xAABBCCDDEEFF0011, 0xBBCCDDEEFF001122, 0xCCDDEEFF00112233,
    0xDDEEFF0011223344, 0xEEFF001122334455, 0xFF00112233445566,
]
MULTIPLY_FLAGS = 0xC03  # CF, OF, DF, and the reserved bit 1.
DIVIDE_FLAGS = 0x402   # Only DF and the reserved bit 1 are defined.


def region(name: str, address: int, data: bytes, *, read: bool, write: bool,
           execute: bool) -> dict:
    return {
        "region_id": name,
        "guest_address": HEX(address),
        "length": len(data),
        "permissions": {"read": read, "write": write, "execute": execute},
        "initial_bytes_hex": data.hex(),
        "comparison_mask_hex": "ff" * len(data),
    }


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_10_xchg_aliases.case.json").read_bytes())
    template_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def case(name: str, code_hex: str, initial: dict[int, int], final: dict[int, int],
             defined: int, flags: int, data_value: bytes | None = None) -> None:
        case_id = "s2_14_" + name
        code = bytes.fromhex(code_hex)
        data = data_value if data_value is not None else bytes(8)
        # Keep construction explicit: the mappings are only fixture literals,
        # and no result is calculated from an operand here.
        gpr = BASE_GPRS.copy()
        for index, value in initial.items():
            gpr[index] = value
        output = gpr.copy()
        for index, value in final.items():
            output[index] = value

        obj = copy.deepcopy(template)
        obj["case_id"] = case_id
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": (
                "Handwritten S2-14 instruction bytes and independent literal expected state; "
                f"{name}. Materializer SHA256 {template_hash}. No assembler, emulator, or "
                "arithmetic reference evaluator."
            ),
            "specification": (
                "Intel SDM revision 093: Vol.2B MUL pp.4-140–4-141; Vol.2A IMUL "
                "pp.3-451–3-454, DIV pp.3-264–3-266, IDIV pp.3-448–3-450; "
                "Vol.2A §2.1.5 and §2.2.1.2 ModR/M, SIB, and REX; "
                "Vol.1 §3.4.1.1 GPR aliases"
            ),
            "specification_version": "Intel SDM revision 093; project S2-14",
        }
        obj["initial_state"]["gpr"] = list(map(HEX, gpr))
        obj["initial_state"]["rflags"] = HEX(0xCD7)
        obj["memory"] = [
            region("code", 0x100000, code, read=True, write=False, execute=True),
            region("data", 0x200000, data, read=True, write=False, execute=False),
            region("stack", 0x300000, bytes(256), read=True, write=True, execute=False),
        ]
        obj["stop_rip"] = HEX(0x100000 + len(code))
        obj["instruction_budget"] = 1
        obj["wall_timeout_ms"] = 2000
        obj["rflags_mask"] = HEX(defined)
        expected[case_id] = {
            "gpr": list(map(HEX, output)),
            "rflags": HEX(flags),
            "rflags_defined": HEX(defined),
            "memory": {"code": code.hex(), "data": data.hex(), "stack": bytes(256).hex()},
        }
        files[case_id + ".case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

    # Implicit unsigned MUL: RDX:RAX (or AX) receives the full product.
    case("mul_8", "f6e3", {0: 0x1122334455667713, 3: 0x200011},
         {0: 0x1122334455660143}, MULTIPLY_FLAGS, 0xC03)
    case("mul_16", "66f7e1", {0: 0x1122334455661234, 1: 0x10},
         {0: 0x1122334455662340, 2: 0x3344556677880001}, MULTIPLY_FLAGS, 0xC03)
    case("mul_32", "f7e1", {0: 0x1122334412345678, 1: 0x10},
         {0: 0x23456780, 2: 1}, MULTIPLY_FLAGS, 0xC03)
    case("mul_64", "48f7e1", {0: 0x123456789ABCDEF0, 1: 0x10},
         {0: 0x23456789ABCDEF00, 2: 1}, MULTIPLY_FLAGS, 0xC03)

    # Implicit signed IMUL: negative products fit exactly in the double width.
    case("imul_implicit_8", "f6eb", {0: 0x11223344556677FC, 3: 0x200003},
         {0: 0x112233445566FFF4}, MULTIPLY_FLAGS, 0x402)
    case("imul_implicit_16", "66f7e9", {0: 0x112233445566FFFC, 1: 3},
         {0: 0x112233445566FFF4, 2: 0x334455667788FFFF}, MULTIPLY_FLAGS, 0x402)
    case("imul_implicit_32", "f7e9", {0: 0x11223344FFFFFFFC, 1: 3},
         {0: 0xFFFFFFF4, 2: 0xFFFFFFFF}, MULTIPLY_FLAGS, 0x402)
    case("imul_implicit_64", "48f7e9", {0: 0xFFFFFFFFFFFFFFFC, 1: 3},
         {0: 0xFFFFFFFFFFFFFFF4, 2: 0xFFFFFFFFFFFFFFFF}, MULTIPLY_FLAGS, 0x402)

    # Two-operand IMUL: low product only, with both signed-fit outcomes.
    for width, prefix in ((16, "66"), (32, ""), (64, "48")):
        if width == 16:
            case(f"imul_2op_{width}_fits", prefix + "0fafc1",
                 {0: 0x112233445566FFFC, 1: 3}, {0: 0x112233445566FFF4},
                 MULTIPLY_FLAGS, 0x402)
            case(f"imul_2op_{width}_overflow", prefix + "0fafc1",
                 {0: 0x1122334455667FFF, 1: 2}, {0: 0x112233445566FFFE},
                 MULTIPLY_FLAGS, 0xC03)
        elif width == 32:
            case(f"imul_2op_{width}_fits", prefix + "0fafc1",
                 {0: 0x11223344FFFFFFFC, 1: 3}, {0: 0xFFFFFFF4},
                 MULTIPLY_FLAGS, 0x402)
            case(f"imul_2op_{width}_overflow", prefix + "0fafc1",
                 {0: 0x7FFFFFFF, 1: 2}, {0: 0xFFFFFFFE},
                 MULTIPLY_FLAGS, 0xC03)
        else:
            case(f"imul_2op_{width}_fits", prefix + "0fafc1",
                 {0: 0xFFFFFFFFFFFFFFFC, 1: 3}, {0: 0xFFFFFFFFFFFFFFF4},
                 MULTIPLY_FLAGS, 0x402)
            case(f"imul_2op_{width}_overflow", prefix + "0fafc1",
                 {0: 0x7FFFFFFFFFFFFFFF, 1: 2}, {0: 0xFFFFFFFFFFFFFFFE},
                 MULTIPLY_FLAGS, 0xC03)

    # Three-operand IMUL: 69 uses a width-sized immediate; 6B sign-extends imm8.
    for width, prefix, source, overflow_result in (
        (16, "66", 0x4000, 0x1122334455668000),
        (32, "", 0x40000000, 0x80000000),
        (64, "48", 0x4000000000000000, 0x8000000000000000),
    ):
        immediate = "0200" if width == 16 else "02000000"
        case(f"imul_imm_full_{width}", prefix + "69c1" + immediate,
             {1: source}, {0: overflow_result}, MULTIPLY_FLAGS, 0xC03)
        case(f"imul_imm8_signed_{width}", prefix + "6bc103",
             {1: 0xFFFFFFFFFFFFFFFC if width == 64 else
                 0x11223344FFFFFFFC if width == 32 else 0x112233445566FFFC},
             {0: 0xFFFFFFFFFFFFFFF4 if width == 64 else
                 0xFFFFFFF4 if width == 32 else 0x112233445566FFF4},
             MULTIPLY_FLAGS, 0x402)

    for width, prefix in ((16, "66"), (32, ""), (64, "48")):
        negative_full_imm = "ffff" if width == 16 else "ffffffff"
        negative_result = {
            16: 0x112233445566FFFD,
            32: 0xFFFFFFFD,
            64: 0xFFFFFFFFFFFFFFFD,
        }[width]
        case(f"imul_imm_negative_{width}", prefix + "69c1" + negative_full_imm,
             {1: 3}, {0: negative_result}, MULTIPLY_FLAGS, 0x402)
        negative_imm8_result = {
            16: 0x112233445566FFF4,
            32: 0xFFFFFFF4,
            64: 0xFFFFFFFFFFFFFFF4,
        }[width]
        case(f"imul_imm8_negative_{width}", prefix + "6bc1fc",
             {1: 3}, {0: negative_imm8_result}, MULTIPLY_FLAGS, 0x402)

    # DIV/IDIV normal quotients and signed remainders across every operand width.
    case("div_8", "f6f3", {0: 0x11223344556601F4, 3: 0x200007},
         {0: 0x1122334455660347}, DIVIDE_FLAGS, 0x402)
    case("div_16", "66f7f3", {0: 0x11223344556686A0, 2: 0x3344556677880001, 3: 300},
         {0: 0x112233445566014D, 2: 0x3344556677880064}, DIVIDE_FLAGS, 0x402)
    case("div_32", "f7f1", {0: 1_000_000, 2: 0, 1: 300},
         {0: 0xD05, 2: 100}, DIVIDE_FLAGS, 0x402)
    case("div_64", "48f7f1", {0: 0x0DE0B6B3A7640000, 2: 0, 1: 300_000},
         {0: 0x000003081A263555, 2: 0x186A0}, DIVIDE_FLAGS, 0x402)
    case("idiv_8", "f6fb", {0: 0x112233445566FE0C, 3: 0x200007},
         {0: 0x112233445566FDB9}, DIVIDE_FLAGS, 0x402)
    case("idiv_16", "66f7fb", {0: 0x1122334455667960, 2: 0x334455667788FFFE, 3: 0xFFFFFFFFFFFFFED4},
         {0: 0x112233445566014D, 2: 0x334455667788FF9C}, DIVIDE_FLAGS, 0x402)
    case("idiv_32", "f7f9", {0: 0x00000000FFF0BDC0, 2: 0xFFFFFFFF, 1: 0xFFFFFED4},
         {0: 0xD05, 2: 0xFFFFFF9C}, DIVIDE_FLAGS, 0x402)
    case("idiv_64", "48f7f9", {0: 0xF21F494C589C0000, 2: 0xFFFFFFFFFFFFFFFF,
                                1: 0xFFFFFFFFFFFB6C20},
         {0: 0x000003081A263555, 2: 0xFFFFFFFFFFFE7960}, DIVIDE_FLAGS, 0x402)

    # Positive physical memory reads: operands are never written by these forms.
    case("mul_64_mem", "48f723", {0: 0x10, 3: 0x200000},
         {0: 0x100, 2: 0}, MULTIPLY_FLAGS, 0x402,
         (16).to_bytes(8, "little"))
    case("div_64_mem", "48f733", {0: 1_000_000, 2: 0, 3: 0x200000},
         {0: 333_333, 2: 1}, DIVIDE_FLAGS, 0x402,
         (3).to_bytes(8, "little"))
    case("imul_64_mem", "480faf03", {0: 0xFFFFFFFFFFFFFFFC, 3: 0x200000},
         {0: 0xFFFFFFFFFFFFFFF4}, MULTIPLY_FLAGS, 0x402,
         (3).to_bytes(8, "little"))
    case("imul_imm8_64_mem", "486b0303", {3: 0x200000},
         {0: 0xFFFFFFFFFFFFFFF4}, MULTIPLY_FLAGS, 0x402,
         (0xFFFFFFFFFFFFFFFC).to_bytes(8, "little"))

    files["s2_14_expected.json"] = (
        json.dumps(expected, indent=2, sort_keys=True) + "\n"
    ).encode()
    return files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="verify exact deterministic checked-in bytes")
    args = parser.parse_args()
    files = materialize()
    stale = sorted(path.name for path in DEST.glob("s2_14_*.case.json")
                   if path.name not in files)
    if stale:
        print("Unexpected multiply/divide fixture files: " + ", ".join(stale))
        return 1
    if args.check:
        bad = [name for name, data in files.items()
               if not (DEST / name).exists() or (DEST / name).read_bytes() != data]
        if bad:
            print("Stale or missing multiply/divide fixtures: " + ", ".join(bad))
            return 1
        print(f"{len(files) - 1} multiply/divide fixtures and expected answers are current")
        return 0
    for name, data in files.items():
        (DEST / name).write_bytes(data)
    print(f"Wrote {len(files) - 1} multiply/divide fixtures and expected answers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
