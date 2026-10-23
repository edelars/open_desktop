#!/usr/bin/env python3
"""Materialize handwritten S2-15 98/99 sign-extension cases and literals."""
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
        # CBW, CWDE, CDQE: each expected register value is an explicit literal.
        "s2_15_cbw": {
            "code": "6698",
            "initial": {0: "0x1122334455667780"},
            "final": {0: "0x112233445566ff80"},
        },
        "s2_15_cwde": {
            "code": "98",
            "initial": {0: "0x1122334400008001"},
            "final": {0: "0x00000000ffff8001"},
        },
        "s2_15_cdqe": {
            "code": "4898",
            "initial": {0: "0x0000000080000001"},
            "final": {0: "0xffffffff80000001"},
        },
        # CWD, CDQ, CQO: high-half destinations exercise 16/32/64-bit writes.
        "s2_15_cwd": {
            "code": "6699",
            "initial": {0: "0x1122334455668001", 2: "0xaabbccddeeff1122"},
            "final": {2: "0xaabbccddeeffffff"},
        },
        "s2_15_cdq": {
            "code": "99",
            "initial": {0: "0x1122334480000001", 2: "0xaabbccddeeff1122"},
            "final": {2: "0x00000000ffffffff"},
        },
        "s2_15_cqo": {
            "code": "4899",
            "initial": {0: "0x8000000000000001", 2: "0xaabbccddeeff1122"},
            "final": {2: "0xffffffffffffffff"},
        },
    }

    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    for case_id, spec in cases.items():
        obj = copy.deepcopy(template)
        code = bytes.fromhex(spec["code"])
        initial = list(obj["initial_state"]["gpr"])
        final = initial.copy()
        for index, value in spec["initial"].items():
            initial[index] = value
            final[index] = value
        for index, value in spec["final"].items():
            final[index] = value

        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = initial
        obj["initial_state"]["rflags"] = "0x0000000000000cd7"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": (
                "Handwritten S2-15 opcode bytes and independently hand-calculated literal state; "
                f"{case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator."
            ),
            "specification": (
                "Intel SDM revision 093, Vol.2A entries CBW/CWDE/CDQE and CWD/CDQ/CQO; "
                "Vol.1 §3.4.1.1 GPR aliases"
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
    files["s2_15_sign_extension_expected.json"] = (
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
        print("stale sign-extension fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} sign-extension fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
