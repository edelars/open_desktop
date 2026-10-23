#!/usr/bin/env python3
"""Materialize literal S2-15 immediate BT-family register fixtures."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"


# Each row is a handwritten input and full-register answer for BT/BTS/BTR/BTC.
# The materializer selects literals; it does not calculate instruction results.
VECTORS = [
    (16, "6641", 0, "0x112233445566ffff", "0x112233445566ffff", "0x112233445566ffff", "0x112233445566fffe", "0x112233445566fffe", 1),
    (16, "6641", 15, "0x112233445566ffff", "0x112233445566ffff", "0x112233445566ffff", "0x1122334455667fff", "0x1122334455667fff", 1),
    (16, "6641", 16, "0x1122334455660000", "0x1122334455660000", "0x1122334455660001", "0x1122334455660000", "0x1122334455660001", 0),
    (16, "6641", 255, "0x1122334455660000", "0x1122334455660000", "0x1122334455668000", "0x1122334455660000", "0x1122334455668000", 0),
    (32, "41", 0, "0x11223344ffffffff", "0x11223344ffffffff", "0x00000000ffffffff", "0x00000000fffffffe", "0x00000000fffffffe", 1),
    (32, "41", 31, "0x11223344ffffffff", "0x11223344ffffffff", "0x00000000ffffffff", "0x000000007fffffff", "0x000000007fffffff", 1),
    (32, "41", 32, "0x1122334400000000", "0x1122334400000000", "0x0000000000000001", "0x0000000000000000", "0x0000000000000001", 0),
    (32, "41", 255, "0x1122334400000000", "0x1122334400000000", "0x0000000080000000", "0x0000000000000000", "0x0000000080000000", 0),
    (64, "49", 0, "0xffffffffffffffff", "0xffffffffffffffff", "0xffffffffffffffff", "0xfffffffffffffffe", "0xfffffffffffffffe", 1),
    (64, "49", 63, "0xffffffffffffffff", "0xffffffffffffffff", "0xffffffffffffffff", "0x7fffffffffffffff", "0x7fffffffffffffff", 1),
    (64, "49", 64, "0x0000000000000000", "0x0000000000000000", "0x0000000000000001", "0x0000000000000000", "0x0000000000000001", 0),
    (64, "49", 255, "0x0000000000000000", "0x0000000000000000", "0x8000000000000000", "0x0000000000000000", "0x8000000000000000", 0),
]

KINDS = [
    ("bt", 4, "bt"),
    ("bts", 5, "bts"),
    ("btr", 6, "btr"),
    ("btc", 7, "btc"),
]


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_10_xchg_aliases.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    for width, prefix, immediate, source, bt, bts, btr, btc, carry in VECTORS:
        answers = {"bt": bt, "bts": bts, "btr": btr, "btc": btc}
        for name, group, answer_key in KINDS:
            case_id = f"s2_15_bti_{name}_{width}_imm_{immediate}"
            obj = copy.deepcopy(template)
            code = bytes.fromhex(f"{prefix}0fba{0xc0 | group << 3 | 1:02x}{immediate:02x}")
            initial = list(obj["initial_state"]["gpr"])
            final = initial.copy()
            initial[9] = source
            final[9] = answers[answer_key]
            obj["case_id"] = case_id
            obj["initial_state"]["gpr"] = initial
            obj["initial_state"]["rflags"] = "0x0000000000000cd7"
            obj["provenance"] = {
                "kind": "project_fixture",
                "source": (
                    "Handwritten S2-15 immediate BT-family bytes and independently "
                    f"hand-calculated literal state; {case_id}. Materializer SHA256 "
                    f"{generator_hash}; no assembler or emulator."
                ),
                "specification": (
            "Intel SDM revision 093, Vol.2A BT pp. 3-112–3-113, BTC pp. 3-114–3-115, "
            "BTR pp. 3-116–3-117, BTS pp. 3-118–3-119; immediate bit index is "
                    "selected within the register operand width; CF reports the old bit, "
                    "ZF is unaffected, OF/SF/AF/PF are undefined; Vol.1 §3.4.1.1 "
                    "(32-bit GPR write zero-extension)."
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
            flags = (0xCD7 & ~1) | carry
            files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
            expected[case_id] = {
                "gpr": final,
                "rflags": f"0x{flags:016x}",
                "rflags_defined": "0x0000000000000443",
                "memory": {
                    region["region_id"]: region["initial_bytes_hex"] for region in obj["memory"]
                },
            }
    files["s2_15_bitops_immediate_expected.json"] = (
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
        print("stale immediate BT-family fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} immediate BT-family fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
