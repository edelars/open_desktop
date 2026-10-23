#!/usr/bin/env python3
"""Materialize handwritten S2-16b SETcc fixtures and literal answers."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"

# Intel condition order: O, NO, B, AE, E, NE, BE, A, S, NS, P, NP, L, GE, LE, G.
CONDITIONS = [
    ("o", 0x800, 0), ("no", 0, 0x800), ("b", 1, 0), ("ae", 0, 1),
    ("e", 0x40, 0), ("ne", 0, 0x40), ("be", 0x41, 0), ("a", 0, 0x41),
    ("s", 0x80, 0), ("ns", 0, 0x80), ("p", 4, 0), ("np", 0, 4),
    ("l", 0x80, 0), ("ge", 0, 0x80), ("le", 0x40, 0), ("g", 0, 0x40),
]


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_10_xchg_aliases.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    # (id, code, flags, register index or None, initial byte, memory EA mode)
    cases: list[tuple[str, bytes, int, int | None, int, str | None]] = []
    for cc, (name, true_flags, false_flags) in enumerate(CONDITIONS):
        for truth, flags in ((True, true_flags), (False, false_flags)):
            value = int(truth)
            opcode = 0x90 + cc
            # The ModRM.reg field is ignored; vary it independently of cc.
            modrm_reg = (cc + int(truth) * 3) & 7
            cases.append((f"s2_16b_set{name}_{'true' if truth else 'false'}_al",
                          bytes((0x0f, opcode, 0xc0 | (modrm_reg << 3))), flags, 0, value, None))
            cases.append((f"s2_16b_set{name}_{'true' if truth else 'false'}_mem",
                          bytes((0x0f, opcode, 0x45 | (modrm_reg << 3), 0)), flags, None, value, "rbp"))

    # Byte aliases: legacy high byte registers, REX low-byte registers, and extended registers.
    extras = [
        ("ah", bytes.fromhex("0f94c4"), 0x40, 0, 1, None),
        ("ch", bytes.fromhex("0f94c5"), 0x40, 1, 1, None),
        ("dh", bytes.fromhex("0f94c6"), 0x40, 2, 1, None),
        ("bh", bytes.fromhex("0f94c7"), 0x40, 3, 1, None),
        ("spl", bytes.fromhex("400f94c4"), 0x40, 4, 1, None),
        ("bpl", bytes.fromhex("400f94c5"), 0x40, 5, 1, None),
        ("sil", bytes.fromhex("400f94c6"), 0x40, 6, 1, None),
        ("dil", bytes.fromhex("400f94c7"), 0x40, 7, 1, None),
        ("r8b", bytes.fromhex("410f94c0"), 0x40, 8, 1, None),
        ("r15b", bytes.fromhex("410f9fc7"), 0x40, 15, 0, None),
        ("rex_r_ignored", bytes.fromhex("440f94c0"), 0x40, 0, 1, None),
        ("rex_w_ignored", bytes.fromhex("480f94c0"), 0x40, 0, 1, None),
        ("operand_size_ignored", bytes.fromhex("660f94c0"), 0x40, 0, 1, None),
        ("address32", bytes.fromhex("670f9403"), 0x40, None, 1, "ebx"),
    ]
    for name, code, flags, reg, value, ea in extras:
        cases.append((f"s2_16b_set{name}", code, flags, reg, value, ea))
    # RIP-relative disp32 targets the byte at stack+0x80 after the full 7-byte instruction.
    rip_code = bytes.fromhex("0f9405") + (0x300080 - (0x100000 + 7)).to_bytes(4, "little")
    cases.append(("s2_16b_set_rip_relative", rip_code, 0x40, None, 1, "rip"))

    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    for case_id, code, flags, reg, value, ea in cases:
        obj = copy.deepcopy(template)
        initial = list(obj["initial_state"]["gpr"])
        final = initial.copy()
        if reg is not None:
            # The runner compares the full GPR. SETcc changes only the selected byte alias.
            old = int(initial[reg], 16)
            if reg < 4 and code[0] == 0x0f and (code[-1] & 7) >= 4:
                shift = 8
            else:
                shift = 0
            mask = 0xff << shift
            final[reg] = f"0x{((old & ~mask) | ((value & 0xff) << shift)):016x}"
        obj["case_id"] = case_id
        obj["initial_state"]["rflags"] = f"0x{(0x2 | flags):016x}"
        obj["initial_state"]["gpr"] = initial
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": ("Handwritten SETcc opcode bytes and independently hand-calculated literal state; "
                       f"{case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator."),
            "specification": (
                "Intel SDM revision 093, Vol.2B SETcc pp. 4-623–4-625: all 16 condition codes, "
                "byte destination 0/1, flags unaffected, ModRM.reg ignored; Vol.1 byte-register "
                "encoding rules for REX/high-byte aliases. Memory cases use one-byte ordinary stores."),
            "specification_version": "Intel SDM revision 093; project S2-16b",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        obj["stop_rip"] = f"0x{int(code_region['guest_address'], 16) + len(code):016x}"
        obj["instruction_budget"] = 1
        obj["rflags_mask"] = "0x0000000000000cd7"
        if ea:
            stack = next(region for region in obj["memory"] if region["region_id"] == "stack")
            stack_bytes = bytearray.fromhex(stack["initial_bytes_hex"])
            stack_bytes[0x80] = 0xA5
            stack["initial_bytes_hex"] = stack_bytes.hex()
            if ea == "ebx":
                initial[3] = final[3] = "0x0000000000300080"
            if ea == "rip":
                displacement = int.from_bytes(code[-4:], "little", signed=True)
                assert int(code_region["guest_address"], 16) + len(code) + displacement == 0x300080
            obj["initial_state"]["gpr"] = initial
        obj["expected_exit"] = "stop_address"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
        expected_memory = {}
        for region in obj["memory"]:
            contents = bytearray.fromhex(region["initial_bytes_hex"])
            if ea and region["region_id"] == "stack":
                contents[0x80] = value
            expected_memory[region["region_id"]] = contents.hex()
        expected[case_id] = {
            "gpr": final,
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": expected_memory,
        }
    files["s2_16b_setcc_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
    return files


def main() -> int:
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    generated = materialize()
    for name, content in generated.items():
        path = DEST / name
        if args.check:
            if not path.exists() or path.read_bytes() != content:
                stale.append(name)
        else:
            path.write_bytes(content)
    if stale:
        print("stale SETcc fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} SETcc fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
