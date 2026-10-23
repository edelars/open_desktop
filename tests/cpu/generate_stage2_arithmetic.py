#!/usr/bin/env python3
"""Materialize hand-authored S2-11 byte templates and literal answers.

This is test data packaging, not an arithmetic evaluator or encoder. No CPU
operation is evaluated here: input/output/flags boundary rows are literals.
Only integer byte packing, operand placement and file serialization are shared.
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

# width: (input full RAX, output full RAX, input flags, final flags).
# DF=1 and reserved bit1=1 are preserved. All six arithmetic flags are checked.
BINARY = {
    "add": (0x00, 0, 1, {
        8: (0x112233445566777f, 0x1122334455667780, 0xcd7, 0xc92),
        16: (0x1122334455667fff, 0x1122334455668000, 0xcd7, 0xc96),
        32: (0x112233447fffffff, 0x80000000, 0xcd7, 0xc96),
        64: (0x7fffffffffffffff, 0x8000000000000000, 0xcd7, 0xc96),
    }),
    "adc": (0x10, 2, 0, {
        8: (0x11223344556677ff, 0x1122334455667700, 0xcd7, 0x457),
        16: (0x112233445566ffff, 0x1122334455660000, 0xcd7, 0x457),
        32: (0x11223344ffffffff, 0, 0xcd7, 0x457),
        64: (0xffffffffffffffff, 0, 0xcd7, 0x457),
    }),
    "sub": (0x28, 5, 1, {
        8: (0x1122334455667700, 0x11223344556677ff, 0xcd7, 0x497),
        16: (0x1122334455660000, 0x112233445566ffff, 0xcd7, 0x497),
        32: (0x1122334400000000, 0xffffffff, 0xcd7, 0x497),
        64: (0, 0xffffffffffffffff, 0xcd7, 0x497),
    }),
    "sbb": (0x18, 3, 0, {
        8: (0x1122334455667780, 0x112233445566777f, 0xcd7, 0xc12),
        16: (0x1122334455668000, 0x1122334455667fff, 0xcd7, 0xc16),
        32: (0x1122334480000000, 0x7fffffff, 0xcd7, 0xc16),
        64: (0x8000000000000000, 0x7fffffffffffffff, 0xcd7, 0xc16),
    }),
    "cmp": (0x38, 7, 1, {
        8: (0x1122334455667701, 0x1122334455667701, 0xcd7, 0x446),
        16: (0x1122334455660001, 0x1122334455660001, 0xcd7, 0x446),
        32: (0x1122334400000001, 0x1122334400000001, 0xcd7, 0x446),
        64: (1, 1, 0xcd7, 0x446),
    }),
}
UNARY = {
    "inc": (0, {
        8: (0x112233445566777f, 0x1122334455667780, 0xcd7, 0xc93),
        16: (0x1122334455667fff, 0x1122334455668000, 0xcd7, 0xc97),
        32: (0x112233447fffffff, 0x80000000, 0xcd7, 0xc97),
        64: (0x7fffffffffffffff, 0x8000000000000000, 0xcd7, 0xc97),
    }),
    "dec": (1, {
        8: (0x1122334455667780, 0x112233445566777f, 0xcd6, 0xc12),
        16: (0x1122334455668000, 0x1122334455667fff, 0xcd6, 0xc16),
        32: (0x1122334480000000, 0x7fffffff, 0xcd6, 0xc16),
        64: (0x8000000000000000, 0x7fffffffffffffff, 0xcd6, 0xc16),
    }),
    "neg": (3, {
        8: (0x1122334455667780, 0x1122334455667780, 0xcd6, 0xc83),
        16: (0x1122334455668000, 0x1122334455668000, 0xcd6, 0xc87),
        32: (0x1122334480000000, 0x80000000, 0xcd6, 0xc87),
        64: (0x8000000000000000, 0x8000000000000000, 0xcd6, 0xc87),
    }),
}


def prefix(width: int) -> bytes:
    return {8: b"", 16: b"\x66", 32: b"", 64: b"\x48"}[width]


def packed(value: int, width: int) -> bytes:
    return value.to_bytes(8, "little")[:width // 8]


def region(name: str, address: int, value: bytes, writable: bool, executable=False) -> dict:
    return {"region_id": name, "guest_address": HEX(address), "length": len(value),
            "permissions": {"read": True, "write": writable, "execute": executable},
            "initial_bytes_hex": value.hex(), "comparison_mask_hex": "ff" * len(value)}


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    expected = {}
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    template = json.loads((DEST / "s2_10_xchg_aliases.case.json").read_bytes())

    def case(name, width, code, answer, source=0, memory=None, address32=False,
             readonly=False, split=False, register=0, custom_initial=None,
             custom_final=None):
        initial, final, flags_in, flags_out = answer
        obj = copy.deepcopy(template)
        case_id = "s2_11_" + name
        obj["case_id"] = case_id
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": "Handwritten single-arithmetic byte template and literal boundary answer; "
                      f"{name}, width{width}. Materializer SHA256 {generator_hash}. "
                      "No assembler, emulator or arithmetic reference evaluator. Exact addresses, "
                      "nonzero untouched memory canaries and complete final state are compared.",
            "specification": "Intel SDM 093 Vol.2A ADC3-9..11,ADD3-14..15,CMP3-161..162,"
                             "DEC3-262..263,INC3-457..458; Vol.2B NEG4-158..159,"
                             "SBB4-613..615,SUB4-685..686; Vol.1 Appendix A Table A-2 EFLAGS; "
                             "Vol.2A sections2.1.5,2.2.1.6 ModR/M and REX",
            "specification_version": "Intel SDM revision 093; project S2-11",
        }
        gpr = [int(v, 16) for v in template["initial_state"]["gpr"]]
        gpr[3] = 0x1122334400200ffe if address32 else 0x200ffe
        gpr[register] = initial
        if custom_initial:
            for index, value in custom_initial.items():
                gpr[index] = value
        gpr[1] = source
        obj["initial_state"]["gpr"] = list(map(HEX, gpr))
        obj["initial_state"]["rflags"] = HEX(flags_in)
        obj["memory"] = [region("code", 0x100000, code, False, True)]
        memory_expected = {}
        if memory is not None:
            before = bytearray(b"\xa5" * 24)
            before[8:8 + width // 8] = packed(memory, width)
            after = bytearray(before)
            if not readonly:
                after[8:8 + width // 8] = packed(final, width)
            if split:
                # Access at0x200ffe crosses logical regions and a host page.
                parts = [("data_left", 0x200ff6, before[:10], after[:10]),
                         ("data_right", 0x201000, before[10:], after[10:])]
            else:
                parts = [("data", 0x200ff6, before, after)]
            for rid, address, before_part, after_part in parts:
                obj["memory"].append(region(rid, address, before_part, not readonly))
                memory_expected[rid] = after_part.hex()
        obj["memory"].append(region("stack", 0x300000, bytes(256), True))
        obj["stop_rip"] = HEX(0x100000 + len(code))
        obj["instruction_budget"] = 1
        obj["wall_timeout_ms"] = 2000
        if memory is None or readonly:
            # CMP is read-only regardless of operand location; reg←mem cases
            # pass their literal destination result separately below.
            gpr[register] = final
        if custom_final:
            for index, value in custom_final.items():
                gpr[index] = value
        expected[case_id] = {"gpr": list(map(HEX, gpr)), "rflags": HEX(flags_out),
                             "memory": memory_expected}
        files[case_id + ".case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

    for op, (opcode, group, source, rows) in BINARY.items():
        for width, answer in rows.items():
            p = prefix(width)
            wide = int(width != 8)
            initial, final, flags_in, flags_out = answer
            for form, suffix in [("rm_reg", bytes([opcode + wide, 0xc8])),
                                 ("reg_rm", bytes([opcode + 2 + wide, 0xc1]))]:
                case(f"{op}_{width}_{form}", width, p + suffix, answer, source)
            case(f"{op}_{width}_mem_reg", width, p + bytes([opcode + wide, 0x0b]),
                 answer, source, initial, readonly=op == "cmp", split=True)
            # Reading a memory source never changes its bytes. The destination
            # literal result still updates RAX except for CMP.
            case(f"{op}_{width}_reg_mem", width, p + bytes([opcode + 2 + wide, 0x03]),
                 answer, source, source, readonly=True, split=True)
            immediate = packed(source, 8 if width == 8 else 16 if width == 16 else 32)
            for form, modrm in [("imm_reg", 0xc0 | group << 3), ("imm_mem", group << 3 | 3)]:
                mem = initial if form == "imm_mem" else None
                case(f"{op}_{width}_{form}", width, p + bytes([0x80 + wide, modrm]) + immediate,
                     answer, source, mem, readonly=op == "cmp", split=True)
            case(f"{op}_{width}_accumulator", width,
                 p + bytes([opcode + 4 + wide]) + immediate, answer, source)
            if width != 8:
                for form, modrm in [("imm8_reg", 0xc0 | group << 3), ("imm8_mem", group << 3 | 3)]:
                    case(f"{op}_{width}_{form}", width, p + bytes([0x83, modrm, source]),
                         answer, source, initial if form == "imm8_mem" else None,
                         readonly=op == "cmp", split=True)
            # Every family/width exercises address truncation. Direction
            # alternates; both memory directions above use address64.
            to_memory = width in (8, 32)
            code = b"\x67" + p + bytes([opcode + wide if to_memory else opcode + 2 + wide,
                                           0x0b if to_memory else 0x03])
            case(f"{op}_{width}_addr32", width, code, answer, source,
                 initial if to_memory else source, address32=True,
                 readonly=op == "cmp" or not to_memory, split=True)

    for op, (group, rows) in UNARY.items():
        for width, answer in rows.items():
            opcode = (0xf6 if op == "neg" else 0xfe) + int(width != 8)
            p = prefix(width)
            case(f"{op}_{width}_reg", width, p + bytes([opcode, 0xc0 | group << 3]), answer)
            case(f"{op}_{width}_mem", width, p + bytes([opcode, group << 3 | 3]),
                 answer, memory=answer[0], split=True)
            case(f"{op}_{width}_addr32", width, b"\x67" + p + bytes([opcode, group << 3 | 3]),
                 answer, memory=answer[0], address32=True, split=True)

    # Extra literal answers distinguish carry input0/1, carry/borrow from signed
    # overflow, AF from whole-width carry, odd/even parity and zero NEG.
    boundaries = [
        ("add_unsigned_zero", "add", 8, (0x11223344556677ff, 0x1122334455667700, 0xcd6, 0x457), 1),
        ("add_odd_parity", "add", 8, (0x1122334455667701, 0x1122334455667702, 0xcd7, 0x402), 1),
        ("adc_carry0_zero", "adc", 8, (0x1122334455667700, 0x1122334455667700, 0xcd6, 0x446), 0),
        ("adc_signed_overflow", "adc", 8, (0x112233445566777f, 0x1122334455667780, 0xcd7, 0xc92), 0),
        ("adc_auxiliary_only", "adc", 8, (0x112233445566770f, 0x112233445566771f, 0xcd7, 0x412), 0xf),
        ("sub_equal_zero", "sub", 8, (0x1122334455667701, 0x1122334455667700, 0xcd7, 0x446), 1),
        ("sub_auxiliary_only", "sub", 8, (0x1122334455667710, 0x112233445566770f, 0xcd7, 0x416), 1),
        ("sbb_carry0_zero", "sbb", 8, (0x1122334455667700, 0x1122334455667700, 0xcd6, 0x446), 0),
        ("sbb_borrow_zero", "sbb", 8, (0x1122334455667700, 0x1122334455667700, 0xcd7, 0x457), 0xff),
        ("cmp_borrow", "cmp", 8, (0x1122334455667700, 0x1122334455667700, 0xcd7, 0x497), 1),
    ]
    for name, op, width, answer, source in boundaries:
        opcode = BINARY[op][0]
        case(name, width, bytes([opcode, 0xc8]), answer, source)

    for width in (8, 16, 32, 64):
        for op in ("inc", "dec"):
            group, rows = UNARY[op]
            initial, final, flags_in, flags_out = rows[width]
            # Complement only CF: INC/DEC preserve either incoming value.
            answer = initial, final, flags_in ^ 1, flags_out ^ 1
            case(f"{op}_{width}_opposite_cf", width,
                 prefix(width) + bytes([0xfe + int(width != 8), 0xc0 | group << 3]), answer)
        initial = {8: 0x1122334455667700, 16: 0x1122334455660000,
                   32: 0x1122334400000000, 64: 0}[width]
        final = {8: 0x1122334455667700, 16: 0x1122334455660000, 32: 0, 64: 0}[width]
        case(f"neg_{width}_zero", width, prefix(width) + bytes([0xf6 + int(width != 8), 0xd8]),
             (initial, final, 0xcd7, 0x446))

        # Whole-width boundaries: all answers are selected literal table values,
        # not arithmetic recomputed from the inputs.
        signed_in, signed_out, _, signed_flags = BINARY["add"][3][width]
        min_in, max_out, _, overflow_flags = BINARY["sbb"][3][width]
        unsigned_in, zero_out, _, _ = BINARY["adc"][3][width]
        _, minus_one, _, _ = BINARY["sub"][3][width]
        one_in = BINARY["cmp"][3][width][0]
        extra = [
            ("add_unsigned_wrap", "add", (unsigned_in, zero_out, 0xcd6, 0x457), 1),
            ("adc_carry0", "adc", (initial, final, 0xcd6, 0x446), 0),
            ("adc_signed", "adc", (signed_in, signed_out, 0xcd7, signed_flags), 0),
            ("sbb_carry0", "sbb", (initial, final, 0xcd6, 0x446), 0),
            ("sub_signed", "sub", (min_in, max_out, 0xcd6, overflow_flags), 1),
            ("cmp_signed", "cmp", (min_in, min_in, 0xcd6, overflow_flags), 1),
        ]
        for name, op, answer, source in extra:
            case(f"{name}_{width}", width,
                 prefix(width) + bytes([BINARY[op][0] + int(width != 8), 0xc8]), answer, source)
        case(f"neg_{width}_one", width,
             prefix(width) + bytes([0xf6 + int(width != 8), 0xd8]),
             (one_in, minus_one, 0xcd6, 0x497))
        case(f"inc_{width}_unsigned_wrap", width,
             prefix(width) + bytes([0xfe + int(width != 8), 0xc0]),
             (unsigned_in, zero_out, 0xcd6, 0x456))
        case(f"dec_{width}_unsigned_borrow", width,
             prefix(width) + bytes([0xfe + int(width != 8), 0xc8]),
             (initial, minus_one, 0xcd7, 0x497))

    # Signed imm8 extension, with full-width literal results in every wide size.
    negative_rows = {
        16: (0x1122334455660000, 0x112233445566ffff, 0x1122334455660001),
        32: (0x1122334400000000, 0xffffffff, 1),
        64: (0, 0xffffffffffffffff, 1),
    }
    for width, (initial, negative, positive) in negative_rows.items():
        for op, (_, group, _, _) in BINARY.items():
            final, flags = {"add": (negative, 0x486), "adc": (initial if width == 16 else 0, 0x457),
                            "sub": (positive, 0x413), "sbb": (initial if width == 16 else 0, 0x457),
                            "cmp": (initial, 0x413)}[op]
            case(f"{op}_{width}_negative_imm8", width,
                 prefix(width) + bytes([0x83, 0xc0 | group << 3, 0xff]),
                 (initial, final, 0xcd7, flags))
            if width == 64:
                # imm32 must sign-extend to the 64-bit destination, both for
                # group81 and the dedicated accumulator opcode.
                case(f"{op}_{width}_negative_imm32_group", width,
                     b"\x48" + bytes([0x81, 0xc0 | group << 3]) + b"\xff" * 4,
                     (initial, final, 0xcd7, flags))
                case(f"{op}_{width}_negative_imm32_accumulator", width,
                     b"\x48" + bytes([BINARY[op][0] + 5]) + b"\xff" * 4,
                     (initial, final, 0xcd7, flags))

    # Aliases use literal full-register answers, including same physical
    # register operands. Native receives exact register values, no normalization.
    case("add_ah_al", 8, bytes.fromhex("00c4"),
         (0x1122334455667f01, 0x1122334455668001, 0xcd7, 0xc92))
    case("sub_al_ah", 8, bytes.fromhex("28e0"),
         (0x1122334455660100, 0x11223344556601ff, 0xcd7, 0x497))
    case("adc_spl_cl", 8, bytes.fromhex("4010cc"),
         (0x3000ff, 0x300000, 0xcd7, 0x457), register=4)
    case("add_r8d_r9d", 32, bytes.fromhex("4501c8"),
         (0x112233447fffffff, 0x80000000, 0xcd7, 0xc96), register=8,
         custom_initial={9: 1})

    files["s2_11_expected.json"] = (json.dumps(expected, indent=2, sort_keys=True) + "\n").encode()
    return files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify exact deterministic checked-in bytes")
    args = parser.parse_args()
    files = materialize()
    stale = sorted(path.name for path in DEST.glob("s2_11_*.case.json") if path.name not in files)
    if stale:
        print("Unexpected arithmetic fixture files: " + ", ".join(stale))
        return 1
    if args.check:
        bad = [name for name, data in files.items() if not (DEST / name).exists() or (DEST / name).read_bytes() != data]
        if bad:
            print("Outdated arithmetic fixture data: " + ", ".join(bad))
            return 1
    else:
        for name, data in files.items():
            (DEST / name).write_bytes(data)
    print(f"S2-11: {len(files) - 1} literal one-instruction fixtures + expected sidecar; deterministic bytes OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
