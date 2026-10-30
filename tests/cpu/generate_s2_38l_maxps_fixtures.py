#!/usr/bin/env python3
"""Generate bounded, literal schema-v2 anchors for exact Long64 MAXPS."""
from __future__ import annotations

import copy
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "tests/fixtures/x86/stage2"
PREFIX = "s2_38l_maxps_"
# binary32 raw event representatives: normal, QNaN, subnormal, QNaN+subnormal
CLASSES = {
    "n": (0x3F800000, 0x40000000),
    "i": (0x7FC12345, 0x40000000),
    "d": (0x00000001, 0x40000000),
    "id": (0x7FC12345, 0x00000001),
}


def pack_lanes(values: tuple[int, ...]) -> bytes:
    return b"".join(value.to_bytes(4, "little") for value in values)


def make(template: dict, name: str, lhs: tuple[int, ...], rhs: tuple[int, ...], im: int = 1, dm: int = 1, osx: int = 1) -> dict:
    value = copy.deepcopy(template)
    value["case_id"] = PREFIX + name
    value["profile"] = "s2-long-maxps-unmasked-denormal-v1"
    value["provenance"]["source"] = f"Project-materialized literal four-lane MAXPS input {name}."
    value["provenance"]["specification_version"] = "Intel SDM Rev. 093; bounded local S2-38l."
    value["provenance"]["specification"] = "Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5.3.1–3; Vol. 2B MAXPS pp. 4-8–4-10."
    code = value["memory"][0]
    code["region_id"] = "code"
    code["guest_address"] = value["initial_state"]["rip"]
    code["length"] = 3
    code["permissions"] = {"read": True, "write": False, "execute": True}
    code["initial_bytes_hex"] = "0f5fc1"
    code["comparison_mask_hex"] = "ffffff"
    value["memory"] = [code]
    value["instruction_budget"] = 1
    start = int(value["initial_state"]["rip"], 16)
    value["stop_rip"] = f"0x{start + 3:016x}"
    value["maxps_unmasked_controls"] = value.pop("minpd_unmasked_controls")
    controls = value["maxps_unmasked_controls"]
    controls["sse_available"] = controls.pop("sse2_available")
    controls["cr4_osxmmexcpt"] = bool(osx)
    mxcsr = 0x1E00 | (im << 7) | (dm << 8)
    if name.endswith("sticky_status_only"):
        mxcsr |= 3
    value["initial_state"]["sse"]["mxcsr"] = f"0x{mxcsr:016x}"
    for index, lanes in ((0, lhs), (1, rhs)):
        xmm = bytearray.fromhex(value["initial_state"]["sse"]["xmm"][index])
        xmm[:] = pack_lanes(lanes)
        value["initial_state"]["sse"]["xmm"][index] = xmm.hex()
    raw_ie = any((bits & 0x7F800000) == 0x7F800000 and (bits & 0x007FFFFF) for pair in zip(lhs, rhs) for bits in pair)
    raw_de = any((bits & 0x7F800000) == 0 and (bits & 0x007FFFFF) for pair in zip(lhs, rhs) for bits in pair)
    value["expected_exit"] = "exception" if raw_ie and not im or raw_de and not dm else "stop_address"
    return value


