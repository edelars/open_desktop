#!/usr/bin/env python3
"""Materialize handwritten S2-38j MAXPD fixture inputs from literal lane classes."""
from __future__ import annotations

import copy
import itertools
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "tests/fixtures/x86/stage2"
MIN_PREFIX = "s2_38i_minpd_"
MAX_PREFIX = "s2_38j_maxpd_"
CLASSES = {
    "n": (0x3FF0000000000000, 0x4000000000000000),
    "i": (0x7FF8123456789ABC, 0x4000000000000000),
    "d": (0x8000000000000001, 0x4000000000000000),
    "id": (0x7FF8123456789ABC, 0x8000000000000001),
}
SPECIALS = {
    "qnan_source1_lane0": ((0x7FF8123456789ABC, 0x3FF0000000000000), (0x4000000000000000, 0x4000000000000000)),
    "qnan_source2_lane0": ((0x3FF0000000000000, 0x4000000000000000), (0xFFF8ABCDEF012345, 0x4000000000000000)),
    "snan_source1_lane1": ((0x3FF0000000000000, 0xFFF0123456789ABC), (0x4000000000000000, 0xBFF0000000000000)),
    "snan_source2_lane1": ((0x3FF0000000000000, 0x4000000000000000), (0x4000000000000000, 0x7FF0123456789ABC)),
    "payload_sign_snan_source2": ((0x3FF0000000000000, 0x4000000000000000), (0xBFF0000000000000, 0xFFF0000000000042)),
    "payload_sign_qnan_source2": ((0x3FF0000000000000, 0x4000000000000000), (0xBFF0000000000000, 0xFFF8FEDCBA987654)),
    "positive_zero_tie_source2": ((0x0000000000000000, 0xBFF0000000000000), (0x8000000000000000, 0x3FF0000000000000)),
    "negative_zero_tie_source2": ((0x8000000000000000, 0x3FF0000000000000), (0x0000000000000000, 0xBFF0000000000000)),
    "equal_finite_source2": ((0x400921FB54442D18, 0xC005BF0A8B145769), (0x400921FB54442D18, 0xC005BF0A8B145769)),
    "positive_infinity_lhs": ((0x7FF0000000000000, 0xBFF0000000000000), (0x3FF0000000000000, 0x3FF0000000000000)),
    "negative_infinity_lhs": ((0xFFF0000000000000, 0xBFF0000000000000), (0xBFF0000000000000, 0x3FF0000000000000)),
    "positive_infinity_rhs": ((0x3FF0000000000000, 0xBFF0000000000000), (0x7FF0000000000000, 0x3FF0000000000000)),
    "distinguish_max_from_min": ((0xBFF0000000000000, 0x4000000000000000), (0x3FF0000000000000, 0x3FF0000000000000)),
    "subnormal_and_normal": ((0x0000000000000001, 0x3FF0000000000000), (0x3FF0000000000000, 0x0000000000000001)),
    "qnan_and_subnormal": ((0x7FF8000000000001, 0x0000000000000001), (0x3FF0000000000000, 0x7FF8000000000002)),
    "sticky_ie_de_no_event": ((0x3FF0000000000000, 0x4000000000000000), (0x4000000000000000, 0x3FF0000000000000)),
}


def rewrite(template: dict, case_id: str, left: tuple[int, int], right: tuple[int, int], im: int = 1, dm: int = 1, osx: int = 1) -> dict:
    value = copy.deepcopy(template)
    value["case_id"] = case_id
    value["profile"] = "s2-long-maxpd-unmasked-denormal-v1"
    value["provenance"]["source"] = f"Project-materialized literal MAXPD two-lane input {case_id.removeprefix(MAX_PREFIX)}."
    value["provenance"]["specification_version"] = "Intel SDM Rev. 093; bounded local S2-38j."
    value["provenance"]["specification"] = "Intel SDM Rev. 093 Vol. 1 §§4.9.2, 11.5.3.1–3; Vol. 2B MAXPD pp. 4-5–4-7."
    value["memory"][0]["initial_bytes_hex"] = "660f5fc1"
    value["maxpd_unmasked_controls"] = value.pop("minpd_unmasked_controls")
    controls = value["maxpd_unmasked_controls"]
    controls["cr4_osxmmexcpt"] = bool(osx)
    sse = value["initial_state"]["sse"]
    mxcsr = 0x1E00 | (int(im) << 7) | (int(dm) << 8)
    if case_id.endswith("sticky_ie_de_no_event"):
        mxcsr |= 3
    sse["mxcsr"] = f"0x{mxcsr:016x}"
    for reg, pair in ((0, left), (1, right)):
        current = bytearray.fromhex(sse["xmm"][reg])
        current[:8] = pair[0].to_bytes(8, "little")
        current[8:16] = pair[1].to_bytes(8, "little")
        sse["xmm"][reg] = current.hex()
    event = False
    for lane in range(2):
        bits = (left[lane], right[lane])
        invalid = any((x >> 52) & 0x7FF == 0x7FF and (x & ((1 << 52) - 1)) != 0 for x in bits)
        denormal = any((x >> 52) & 0x7FF == 0 and (x & ((1 << 52) - 1)) != 0 for x in bits)
        event |= invalid and not im or denormal and not dm
    value["expected_exit"] = "exception" if event else "stop_address"
    return value


def write(value: dict) -> None:
    path = ROOT / f"{value['case_id']}.case.json"
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    templates = {p.stem.removesuffix(".case"): json.loads(p.read_text()) for p in ROOT.glob("s2_38i_minpd_l0_*.case.json")}
    if len(templates) != 128:
        raise SystemExit(f"expected 128 S2-38i matrix templates, got {len(templates)}")
    for c0, c1, im, dm, osx in itertools.product(CLASSES, CLASSES, (0, 1), (0, 1), (0, 1)):
        old_id = f"s2_38i_minpd_l0_{c0}_l1_{c1}_im{im}_dm{dm}_os{osx}"
        new_id = f"{MAX_PREFIX}l0_{c0}_l1_{c1}_im{im}_dm{dm}_os{osx}"
        write(rewrite(templates[old_id], new_id, (CLASSES[c0][0], CLASSES[c1][0]), (CLASSES[c0][1], CLASSES[c1][1]), im, dm, osx))
    special_template = json.loads((ROOT / "s2_38i_minpd_special_equal_finite_source2.case.json").read_text())
    for name, (left, right) in SPECIALS.items():
        write(rewrite(special_template, f"{MAX_PREFIX}special_{name}", left, right))


if __name__ == "__main__":
    main()
