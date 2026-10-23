#!/usr/bin/env python3
"""Package the checked S2-13 answer table into schema-v2 fixtures."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / "tests/fixtures/x86/stage2"
TABLE_PATH = DEST / "s2_13_answers.json"
BASELINE = "s2_12_and_8_rm_reg.case.json"
MEMORY_BASELINE = "s2_11_add_64_mem_reg.case.json"
HEX = lambda n: f"0x{n:016x}"
PREFIX = {8: b"", 16: b"\x66", 32: b"", 64: b"\x48"}
GROUP = {"rol": 0, "ror": 1, "rcl": 2, "rcr": 3, "shl": 4, "shr": 5, "sar": 7}


def instruction(row: dict) -> bytes:
    op, width, form, count = row["op"], row["width"], row["form"], row["count"]
    prefix = PREFIX[width]
    rm = 3 if row.get("memory") else 0
    if op in GROUP:
        modrm = (GROUP[op] << 3) | rm
        if not row.get("memory"):
            modrm |= 0xC0
        if form == "cl":
            opcode = 0xD2 if width == 8 else 0xD3
            return prefix + bytes((opcode, modrm))
        opcode = 0xC0 if width == 8 else 0xC1
        return prefix + bytes((opcode, modrm, count))

    base = {"shld": (0xA4, 0xA5), "shrd": (0xAC, 0xAD)}[op]
    opcode = base[1] if form == "cl" else base[0]
    modrm = (1 << 3) | rm if row.get("memory") else 0xD0
    if not row.get("memory"):
        modrm |= 0xC0
    encoded = prefix + b"\x0f" + bytes((opcode, modrm))
    return encoded if form == "cl" else encoded + bytes((count,))


def materialize() -> dict[str, bytes]:
    answers = json.loads(TABLE_PATH.read_text(encoding="utf-8"))
    table_hash = hashlib.sha256(TABLE_PATH.read_bytes()).hexdigest()
    generator_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    baseline_raw = (DEST / BASELINE).read_bytes()
    baseline = json.loads(baseline_raw)
    files: dict[str, bytes] = {}
    expected = {}

    for case_id, row in sorted(answers.items()):
        width = row["width"]
        code = instruction(row)
        case_source = MEMORY_BASELINE if row.get("memory") else BASELINE
        case_raw = (DEST / case_source).read_bytes()
        case = json.loads(case_raw) if row.get("memory") else json.loads(json.dumps(baseline))
        case["case_id"] = case_id
        case["provenance"] = {
            "kind": "project_fixture",
            "source": (
                f"Handwritten S2-13 opcode form; result and flags are a literal table row, "
                f"not computed by this materializer. Table SHA256 {table_hash}; "
                f"materializer SHA256 {generator_hash}; baseline {case_source} SHA256 "
                f"{hashlib.sha256(case_raw if row.get('memory') else baseline_raw).hexdigest()}."
            ),
            "specification": (
                "Intel SDM revision093 Vol.2A SAL/SAR/SHL/SHR pp.4-582..585; "
                "Vol.2B RCL/RCR/ROL/ROR pp.4-521..524; SHLD pp.4-640..641; "
                "SHRD pp.4-643..644; Vol.1 AppendixA TableA-2 EFLAGS; "
                "Vol.2A sections2.1.5 and 2.2.1.6 ModR/M/REX"
            ),
            "specification_version": "Intel SDM revision093; project S2-13",
        }
        gpr = [int(value, 16) for value in baseline["initial_state"]["gpr"]]
        if not row.get("memory"):
            if width == 8:
                gpr[0] = (gpr[0] & ~0xFF) | row["old"]
            elif width == 16:
                gpr[0] = (gpr[0] & ~0xFFFF) | row["old"]
            elif width == 32:
                # The decoded r32 destination is written even when the
                # effective count is zero, so the architectural dword write
                # zero-extends into RAX[63:32].
                gpr[0] = (gpr[0] & 0xFFFFFFFF00000000) | row["old"]
            else:
                gpr[0] = row["old"]
        if not row.get("memory"):
            gpr[2] = (gpr[2] & ~((1 << width) - 1)) | row["source"]
        elif row["op"] in ("shld", "shrd"):
            gpr[1] = row["source"]
        if row["form"] == "cl" and not row.get("memory"):
            gpr[1] = (gpr[1] & ~0xFF) | row["count"]
        case["initial_state"]["gpr"] = list(map(HEX, gpr))
        initial_flags = 0xCD7 if row["carry_in"] else 0xCD6
        case["initial_state"]["rflags"] = HEX(initial_flags)
        case["rflags_mask"] = HEX(row["rflags_defined"])
        code_region = case["memory"][0]
        code_region["length"] = len(code)
        code_region["initial_bytes_hex"] = code.hex()
        code_region["comparison_mask_hex"] = "ff" * len(code)
        stop = int(case["initial_state"]["rip"], 16) + len(code)
        case["stop_rip"] = HEX(stop)
        case["instruction_budget"] = 1

        result_gpr = list(gpr)
        if row.get("memory"):
            access = case["initial_state"]["gpr"][3]
            access = int(access, 16)
            replacement = row["result"].to_bytes(width // 8, "little")
            memory_expected = {}
            for region in case["memory"]:
                if not region["region_id"].startswith("data"):
                    continue
                base = int(region["guest_address"], 16)
                before = bytearray.fromhex(region["initial_bytes_hex"])
                after = bytearray(before)
                for offset, byte in enumerate(replacement):
                    if base <= access + offset < base + len(after):
                        after[access + offset - base] = byte
                if after != before:
                    memory_expected[region["region_id"]] = after.hex()
        elif width == 8:
            result_gpr[0] = (gpr[0] & ~0xFF) | row["result"]
        elif width == 16:
            result_gpr[0] = (gpr[0] & ~0xFFFF) | row["result"]
        else:
            result_gpr[0] = row["result"]
        expected[case_id] = {
            "gpr": list(map(HEX, result_gpr)),
            "rflags": HEX(row["rflags"]),
            "rflags_defined": HEX(row["rflags_defined"]),
            "memory": memory_expected if row.get("memory") else {},
        }
        files[f"{case_id}.case.json"] = (json.dumps(case, indent=2) + "\n").encode()

    files["s2_13_expected.json"] = (json.dumps(expected, indent=2, sort_keys=True) + "\n").encode()
    return files


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    files = materialize()
    stale = sorted(p.name for p in DEST.glob("s2_13_*.case.json") if p.name not in files)
    bad = [name for name, data in files.items() if not (DEST / name).exists() or (DEST / name).read_bytes() != data]
    if stale or args.check and bad:
        print("Shift fixture mismatch: " + ", ".join(stale + bad))
        return 1
    if not args.check:
        for name, data in files.items():
            (DEST / name).write_bytes(data)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
