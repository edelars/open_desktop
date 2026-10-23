#!/usr/bin/env python3
"""Serialize literal S2-22h MOV GS,AX descriptor-load cases."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
RIP = 0x00100000
GDTR_BASE = 0x00200000
CODE = "8ee8"  # MOV GS, AX
COMPOSITION_CODE = "8ee865488b00"  # MOV GS, AX; MOV RAX, qword ptr GS:[RAX]
FS_COMPOSITION_CODE = "8ee064488b00"  # MOV FS, AX; MOV RAX, qword ptr FS:[RAX]
GPRS = [
    0xAABBCCDDEEFF000B, 0x0102030405060708,
    0x1122334455667788, 0x8877665544332211,
    0x0000000000200080, 0x0000000000200081,
    0x0000000000200004, 0x0000000000200010,
    0x0123456789ABCDEF, 0xFEDCBA9876543210,
    0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344,
    0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF,
]

# (case id, selector, access, descriptor flags, GDTR limit, expected exit, note, base32)
CASES = [
    ("s2_22h_gs_load_writable_g0", 0x0B, 0xF3, 0x0, 0x000F, "stop_address",
     "accessed writable data descriptor with G=0 and base32=0x89abcdef", 0x89ABCDEF),
    ("s2_22h_gs_load_readonly_g1", 0x0B, 0xF1, 0x8, 0x000F, "stop_address",
     "accessed read-only data descriptor with G=1", 0),
    ("s2_22h_gs_load_readable_code", 0x0B, 0xFB, 0x0, 0x000F, "stop_address",
     "accessed readable non-conforming code descriptor", 0),
    ("s2_22h_gs_load_conforming_code", 0x08, 0x9F, 0x0, 0x000F, "stop_address",
     "CPL3 loads readable conforming DPL0 code despite ordinary DPL privilege checks", 0),
    ("s2_22h_gs_load_system_gp", 0x0B, 0x83, 0x0, 0x000F, "exception",
     "system descriptor raises #GP(selector)", 0),
    ("s2_22h_gs_load_execute_only_gp", 0x0B, 0xF9, 0x0, 0x000F, "exception",
     "execute-only code descriptor raises #GP(selector)", 0),
    ("s2_22h_gs_load_privilege_gp", 0x08, 0x93, 0x0, 0x000F, "exception",
     "CPL3 exceeds descriptor DPL0 even with RPL0; raises #GP(selector)", 0),
    ("s2_22h_gs_load_not_present_np", 0x0B, 0x73, 0x0, 0x000F, "exception",
     "otherwise valid not-present descriptor raises #NP(selector)", 0),
    ("s2_22h_gs_load_table_limit_gp", 0x0B, 0xF3, 0x0, 0x000E, "exception",
     "full descriptor crosses GDTR limit and raises #GP(selector) before read", 0),
    ("s2_22h_gs_load_a0_writable", 0x0B, 0xF2, 0x0, 0x000F, "stop_address",
     "present writable data descriptor starts with A=0; one byte write sets A", 0x89ABCDEF),
    ("s2_22h_gs_load_a0_readonly", 0x0B, 0xF0, 0x0, 0x000F, "stop_address",
     "present read-only data descriptor starts with A=0; one byte write sets A", 0),
]


def hx(value: int) -> str:
    return f"0x{value:016x}"


def make_descriptor(access: int, flags: int, base32: int) -> bytes:
    # The eight-byte descriptor encodes base[23:0] in bytes 2–4 and
    # base[31:24] in byte 7. Keep the high-byte base fixture literal.
    return bytes([
        0, 0,
        base32 & 0xff,
        (base32 >> 8) & 0xff,
        (base32 >> 16) & 0xff,
        access,
        (flags & 0x0f) << 4,
        (base32 >> 24) & 0xff,
    ])


def make_case(case_id: str, selector: int, access: int, flags: int, gdtr_limit: int,
              expected_exit: str, note: str, base32: int) -> dict:
    gprs = list(GPRS)
    gprs[0] = (gprs[0] & ~0xFFFF) | selector
    return {
        "schema_version": 2,
        "case_id": case_id,
        "profile": "s2-long-gs-load-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten instruction bytes, selector/access/flags/base fields, state, "
                       "and expected exit. The generator lays the literal descriptor fields out "
                       "in the architectural eight-byte encoding; it does not assemble the "
                       "instruction, emulate, or calculate CPU semantics."),
            "specification": ("Intel SDM revision 093 (253667-093US), Vol.2B MOV pp.4-28–4-31; "
                              "Vol.3A (325384-093US) §3.4.5.1 pp.3-12–3-13, "
                              "§5.6.1 pp.5-33–5-35, §5.7 pp.5-37–5-38, "
                              "§11.1.2.1 p.11-3 Automatic Locking."),
            "specification_version": (
                "Intel SDM revision 093; Vol.2B PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671241; Vol.3A PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671447; project S2-22h. " + note
            ),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in gprs],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
            "gs_base": hx(0xFFFF800000001000),
            "gs_selector": hx(0x0033),
            "gdtr_base": hx(GDTR_BASE),
            "gdtr_limit": hx(gdtr_limit),
            "gs_cache_limit": hx(0x0000FFFF),
            "gs_cache_access": hx(0x00F3),
            "gs_cache_flags": hx(0x0000),
        },
        "memory": [
            {
                "region_id": "code",
                "guest_address": hx(RIP),
                "length": 2,
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": CODE,
                "comparison_mask_hex": "ffff",
            },
            {
                "region_id": "gdt",
                "guest_address": hx(GDTR_BASE),
                "length": 16,
                "permissions": {
                    "read": True,
                    "write": access & 1 == 0,
                    "execute": False,
                },
                "initial_bytes_hex": (
                    "0000000000000000" + make_descriptor(access, flags, base32).hex()
                ),
                "comparison_mask_hex": "ff" * 16,
            },
        ],
        "stop_rip": hx(RIP + 2),
        "instruction_budget": 1,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": expected_exit,
    }


def make_composition_case() -> dict:
    """Literal oracle for consuming the descriptor-loaded GS.base."""
    gprs = [0] * 16
    gprs[0] = 0x000000000000000B
    gprs[1] = 0x0102030405060708
    gprs[2] = 0x1122334455667788
    gprs[3] = 0x8877665544332211
    gprs[4] = 0x0000000000200080
    gprs[5] = 0x0000000000200081
    gprs[6] = 0x0000000000200004
    gprs[7] = 0x0000000000200010
    gprs[8:] = [0x0123456789ABCDEF, 0xFEDCBA9876543210,
                0x8877665544332211, 0xAABBCCDDEEFF0011,
                0x1122112211221122, 0x3344334433443344,
                0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
    return {
        "schema_version": 2,
        "case_id": "s2_22i_gs_base_composition",
        "profile": "s2-long-gs-load-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten 8E E8 65 48 8B 00 bytes, initial state, literal "+
                       "descriptor/data/canary bytes, and harness fields. The generator "+
                       "serializes fixture inputs only; Rust tests hold literal expected "+
                       "post-state and memory answers. It does not assemble or emulate."),
            "specification": ("Intel SDM revision 093 (253667-093US), Vol.2B MOV "+
                              "pp.4-28–4-31; Vol.3A (325384-093US) §3.4.5.1, "+
                              "§5.6.1, §5.7, and §11.1.2.1 Automatic Locking."),
            "specification_version": (
                "Intel SDM revision 093; Vol.2B PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671241; Vol.3A PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671447; project S2-22i. "
                "Composition case proves the loaded GS.base selects the qword data."),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in gprs],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
            "gs_base": hx(0x00400000),
            "gs_selector": hx(0x0033),
            "gdtr_base": hx(GDTR_BASE),
            "gdtr_limit": hx(15),
            "gs_cache_limit": hx(0x0000FFFF),
            "gs_cache_access": hx(0x00F3),
            "gs_cache_flags": hx(0),
        },
        "memory": [
            {
                "region_id": "code", "guest_address": hx(RIP), "length": 6,
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": COMPOSITION_CODE, "comparison_mask_hex": "ff" * 6,
            },
            {
                "region_id": "gdt", "guest_address": hx(GDTR_BASE), "length": 16,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": "00000000000000000000000000f20080",
                "comparison_mask_hex": "ff" * 16,
            },
            {
                "region_id": "old_gs_canary", "guest_address": hx(0x00400000), "length": 19,
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": "00" * 11 + "deadc0decafef00d",
                "comparison_mask_hex": "ff" * 19,
            },
            {
                "region_id": "loaded_gs_data", "guest_address": hx(0x80000000), "length": 19,
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": "00" * 11 + "8877665544332211",
                "comparison_mask_hex": "ff" * 19,
            },
        ],
        "stop_rip": hx(RIP + 6),
        "instruction_budget": 2,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "stop_address",
    }


def make_fs_composition_case() -> dict:
    """Literal oracle for consuming the descriptor-loaded FS.base."""
    gprs = [0] * 16
    gprs[0] = 0x000000000000000B
    gprs[1] = 0x0102030405060708
    gprs[2] = 0x1122334455667788
    gprs[3] = 0x8877665544332211
    gprs[4] = 0x0000000000200080
    gprs[5] = 0x0000000000200081
    gprs[6] = 0x0000000000200004
    gprs[7] = 0x0000000000200010
    gprs[8:] = [0x0123456789ABCDEF, 0xFEDCBA9876543210,
                0x8877665544332211, 0xAABBCCDDEEFF0011,
                0x1122112211221122, 0x3344334433443344,
                0x778899AABBCCDDEE, 0xFFFFFFFFFFFFFFFF]
    return {
        "schema_version": 2,
        "case_id": "s2_22j_fs_base_composition",
        "profile": "s2-long-fs-load-v1",
        "provenance": {
            "kind": "project_fixture",
            "source": ("Handwritten 8E E0 64 48 8B 00 bytes, initial state, literal "+
                       "descriptor/data/canary bytes, and harness fields. The generator "+
                       "serializes fixture inputs only; Rust tests hold literal expected "+
                       "post-state and memory answers. It does not assemble or emulate."),
            "specification": ("Intel SDM revision 093 (253667-093US), Vol.2B MOV "+
                              "pp.4-28–4-31; Vol.3A (325384-093US) §3.4.5.1, "+
                              "§5.6.1, §5.7, and §11.1.2.1 Automatic Locking."),
            "specification_version": (
                "Intel SDM revision 093; Vol.2B PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671241; Vol.3A PDF "
                "https://cdrdv2.intel.com/v1/dl/getContent/671447; project S2-22j. "
                "Composition case proves the loaded FS.base selects the qword data."),
        },
        "seed": None,
        "converted_from_v1_sha256": None,
        "initial_state": {
            "mode": "long64",
            "gpr": [hx(value) for value in gprs],
            "rip": hx(RIP),
            "rflags": hx(0x0CD7),
            "fs_base": hx(0x00400000),
            "fs_selector": hx(0x0033),
            "gdtr_base": hx(GDTR_BASE),
            "gdtr_limit": hx(15),
            "fs_cache_limit": hx(0x0000FFFF),
            "fs_cache_access": hx(0x00F3),
            "fs_cache_flags": hx(0),
        },
        "memory": [
            {
                "region_id": "code", "guest_address": hx(RIP), "length": 6,
                "permissions": {"read": False, "write": False, "execute": True},
                "initial_bytes_hex": FS_COMPOSITION_CODE, "comparison_mask_hex": "ff" * 6,
            },
            {
                "region_id": "gdt", "guest_address": hx(GDTR_BASE), "length": 16,
                "permissions": {"read": True, "write": True, "execute": False},
                "initial_bytes_hex": "00000000000000000000000000f20080",
                "comparison_mask_hex": "ff" * 16,
            },
            {
                "region_id": "old_fs_canary", "guest_address": hx(0x00400000), "length": 19,
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": "00" * 11 + "deadc0decafef00d",
                "comparison_mask_hex": "ff" * 19,
            },
            {
                "region_id": "loaded_fs_data", "guest_address": hx(0x80000000), "length": 19,
                "permissions": {"read": True, "write": False, "execute": False},
                "initial_bytes_hex": "00" * 11 + "8877665544332211",
                "comparison_mask_hex": "ff" * 19,
            },
        ],
        "stop_rip": hx(RIP + 6),
        "instruction_budget": 2,
        "wall_timeout_ms": 2000,
        "rflags_mask": hx(0x0CD7),
        "expected_exit": "stop_address",
    }


def materialize() -> dict[str, bytes]:
    result = {
        f"{case_id}.case.json": (json.dumps(make_case(*case), indent=2) + "\n").encode()
        for case in CASES
        for case_id in [case[0]]
    }
    result["s2_22i_gs_base_composition.case.json"] = (
        json.dumps(make_composition_case(), indent=2) + "\n"
    ).encode()
    result["s2_22j_fs_base_composition.case.json"] = (
        json.dumps(make_fs_composition_case(), indent=2) + "\n"
    ).encode()
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    mismatches = []
    for name, contents in materialize().items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                mismatches.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(contents)
    if mismatches:
        print("stale generated files: " + ", ".join(mismatches))
        return 1
    print("S2-22h MOV GS load and S2-22i/j composition fixtures are fresh" if args.check
          else "wrote S2-22h MOV GS load and S2-22i/j composition fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
