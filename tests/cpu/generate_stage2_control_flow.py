#!/usr/bin/env python3
"""Materialize literal S2-16a direct relative branch fixtures and answers."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"

# Per Intel condition-code order, each pair is (flags satisfying, flags failing).
# The values use architectural RFLAGS bit positions CF/PF/ZF/SF/OF.
CODES = [
    ("o", 0x800, 0), ("no", 0, 0x800),
    ("b", 1, 0), ("ae", 0, 1),
    ("e", 0x40, 0), ("ne", 0, 0x40),
    ("be", 0x40, 0), ("a", 0, 1),
    ("s", 0x80, 0), ("ns", 0, 0x80),
    ("p", 4, 0), ("np", 0, 4),
    ("l", 0x80, 0), ("ge", 0, 0x80),
    ("le", 0x40, 0), ("g", 0, 0x40),
]


def materialize() -> dict[str, bytes]:
    template = json.loads((DEST / "s2_15_bsf_16_zero.case.json").read_bytes())
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    files: dict[str, bytes] = {}
    expected: dict[str, dict] = {}

    def add(case_id: str, code: bytes, flags: int, target_delta: int, spec: str) -> None:
        obj = copy.deepcopy(template)
        initial = list(obj["initial_state"]["gpr"])
        obj["case_id"] = case_id
        obj["initial_state"]["rflags"] = f"0x{(0xcd7 & ~0x8d5 | flags):016x}"
        obj["provenance"] = {
            "kind": "project_fixture",
            "source": f"Handwritten x86 branch bytes and literal expected state; {case_id}. Materializer SHA256 {generator_hash}; no assembler or emulator.",
            "specification": f"Intel SDM revision 093, Vol.2A Jcc pp. 3-499–3-502 and JMP pp. 3-504–3-505: {spec}; condition flags CF/PF/ZF/SF/OF; displacement is signed relative to the complete instruction's next RIP; long-mode relative target is 64-bit.",
            "specification_version": "Intel SDM revision 093; project S2-16a",
        }
        code_region = next(region for region in obj["memory"] if region["region_id"] == "code")
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        base = int(obj["initial_state"]["rip"], 16)
        obj["stop_rip"] = f"0x{(base + len(code) + target_delta):016x}"
        obj["instruction_budget"] = 1
        obj["rflags_mask"] = "0x0000000000000cd7"
        files[f"{case_id}.case.json"] = (json.dumps(obj, indent=2) + "\n").encode()
        expected[case_id] = {
            "gpr": initial,
            "rflags": obj["initial_state"]["rflags"],
            "rflags_defined": "0x0000000000000cd7",
            "memory": {"code": code.hex(), "stack": next(r["initial_bytes_hex"] for r in obj["memory"] if r["region_id"] == "stack")},
        }

    for encoding, (name, true_flags, false_flags) in enumerate(CODES):
        for mode in ("short", "near"):
            if mode == "short":
                prefix = bytes([0x70 + encoding])
                displacement = bytes([2])
            else:
                prefix = bytes([0x0f, 0x80 + encoding])
                displacement = bytes([2, 0, 0, 0])
            code = prefix + displacement
            length = len(code)
            for taken, flags in ((True, true_flags), (False, false_flags)):
                case_id = f"s2_16a_j{name}_{mode}_{'taken' if taken else 'fallthrough'}"
                add(case_id, code, flags, 2 if taken else 0,
                    f"{mode} J{name} {('takes' if taken else 'does not take')} its branch.")

    for name, code, delta in (
        ("jmp_short_forward", bytes.fromhex("eb02"), 2),
        ("jmp_short_backward", bytes.fromhex("ebfd"), -3),
        ("jmp_near_forward", bytes.fromhex("e902000000"), 2),
        ("jmp_near_backward", bytes.fromhex("e9faffffff"), -6),
    ):
        add(f"s2_16a_{name}", code, 0xcd7, delta,
            "direct relative JMP sign-extends rel8 or rel32 and adds it to next RIP")

    files["s2_16a_control_flow_expected.json"] = (json.dumps(expected, indent=2) + "\n").encode()
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
        print("stale S2-16a control-flow fixtures: " + ", ".join(stale))
        return 1
    print(f"{len(generated) - 1} S2-16a direct-control-flow fixtures and answers are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
