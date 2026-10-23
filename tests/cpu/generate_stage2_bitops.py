#!/usr/bin/env python3
"""Materialize handwritten S2-15 register-only BT-family cases and answers."""
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
        "s2_15_bt_16": ("660fa3c8", "0x112233445566a5a0", "0x000000000000fff7", "0x112233445566a5a0", 1),
        "s2_15_bt_32": ("0fa3c8", "0xaabbccdd12345678", "0xffffffff80000020", "0xaabbccdd12345678", 0),
        "s2_15_bt_64": ("480fa3c8", "0x8000000000000001", "0xffffffffffffffff", "0x8000000000000001", 1),
        "s2_15_bts_16": ("660fab c8".replace(" ", ""), "0x1122334455661234", "0x000000000000000f", "0x1122334455669234", 0),
        "s2_15_bts_32": ("0fabc8", "0xaabbccdd00000001", "0x000000000000001f", "0x0000000080000001", 0),
        "s2_15_bts_64": ("480fabc8", "0x0000000000000001", "0xffffffffffffffff", "0x8000000000000001", 0),
        "s2_15_btr_16": ("660fb3c8", "0x112233445566ffff", "0x0000000000000004", "0x112233445566ffef", 1),
        "s2_15_btr_32": ("0fb3c8", "0x1234567880000020", "0x0000000000000005", "0x0000000080000000", 1),
        "s2_15_btr_64": ("480fb3c8", "0x8000000000000001", "0x000000000000003f", "0x0000000000000001", 1),
        "s2_15_btc_16": ("660fbbc8", "0x1122334455668001", "0x000000000000ffff", "0x1122334455660001", 1),
        "s2_15_btc_32": ("0fbbc8", "0xaabbccdd80000000", "0x000000000000001f", "0x0000000000000000", 1),
        "s2_15_btc_64": ("480fbbc8", "0x0000000000000001", "0x0000000000000040", "0x0000000000000000", 1),
    }

    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}
    for case_id, (code_hex, base_value, offset_value, final_value, carry) in cases.items():
        obj = copy.deepcopy(template)
        code = bytes.fromhex(code_hex)
        initial = list(obj["initial_state"]["gpr"])
        final = initial.copy()
        initial[0] = base_value
        initial[1] = offset_value
        final[0] = final_value
        final[1] = offset_value
        obj["case_id"] = case_id
        obj["initial_state"]["gpr"] = initial
        obj["initial_state"]["rflags"] = "0x0000000000000cd7"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": (
                "Handwritten S2-15 BT/BTS/BTR/BTC register-form opcode bytes and independently "
                f"hand-calculated literal state; {case_id}. Materializer SHA256 {generator_hash}; "
                "no assembler or emulator."
            ),
            "specification": (
            "Intel SDM revision 093, Vol.2A BT pp. 3-112–3-113, BTC pp. 3-114–3-115, "
            "BTR pp. 3-116–3-117, BTS pp. 3-118–3-119; register bit-offset modulo "
                "operand width; CF defined, ZF unaffected, OF/SF/AF/PF undefined; "
                "Vol.1 §3.4.1.1 (32-bit GPR write zero-extension)"
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
    files["s2_15_bitops_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale BT-family fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} BT-family fixtures and expected answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
