#!/usr/bin/env python3
"""Materialize S2-38r MAXPD m128 memory fixtures from raw binary64 lane classes."""
from __future__ import annotations

import copy
import itertools
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "tests/fixtures/x86/stage2"
PREFIX = "s2_38r_maxpd_m128_"
CLASSES = {
    "n": (0x3FF0000000000000, 0x4000000000000000),
    "i": (0x7FF8123456789ABC, 0x4000000000000000),
    "d": (0x8000000000000001, 0x4000000000000000),
    "id": (0x7FF8123456789ABC, 0x8000000000000001),
}
SPECIALS = {
    "qnan_memory_rhs": ((0x3FF0000000000000, 0x4000000000000000), (0xFFF8ABCDEF012345, 0x4000000000000000)),
    "snan_memory_rhs_payload": ((0x3FF0000000000000, 0x4000000000000000), (0xFFF0000000000042, 0x4000000000000000)),
    "qnan_lhs_and_memory_rhs": ((0x7FF8123456789ABC, 0x4000000000000000), (0xFFF8FEDCBA987654, 0x4000000000000000)),
    "positive_zero_tie_memory_rhs": ((0x0000000000000000, 0xBFF0000000000000), (0x8000000000000000, 0x3FF0000000000000)),
    "negative_zero_tie_memory_rhs": ((0x8000000000000000, 0x3FF0000000000000), (0x0000000000000000, 0xBFF0000000000000)),
    "distinguish_max_from_min": ((0xBFF0000000000000, 0x4000000000000000), (0x3FF0000000000000, 0x3FF0000000000000)),
    "memory_rhs_not_xmm1": ((0xBFF0000000000000, 0x4000000000000000), (0x3FF0000000000000, 0x3FF0000000000000)),
    "qnan_and_other_lane_subnormal": ((0x7FF8000000000001, 0x0000000000000001), (0x3FF0000000000000, 0x7FF8000000000002)),
    "finite_tie_memory_rhs": ((0x400921FB54442D18, 0xC005BF0A8B145769), (0x400921FB54442D18, 0xC005BF0A8B145769)),
    "sticky_ie_de_no_event": ((0x3FF0000000000000, 0x4000000000000000), (0x4000000000000000, 0x3FF0000000000000)),
}


def has_nan(bits: int) -> bool:
    return (bits >> 52) & 0x7FF == 0x7FF and bits & ((1 << 52) - 1) != 0


def has_subnormal(bits: int) -> bool:
    return (bits >> 52) & 0x7FF == 0 and bits & ((1 << 52) - 1) != 0


def rewrite(template: dict, case_id: str, left: tuple[int, int], right: tuple[int, int], im: int, dm: int, osx: int, rc: int = 0, sticky: int = 0, ac: int = 1) -> dict:
    value = copy.deepcopy(template)
    value["case_id"] = case_id
    value["profile"] = "s2-long-maxpd-memory-unmasked-rax-v1"
    value["provenance"]["source"] = f"Project-generated literal S2-38r memory MAXPD input {case_id.removeprefix(PREFIX)}."
    value["provenance"]["specification_version"] = "Intel SDM Rev. 093; bounded local S2-38r."
    value["provenance"]["specification"] = "Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5.3.1–3; Vol. 2B MAXPD pp. 4-5–4-7."
    value["expected_exit"] = "stop_address"
    sse = value["initial_state"]["sse"]
    sse["mxcsr"] = f"0x{(0x1E00 | (im << 7) | (dm << 8) | (rc << 13) | sticky):016x}"
    rflags = int(value["initial_state"]["rflags"], 16)
    value["initial_state"]["rflags"] = f"0x{(rflags | (1 << 18) if ac else rflags & ~(1 << 18)):016x}"
    for offset, bits in zip((0, 8), left):
        xmm = bytearray.fromhex(sse["xmm"][0])
        xmm[offset:offset + 8] = bits.to_bytes(8, "little")
        sse["xmm"][0] = xmm.hex()
    sse["xmm"][1] = "420000000000f07f0100000000000080"  # poisoned SNaN + subnormal
    data = value["memory"][1]
    data["initial_bytes_hex"] = b"".join(bits.to_bytes(8, "little") for bits in right).hex()
    controls = value["maxpd_memory_rax_controls"]
    value["maxpd_memory_rax_controls"] = None
    value["maxpd_unmasked_controls"] = {
        "sse2_available": controls["sse2_available"],
        "cr0_em": controls["cr0_em"],
        "cr0_ts": controls["cr0_ts"],
        "cr4_osfxsr": controls["cr4_osfxsr"],
        "cr4_osxmmexcpt": bool(osx),
    }
    event = False
    for lane in range(2):
        invalid = has_nan(left[lane]) or has_nan(right[lane])
        denormal = has_subnormal(left[lane]) or has_subnormal(right[lane])
        event |= invalid and not im or denormal and not dm
    value["expected_exit"] = "exception" if event else "stop_address"
    return value


def write(value: dict) -> None:
    (ROOT / f"{value['case_id']}.case.json").write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    template = json.loads((ROOT / "s2_37cz_maxpd_m128_qnan_and_other_lane_denormal.case.json").read_text())
    for c0, c1, im, dm, osx, ac in itertools.product(CLASSES, CLASSES, (0, 1), (0, 1), (0, 1), (0, 1)):
        name = f"l0_{c0}_l1_{c1}_im{im}_dm{dm}_os{osx}_ac{ac}"
        write(rewrite(template, PREFIX + name, (CLASSES[c0][0], CLASSES[c1][0]), (CLASSES[c0][1], CLASSES[c1][1]), im, dm, osx, ac=ac))
    for name, (left, right) in SPECIALS.items():
        write(rewrite(template, PREFIX + "special_" + name, left, right, 1, 1, 1, rc=2, ac=1))
    xm_lanes = SPECIALS["qnan_memory_rhs"]
    write(rewrite(template, PREFIX + "literal_xm_memory_qnan", *xm_lanes, im=0, dm=1, osx=1))
    write(rewrite(template, PREFIX + "literal_arithmetic_ud_memory_qnan", *xm_lanes, im=0, dm=1, osx=0))
    base_left = (0x7FF8000000000001, 0x0000000000000001)
    base_right = (0x3FF0000000000000, 0x4000000000000000)
    for rc, sticky in itertools.product(range(4), range(64)):
        write(rewrite(template, PREFIX + f"rc{rc}_sticky_events{sticky:02x}", base_left, base_right, 1, 1, 1, rc, sticky, ac=rc & 1))


if __name__ == "__main__":
    main()
