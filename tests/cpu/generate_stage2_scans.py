#!/usr/bin/env python3
"""Materialize handwritten S2-15 BSF/BSR fixtures and answers."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_10_xchg_aliases.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    # Handwritten literals: case id, bytes, width, dst/src GPRs, source,
    # old destination, expected destination, expected RFLAGS.
    cases = [
        ("s2_15_bsf_16_zero", "660fbcc1", 16, 0, 1, 0, 0x1122334455667788, 0x1122334455667788, 0x446),
        ("s2_15_bsf_16_even", "660fbcc1", 16, 0, 1, 5, 0x1234, 0, 0x406),
        ("s2_15_bsf_16_high", "660fbcc1", 16, 0, 1, 0x8000, 0, 15, 0x402),
        ("s2_15_bsf_32_zero", "0fbcc1", 32, 0, 1, 0, 0xAABBCCDDEEFF0011, 0xAABBCCDDEEFF0011, 0x446),
        ("s2_15_bsf_32_rex_rb", "450fbcc1", 32, 8, 9, 0x80000000, 0xAABBCCDDEEFF0011, 31, 0x402),
        ("s2_15_bsf_32_alias", "0fbcc0", 32, 0, 0, 0x80000001, 0, 0, 0x406),
        ("s2_15_bsf_64_zero", "480fbcc1", 64, 0, 1, 0, 0x123456789ABCDEF0, 0x123456789ABCDEF0, 0x446),
        ("s2_15_bsf_64_low", "480fbcc1", 64, 0, 1, 1, 0, 0, 0x402),
        ("s2_15_bsf_64_high", "480fbcc1", 64, 0, 1, 0x8000000000000000, 0, 63, 0x402),
        ("s2_15_bsr_16_zero", "660fbdc1", 16, 0, 1, 0, 0x1122334455667788, 0x1122334455667788, 0x446),
        ("s2_15_bsr_16_low", "660fbdc1", 16, 0, 1, 1, 0, 0, 0x402),
        ("s2_15_bsr_16_high", "660fbdc1", 16, 0, 1, 0x8000, 0, 15, 0x402),
        ("s2_15_bsr_32_zero", "0fbdc1", 32, 0, 1, 0, 0xAABBCCDDEEFF0011, 0xAABBCCDDEEFF0011, 0x446),
        ("s2_15_bsr_32_rex_rb", "450fbdc1", 32, 8, 9, 0x80000000, 0xAABBCCDDEEFF0011, 31, 0x402),
        ("s2_15_bsr_32_low", "0fbdc1", 32, 0, 1, 1, 0, 0, 0x402),
        ("s2_15_bsr_64_zero", "480fbdc1", 64, 0, 1, 0, 0x123456789ABCDEF0, 0x123456789ABCDEF0, 0x446),
        ("s2_15_bsr_64_high_alias", "4d0fbdc9", 64, 9, 9, 0x8000000000000001, 0, 63, 0x406),
    ]

    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    for case_id, code_hex, _width, dst, src, source, old_dst, result, flags in cases:
        obj = copy.deepcopy(template)
        code = bytes.fromhex(code_hex)
        initial = list(obj["initial_state"]["gpr"])
        final = initial.copy()
        initial[dst] = f"0x{(source if dst == src else old_dst):016x}"
        initial[src] = f"0x{source:016x}"
        final[dst] = f"0x{result:016x}"
        if src != dst:
            final[src] = initial[src]
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = initial
        obj["initial_state"]["rflags"] = "0x0000000000000cd7"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": (
                "Handwritten S2-15 BSF/BSR register-form opcode bytes and independently "
                f"hand-calculated literal state; {case_id}. Materializer SHA256 {generator_hash}; "
                "no assembler or emulator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2A BSF pp. 3-107–3-108 and BSR pp. 3-109–3-110: "
                "16/32/64-bit forms, zero-source destination preservation, least/most-significant "
                "set-bit index, ZF, source-parity PF, cleared CF/OF/SF/AF, and REX.R/W; Vol.1 "
                "§3.4.1.1 (32-bit GPR write zero-extension). Older-processor caveat in the "
                "Vol.2A footnotes is outside the selected Intel 093 behavior."
            ),
            "specification_version": "Intel SDM revision 093; project S2-15",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        obj["stop_rip"] = f"0x{int(code_region['guest_address'], 16) + len(code):016x}"
        obj["instruction_budget"] = 1
        obj["rflags_mask"] = "0x0000000000000cd7"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
        expected[case_id] = {
            "gpr": final,
            "rflags": f"0x{flags:016x}",
            "rflags_defined": "0x0000000000000cd7",
            "memory": {
                region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]
            },
        }

    # Memory-source forms use the existing writable stack mapping. RBP points
    # at its midpoint, and a disp8 of zero keeps the effective address explicit
    # in the instruction bytes while exercising the normal guest read path.
    memory_cases = [
        ("s2_15_bsf_mem_16_zero", "660fbc4d00", 16, 0, 0xAABBCCDDEEFF0011, 0xAABBCCDDEEFF0011, 0x446),
        ("s2_15_bsf_mem_32_even", "0fbc4d00", 32, 3, 0x8877665544332211, 0, 0x406),
        ("s2_15_bsf_mem_64_high", "480fbc4d00", 64, 0x8000000000000000, 0, 63, 0x402),
        ("s2_15_bsr_mem_32_zero", "0fbd4d00", 32, 0, 0x1122334455667788, 0x1122334455667788, 0x446),
        ("s2_15_bsr_mem_16_high", "660fbd4d00", 16, 0x8000, 0x8877665544332211, 0x887766554433000F, 0x402),
        ("s2_15_bsr_mem_64_high", "480fbd4d00", 64, 0x8000000000000000, 0, 63, 0x402),
    ]
    for case_id, code_hex, width, source, old_dst, result, flags in memory_cases:
        obj = copy.deepcopy(template)
        code = bytes.fromhex(code_hex)
        stack = next(region for region in obj["memory"] if region["region_id"] == "stack")
        stack_bytes = bytearray.fromhex(stack["initial_bytes_hex"])
        source_bytes = source.to_bytes(width // 8, "little")
        stack_bytes[0x80 : 0x80 + len(source_bytes)] = source_bytes
        stack["initial_bytes_hex"] = stack_bytes.hex()
        initial = list(obj["initial_state"]["gpr"])
        final = initial.copy()
        initial[1] = f"0x{old_dst:016x}"
        final[1] = f"0x{result:016x}"
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = initial
        obj["initial_state"]["rflags"] = "0x0000000000000cd7"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": (
                "Handwritten S2-15 BSF/BSR memory-source opcode bytes and independently "
                f"hand-calculated literal state; {case_id}. Materializer SHA256 {generator_hash}; "
                "no assembler or emulator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2A BSF pp. 3-107–3-108 and BSR pp. 3-109–3-110: "
                "16/32/64-bit r/m source forms, zero-source destination preservation, "
                "least/most-significant set-bit index, ZF, source-parity PF, and cleared "
                "CF/OF/SF/AF; Vol.1 §3.4.1.1 (32-bit GPR write zero-extension)."
            ),
            "specification_version": "Intel SDM revision 093; project S2-15",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        obj["stop_rip"] = f"0x{int(code_region['guest_address'], 16) + len(code):016x}"
        obj["instruction_budget"] = 1
        obj["rflags_mask"] = "0x0000000000000cd7"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
        expected[case_id] = {
            "gpr": final,
            "rflags": f"0x{flags:016x}",
            "rflags_defined": "0x0000000000000cd7",
            "memory": {
                region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]
            },
        }
    files["s2_15_scans_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale BSF/BSR fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} BSF/BSR fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
