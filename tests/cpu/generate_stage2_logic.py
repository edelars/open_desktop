#!/usr/bin/env python3
"""Package literal S2-12 answers and handwritten byte forms, without evaluating logic."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
HEX = lambda n: f"0x{n:016x}"
PREFIX = {8: b"", 16: b"\x66", 32: b"", 64: b"\x48"}

# Literal full-register input/output and defined flag answers. AF is undefined
# for binary logic/TEST and is excluded from their comparison/definedness masks.
ROWS = {
    "and": (0x20, 4, {
        8: (0x11223344556677ff, 0x1122334455667701, 0x402),
        16: (0x112233445566ffff, 0x1122334455660001, 0x402),
        32: (0x11223344ffffffff, 1, 0x402),
        64: (0xffffffffffffffff, 1, 0x402),
    }),
    "or": (0x08, 1, {
        8: (0x1122334455667780, 0x1122334455667781, 0x486),
        16: (0x1122334455668000, 0x1122334455668001, 0x482),
        32: (0x1122334480000000, 0x80000001, 0x482),
        64: (0x8000000000000000, 0x8000000000000001, 0x482),
    }),
    "xor": (0x30, 6, {
        8: (0x1122334455667701, 0x1122334455667700, 0x446),
        16: (0x1122334455660001, 0x1122334455660000, 0x446),
        32: (0x1122334400000001, 0, 0x446),
        64: (1, 0, 0x446),
    }),
}
NOT_ROWS = {
    8: (0x112233445566770f, 0x11223344556677f0, 0xcd7),
    16: (0x11223344556600ff, 0x112233445566ff00, 0xcd7),
    32: (0x1122334400ff00ff, 0xff00ff00, 0xcd7),
    64: (0x00ff00ff00ff00ff, 0xff00ff00ff00ff00, 0xcd7),
}
ZERO = {8: 0x1122334455667700, 16: 0x1122334455660000, 32: 0x1122334400000000, 64: 0}
ZERO_OUT = {8: 0x1122334455667700, 16: 0x1122334455660000, 32: 0, 64: 0}
ONES = {8: 0x11223344556677ff, 16: 0x112233445566ffff, 32: 0xffffffff, 64: 0xffffffffffffffff}
MIN = {8: 0x1122334455667780, 16: 0x1122334455668000, 32: 0x1122334480000000, 64: 0x8000000000000000}
MIN_OUT = {8: 0x1122334455667780, 16: 0x1122334455668000, 32: 0x80000000, 64: 0x8000000000000000}
MIN_SOURCE = {8: 0x80, 16: 0x8000, 32: 0x80000000, 64: 0x8000000000000000}


def materialize() -> dict[str, bytes]:
    files = {}
    expected = {}
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()

    def case(name, width, code, answer, form="rm_reg", source=1, op="and", count=1,
             custom_initial=None, custom_final=None, flags_in=0xcd7, defined=None):
        # Versioned S2-11 inputs provide only an explicit existing address layout
        # and canaries. Their bytes remain untouched and their hash is recorded.
        baseline = f"s2_11_{'neg' if op == 'not' else 'cmp' if op == 'test' else 'add'}_{width}_{form}.case.json"
        raw = (DEST / baseline).read_bytes()
        obj = json.loads(raw)
        case_id = "s2_12_" + name
        obj["case_id"] = case_id
        initial, final, flags = answer
        mask = (0xcd7 if op == "not" else 0xcc7) if defined is None else defined
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten {name} byte form and literal answer; no logic evaluator. "
                      f"Materializer SHA256 {generator_hash}; layout baseline {baseline} "
                      f"SHA256 {hashlib.sha256(raw).hexdigest()}. New exact input, not normalization.",
            "specification": "Intel SDM revision093 Vol.2A AND3-60..61; Vol.2B OR4-163..164,"
                             "NOT4-161..162,TEST4-721..722; Vol.2D XOR6-40..41; "
                             "Vol.1 AppendixA TableA-2 EFLAGS; Vol.2A sections2.1.5,2.2.1.6 ModR/M/REX",
            "specification_version": "Intel SDM revision093; project S2-12",
        }
        gpr = [int(v, 16) for v in obj["initial_state"]["gpr"]]
        gpr[0], gpr[1] = initial, source
        if custom_initial:
            for index, value in custom_initial.items():
                gpr[index] = value
        obj["initial_state"]["gpr"] = list(map(HEX, gpr))
        obj["initial_state"]["rflags"] = HEX(flags_in)
        obj["rflags_mask"] = HEX(mask)
        code_region = obj["memory"][0]
        code_region["initial_bytes_hex"] = code.hex()
        code_region["length"] = len(code)
        code_region["comparison_mask_hex"] = "ff" * len(code)
        obj["stop_rip"] = HEX(0x100000 + len(code))
        obj["instruction_budget"] = count
        is_memory = any(r["region_id"].startswith("data") for r in obj["memory"])
        to_register = form == "reg_mem" or form == "addr32" and width in (16, 64)
        if op in ("not", "test"):
            to_register = False
        mem_before = source if to_register else initial
        mem_after = mem_before if to_register or op == "test" else final
        memory_expected = {}
        access = 0x200ffe
        for region in obj["memory"]:
            if not region["region_id"].startswith("data"):
                continue
            region["permissions"]["write"] = not (to_register or op == "test")
            before = bytearray(b"\xa5" * region["length"])
            after = bytearray(before)
            base = int(region["guest_address"], 16)
            for offset, (old, new) in enumerate(zip(mem_before.to_bytes(8, "little")[:width // 8],
                                                    mem_after.to_bytes(8, "little")[:width // 8])):
                if base <= access + offset < base + len(before):
                    before[access + offset - base] = old
                    after[access + offset - base] = new
            region["initial_bytes_hex"] = before.hex()
            memory_expected[region["region_id"]] = after.hex()
        if not is_memory or to_register:
            gpr[0] = final
        if custom_final:
            for index, value in custom_final.items():
                gpr[index] = value
        expected[case_id] = {"gpr": list(map(HEX, gpr)), "rflags": HEX(flags),
                             "rflags_defined": HEX(mask), "memory": memory_expected}
        files[case_id + ".case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

    for op, (opcode, group, rows) in ROWS.items():
        for width, answer in rows.items():
            p, wide = PREFIX[width], int(width != 8)
            immediate = (1).to_bytes(1 if width == 8 else 2 if width == 16 else 4, "little")
            forms = {
                "rm_reg": p + bytes([opcode + wide, 0xc8]),
                "reg_rm": p + bytes([opcode + 2 + wide, 0xc1]),
                "mem_reg": p + bytes([opcode + wide, 0x0b]),
                "reg_mem": p + bytes([opcode + 2 + wide, 0x03]),
                "imm_reg": p + bytes([0x80 + wide, 0xc0 | group << 3]) + immediate,
                "imm_mem": p + bytes([0x80 + wide, 3 | group << 3]) + immediate,
                "accumulator": p + bytes([opcode + 4 + wide]) + immediate,
                "addr32": b"\x67" + p + bytes([opcode + wide if width in (8, 32) else opcode + 2 + wide,
                                                  0x0b if width in (8, 32) else 0x03]),
            }
            if width != 8:
                forms.update({"imm8_reg": p + bytes([0x83, 0xc0 | group << 3, 1]),
                              "imm8_mem": p + bytes([0x83, 3 | group << 3, 1])})
            for form, code in forms.items():
                case(f"{op}_{width}_{form}", width, code, answer, form, op=op)
            # AF remains unrequested; the other flags have independent zero and
            # negative-result boundaries, including distinct low-byte parity.
            if op == "and":
                boundary = (MIN[width], MIN_OUT[width], 0x482 if width == 8 else 0x486)
                source = MIN_SOURCE[width]
            elif op == "or":
                boundary, source = (ZERO[width], ZERO_OUT[width], 0x446), 0
            else:
                boundary = (MIN[width], MIN_OUT[width], 0x482 if width == 8 else 0x486)
                source = 0
            case(f"{op}_{width}_boundary", width, p + bytes([opcode + wide, 0xc8]),
                 boundary, source=source, op=op)
            if width != 8:
                negative = ((MIN[width], MIN_OUT[width], 0x486) if op == "and"
                            else (ZERO[width], ONES[width], 0x486))
                case(f"{op}_{width}_negative_imm8", width,
                     p + bytes([0x83, 0xc0 | group << 3, 0xff]), negative, op=op)
                if width == 64:
                    for form, code in [("imm_reg", p + bytes([0x81, 0xc0 | group << 3]) + b"\xff" * 4),
                                       ("accumulator", p + bytes([opcode + 5]) + b"\xff" * 4)]:
                        case(f"{op}_64_negative_{form}", width, code, negative, form, op=op)

    for width in (8, 16, 32, 64):
        p, wide = PREFIX[width], int(width != 8)
        answer = ZERO[width], ZERO[width], 0x446  # TEST never writes, even32-bit.
        immediate = (1).to_bytes(1 if width == 8 else 2 if width == 16 else 4, "little")
        for form, code in {
            "rm_reg": p + bytes([0x84 + wide, 0xc8]),
            "mem_reg": p + bytes([0x84 + wide, 0x0b]),
            "imm_reg": p + bytes([0xf6 + wide, 0xc0]) + immediate,
            "imm_mem": p + bytes([0xf6 + wide, 0x03]) + immediate,
            "accumulator": p + bytes([0xa8 + wide]) + immediate,
            "addr32": b"\x67" + p + bytes([0x84 + wide, 0x0b]),
        }.items():
            case(f"test_{width}_{form}", width, code, answer, form, op="test")
        case(f"test_{width}_negative", width, p + bytes([0x84 + wide, 0xc8]),
             (MIN[width], MIN[width], 0x482 if width == 8 else 0x486),
             source=MIN_SOURCE[width], op="test")
        for form, code in {"reg": p + bytes([0xf6 + wide, 0xd0]),
                           "mem": p + bytes([0xf6 + wide, 0x13]),
                           "addr32": b"\x67" + p + bytes([0xf6 + wide, 0x13])}.items():
            case(f"not_{width}_{form}", width, code, NOT_ROWS[width], form, op="not")
        case(f"not_{width}_cf0", width, p + bytes([0xf6 + wide, 0xd0]),
             (*NOT_ROWS[width][:2], 0xcd6), "reg", op="not", flags_in=0xcd6)
        case(f"not_{width}_flags_zero", width, p + bytes([0xf6 + wide, 0xd0]),
             (*NOT_ROWS[width][:2], 2), "reg", op="not", flags_in=2)
    for form, code in [("imm_reg", bytes.fromhex("48f7c0ffffffff")),
                       ("accumulator", bytes.fromhex("48a9ffffffff"))]:
        case(f"test_64_negative_{form}", 64, code,
             (MIN[64], MIN[64], 0x486), form, op="test")

    case("xor_al_ah", 8, bytes.fromhex("30e0"),
         (0x112233445566ff0f, 0x112233445566fff0, 0x486), op="xor")
    case("and_ah_al", 8, bytes.fromhex("20c4"),
         (0x112233445566ff0f, 0x1122334455660f0f, 0x406), op="and")
    case("or_spl_cl", 8, bytes.fromhex("4008cc"),
         (0, 0, 0x486), source=1, op="or",
         custom_initial={4: 0x300080}, custom_final={4: 0x300081})
    case("xor_r8d_r9d", 32, bytes.fromhex("4531c8"),
         (0, 0, 0x446), op="xor", custom_initial={8: 0x1122334400000001, 9: 1}, custom_final={8: 0})
    case("logic_mov_not_preserves_undefined_af", 64, bytes.fromhex("4831c04889c248f7d2"),
         (0, 0, 0x446), op="xor", count=3, custom_final={2: 0xffffffffffffffff})
    case("logic_add_restores_defined_af", 64, bytes.fromhex("4831c04883c001"),
         (0, 1, 0x402), op="xor", count=2, defined=0xcd7)
    files["s2_12_expected.json"] = (json.dumps(expected, indent=2, sort_keys=True) + "\n").encode()
    return files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    files = materialize()
    stale = sorted(p.name for p in DEST.glob("s2_12_*.case.json") if p.name not in files)
    bad = [name for name, data in files.items() if not (DEST / name).exists() or (DEST / name).read_bytes() != data]
    if stale or args.check and bad:
        print("Logic fixture mismatch: " + ", ".join(stale + bad))
        return 1
    if not args.check:
        for name, data in files.items():
            (DEST / name).write_bytes(data)
    print(f"S2-12: {len(files)-1} literal fixtures + expected sidecar; deterministic bytes OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
