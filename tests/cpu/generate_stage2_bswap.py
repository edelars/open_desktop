#!/usr/bin/env python3
"""Materialize handwritten S2-15 BSWAP register cases and literal answers."""
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
    cases = {
        "s2_15_bswap_eax": {
            "code": "0fc8",
            "register": 0,
            "initial": "0x11223344a1b2c3d4",
            "final": "0x00000000d4c3b2a1",
        },
        "s2_15_bswap_edi": {
            "code": "0fcf",
            "register": 7,
            "initial": "0x8877665544332211",
            "final": "0x0000000011223344",
        },
        "s2_15_bswap_r8d": {
            "code": "410fc8",
            "register": 8,
            "initial": "0x0123456789abcdef",
            "final": "0x00000000efcdab89",
        },
        "s2_15_bswap_r15d": {
            "code": "410fcf",
            "register": 15,
            "initial": "0xfedcba9876543210",
            "final": "0x0000000010325476",
        },
        "s2_15_bswap_rax": {
            "code": "480fc8",
            "register": 0,
            "initial": "0x11223344a1b2c3d4",
            "final": "0xd4c3b2a144332211",
        },
        "s2_15_bswap_r9": {
            "code": "490fc9",
            "register": 9,
            "initial": "0x0123456789abcdef",
            "final": "0xefcdab8967452301",
        },
        "s2_15_bswap_66_rexw_r9": {
            "code": "66490fc9",
            "register": 9,
            "initial": "0x0123456789abcdef",
            "final": "0xefcdab8967452301",
        },
    }

    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    for case_id, spec in cases.items():
        obj = copy.deepcopy(template)
        code = bytes.fromhex(spec["code"])
        initial = list(obj["initial_state"]["gpr"])
        final = initial.copy()
        initial[spec["register"]] = spec["initial"]
        final[spec["register"]] = spec["final"]
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = initial
        obj["initial_state"]["rflags"] = "0x0000000000000cd7"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": (
                "Handwritten S2-15 BSWAP opcode bytes and independently hand-calculated literal state; "
                f"{case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2A BSWAP entry (opcode 0F C8+rd, r32/r64, "
                "undefined 16-bit result, REX.W and REX.B behavior); Vol.1 §3.4.1.1 "
                "(32-bit GPR write zero-extension)"
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
            "rflags": "0x0000000000000cd7",
            "rflags_defined": "0x0000000000000cd7",
            "memory": {
                region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]
            },
        }
    files["s2_15_bswap_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale BSWAP fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} BSWAP fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
