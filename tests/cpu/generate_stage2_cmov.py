#!/usr/bin/env python3
"""Materialize handwritten S2-16c CMOVcc fixtures and literal answers."""
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
DATA = bytes.fromhex("123456789abcdef0")


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_10_xchg_aliases.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    cases: list[tuple[str, bytes, int, int, str | None]] = []
    for cc, (name, true_flags, false_flags) in enumerate(CONDITIONS):
        for truth, flags in ((True, true_flags), (False, false_flags)):
            cases.append((f"s2_16c_cmov{name}_{'true' if truth else 'false'}_reg",
                          bytes((0x0f, 0x40 + cc, 0xc3)), flags, 32, None))
            cases.append((f"s2_16c_cmov{name}_{'true' if truth else 'false'}_mem",
                          bytes((0x0f, 0x40 + cc, 0x45, 0)), flags, 32, "rbp"))

    # Width, REX register extension, 32-bit addressing, and RIP-relative source coverage.
    cases.extend([
        ("s2_16c_cmov_word_reg", bytes.fromhex("660f44c3"), 0x40, 16, None),
        ("s2_16c_cmov_qword_reg", bytes.fromhex("480f44c3"), 0x40, 64, None),
        ("s2_16c_cmov_extended_regs", bytes.fromhex("4d0f44c7"), 0x40, 64, None),
        ("s2_16c_cmov_word_mem", bytes.fromhex("660f444500"), 0x40, 16, "rbp"),
        ("s2_16c_cmov_address32", bytes.fromhex("670f4403"), 0x40, 32, "ebx"),
    ])
    rip_disp = 0x300080 - (0x100000 + 7)
    cases.append(("s2_16c_cmov_rip_relative",
                  bytes.fromhex("0f4405") + rip_disp.to_bytes(4, "little", signed=True),
                  0x40, 32, "rip"))

    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    for case_id, code, flags, width, ea in cases:
        obj = copy.deepcopy(template)
        initial = [int(value, 16) for value in obj["initial_state"]["gpr"]]
        final = initial.copy()
        if case_id.endswith("extended_regs"):
            dst_reg, src_reg = 8, 15
        else:
            dst_reg, src_reg = 0, 3
        if case_id == "s2_16c_cmov_word_reg":
            initial[src_reg] = 0x1234
            final[src_reg] = initial[src_reg]

        if ea:
            stack = next(region for region in obj["memory"] if region["region_id"] == "stack")
            stack_bytes = bytearray.fromhex(stack["initial_bytes_hex"])
            stack_bytes[0x80:0x80 + len(DATA)] = DATA
            stack["initial_bytes_hex"] = stack_bytes.hex()
            if ea == "ebx":
                initial[3] = 0x300080
                final[3] = initial[3]
            elif ea == "rip":
                assert 0x100000 + len(code) + rip_disp == 0x300080
            source_value = int.from_bytes(DATA[:width // 8], "little")
        else:
            if case_id.endswith("extended_regs"):
                initial[src_reg] = 0xFEDCBA9876543210
                final[src_reg] = initial[src_reg]
            source_value = initial[src_reg] & ((1 << width) - 1)

        # Standard cases encode their expected outcome in their case name and use
        # the condition's explicit truth-pair flags; extras above all set ZF.
        if "_true_" in case_id:
            condition_true = True
        elif "_false_" in case_id:
            condition_true = False
        else:
            condition_true = True
        if condition_true:
            mask = (1 << width) - 1
            if width == 16:
                final[dst_reg] = (initial[dst_reg] & ~mask) | source_value
            else:
                final[dst_reg] = source_value & mask
        elif width == 32:
            # A false 32-bit CMOV preserves DEST[31:0] and clears DEST[63:32].
            final[dst_reg] = initial[dst_reg] & 0xffff_ffff

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = [f"0x{value:016x}" for value in initial]
        obj["initial_state"]["rflags"] = f"0x{0x2 | flags:016x}"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": ("Handwritten CMOVcc opcode bytes and independently hand-calculated literal state; "
                       f"{case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator."),
            "specification": (
                "Intel SDM revision 093, Vol.2A CMOVcc p. 3-157: all 16 condition codes, "
                "register and ordinary memory source forms, source read on false condition, and "
                "operand-size behavior; Vol.1 long-mode register and address-size rules."),
            "specification_version": "Intel SDM revision 093; project S2-16c",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        obj["stop_rip"] = f"0x{int(code_region['guest_address'], 16) + len(code):016x}"
        obj["instruction_budget"] = 1
        obj["rflags_mask"] = "0x0000000000000cd7"
        obj["expected_exit"] = "stop_address"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()

        expected_memory = {
            region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]
        }
        expected[case_id] = {
            "gpr": [f"0x{value:016x}" for value in final],
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": expected_memory,
        }
    files["s2_16c_cmovcc_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale CMOVcc fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} CMOVcc fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