def write(value: dict) -> None:
    (ROOT / f"{value['case_id']}.case.json").write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    template_path = next(ROOT.glob("s2_38i_minpd_l0_n_l1_n_im1_dm1_os1.case.json"))
    template = json.loads(template_path.read_text(encoding="utf-8"))
    # 16 named four-lane event anchors, spanning late lanes and split/mixed events.
    patterns = [
        ("n", "n", "n", "n"), ("i", "n", "n", "n"), ("n", "d", "n", "n"), ("n", "n", "i", "n"),
        ("n", "n", "n", "d"), ("id", "n", "n", "n"), ("n", "id", "n", "n"), ("i", "d", "n", "n"),
        ("d", "i", "n", "n"), ("n", "i", "d", "n"), ("n", "d", "i", "n"), ("i", "n", "n", "d"),
        ("d", "n", "n", "i"), ("n", "n", "id", "n"), ("n", "n", "n", "id"), ("i", "d", "i", "d"),
    ]
    for index, pattern in enumerate(patterns):
        lhs = tuple(CLASSES[c][0] for c in pattern)
        rhs = tuple(CLASSES[c][1] for c in pattern)
        im, dm, osx = ((index >> 0) & 1, (index >> 1) & 1, (index >> 2) & 1)
        write(make(template, f"matrix_{index:02}_{'_'.join(pattern)}_im{im}_dm{dm}_os{osx}", lhs, rhs, im, dm, osx))
    specials = {
        "qnan_source1_lane2": ((0x3F800000, 0x40000000, 0x7FC12345, 0x40400000), (0x40000000, 0x40000000, 0x40800000, 0x40800000)),
        "qnan_source2_lane3_payload_sign": ((0x3F800000, 0x40000000, 0x40400000, 0x40800000), (0x40000000, 0x40000000, 0x40800000, 0xFFC54321)),
        "snan_source1_lane3": ((0x3F800000, 0x40000000, 0x40400000, 0xFF812345), (0x40000000, 0x40000000, 0x40800000, 0x3F800000)),
        "snan_source2_lane2": ((0x3F800000, 0x40000000, 0x40400000, 0x40800000), (0x40000000, 0x40000000, 0x7F812345, 0x40800000)),
        "positive_zero_tie_source2": ((0, 0x3F800000, 0, 0x40000000), (0x80000000, 0x40000000, 0, 0x3F800000)),
        "negative_zero_tie_source2": ((0x80000000, 0x40000000, 0x80000000, 0x3F800000), (0, 0x3F800000, 0, 0x40000000)),
        "equal_finite_source2": ((0x40490FDB, 0xC005BF0A, 0x3F800000, 0x40000000), (0x40490FDB, 0xC005BF0A, 0x3F800000, 0x40000000)),
        "positive_infinity": ((0x7F800000, 0xBF800000, 0x3F800000, 0x40000000), (0x3F800000, 0x3F800000, 0x7F800000, 0x40000000)),
        "negative_infinity": ((0xFF800000, 0xBF800000, 0x3F800000, 0x40000000), (0xBF800000, 0x3F800000, 0x3F800000, 0x40000000)),
        "distinguish_min_from_max": ((0xBF800000, 0x40000000, 0x3F800000, 0x40400000), (0x3F800000, 0x3F800000, 0x40000000, 0x3F800000)),
        "subnormal_lane3": ((0x3F800000, 0x40000000, 0x40400000, 1), (0x40000000, 0x3F800000, 0x40800000, 0x3F800000)),
        "same_lane_qnan_subnormal": ((0x3F800000, 0x40000000, 0x7FC00042, 0x40800000), (0x40000000, 0x3F800000, 1, 0x3F800000)),
        "split_invalid_and_denormal": ((0x7FC12345, 0x40000000, 0x40400000, 0x40800000), (0x40000000, 1, 0x40800000, 0x3F800000)),
        "masked_qnan_source2": ((0x3F800000, 0x40000000, 0x40400000, 0x40800000), (0xFFC12345, 0x40000000, 0x40800000, 0x3F800000)),
        "masked_snan_source2": ((0x3F800000, 0x40000000, 0x40400000, 0x40800000), (0xFF812345, 0x40000000, 0x40800000, 0x3F800000)),
        "sticky_status_only": ((0x3F800000, 0x40000000, 0x40400000, 0x40800000), (0x40000000, 0x3F800000, 0x40800000, 0x40400000)),
    }
    for name, (lhs, rhs) in specials.items():
        write(make(template, f"special_{name}", lhs, rhs, 1, 1, 1))

if __name__ == "__main__":
    main()
