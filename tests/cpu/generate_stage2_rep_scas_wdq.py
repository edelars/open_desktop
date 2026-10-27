#!/usr/bin/env python3
"""Serialize hand-authored S2-19g REPE SCASW/D/Q fixtures and answers."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
BASE = 0x200000
CODE_BASE = 0x100000
PF_ZF = 0x46
EQUAL_FLAGS = 0x46
MISMATCH_FLAGS = 0x97
BASE_GPR = [0x41, 0, 0xDEADBEEFFEDCBA98, 0x200000,
            0x300080, 0x300080, 0x200000, BASE, 0x0123456789ABCDEF,
            0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
            0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
            0xFFFFFFFFFFFFFFFF]
WIDTHS = (("word", 2, bytes.fromhex("f366af")),
          ("dword", 4, bytes.fromhex("f3af")),
          ("qword", 8, bytes.fromhex("f348af")))


def hx(value: int) -> str:
    return f"0x{value & ((1 << 64) - 1):016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    answers: dict[str, object] = {}

    def add(case_id: str, *, code: bytes, count: int, rdi: int, data_base: int,
            data: bytes, flags: int, final_count: int, final_rdi: int,
            final_flags: int, outcome: str = "stop_address", budget: int = 1,
            defined: int = 0x0CD7, rip: int | None = None,
            fault: dict | None = None, rax: int | None = None,
            test_definedness: bool = False) -> None:
        gpr = BASE_GPR.copy()
        gpr[0] = 0x41 if rax is None else rax
        gpr[1], gpr[7] = count, rdi
        final = gpr.copy()
        final[1], final[7] = final_count, final_rdi
        if test_definedness:
            defined = 0x0CC7
        code_region = {
            "region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code),
            "permissions": {"read": False, "write": False, "execute": True},
            "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code),
        }
        data_region = {
            "region_id": "data", "guest_address": hx(data_base), "length": len(data),
            "permissions": {"read": True, "write": False, "execute": False},
            "initial_bytes_hex": data.hex(), "comparison_mask_hex": "ff" * len(data),
        }
        prefix_label = "F3 AE / F3 66 AF / F3 AF / F3 REX.W AF"
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {
                "kind": "project_fixture",
                "source": "Handwritten literal instruction bytes, state and expected answers; tests/cpu/generate_stage2_rep_scas_wdq.py serializes constants only. No assembler, oracle, or execution engine is used.",
                "specification": "Intel SDM revision 093, Vol.2B SCAS and REP/REPE entries; SCASW/D/Q compares AX/EAX/RAX with ES:[RDI] and adjusts RDI by DF.",
                "specification_version": "Intel SDM Vol.2B revision 093 (253667-093US); project S2-19g bounded REPE SCASW/D/Q",
            },
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(x) for x in gpr],
                              "rip": hx(CODE_BASE), "rflags": hx(flags)},
            "memory": [code_region, data_region],
            "stop_rip": hx(CODE_BASE + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000,
            "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        files[f"{case_id}.case.json"] = dump(fixture)
        answer: dict[str, object] = {
            "gpr": [hx(x) for x in final], "rflags": hx(final_flags),
            "rflags_defined": hx(defined),
            "rip": hx(rip if rip is not None else
                      (CODE_BASE + len(code) if outcome == "stop_address" else CODE_BASE)),
            "memory": {"code": code.hex(), "data": data.hex()},
        }
        if fault is not None:
            answer["fault"] = fault
        answers[case_id] = answer

    for name, width, code in WIDTHS:
        step = width
        equal = b"A" + b"\x00" * (width - 1)
        mismatch = b"B" + b"\x00" * (width - 1)
        for df in (False, True):
            bit = int(df)
            start = BASE + (2 * width if df else 0)
            direction = -step if df else step
            initial_flags = 2 | 4 | 0x40 | (0x400 if df else 0)
            equal_flags = EQUAL_FLAGS | (0x400 if df else 0)
            add(f"s2_19g_rep_scas_{name}_zero_df{bit}", code=code, count=0,
                rdi=0x0000800000000000, data_base=BASE, data=equal,
                flags=initial_flags, final_count=0, final_rdi=0x0000800000000000,
                final_flags=initial_flags)
            for is_equal, label, byte, result_flags in (
                (True, "equal", equal, equal_flags),
                (False, "mismatch", mismatch, MISMATCH_FLAGS | (0x400 if df else 0)),
            ):
                add(f"s2_19g_rep_scas_{name}_one_{label}_df{bit}", code=code,
                    count=1, rdi=start, data_base=BASE,
                    data=(equal * 2 + byte if df else byte),
                    flags=initial_flags, final_count=0, final_rdi=start + direction,
                    final_flags=result_flags)
            middle = equal + mismatch + equal
            add(f"s2_19g_rep_scas_{name}_middle_mismatch_df{bit}", code=code,
                count=3, rdi=BASE + 2 * width if df else BASE,
                data_base=BASE, data=middle, flags=initial_flags, final_count=1,
                final_rdi=(BASE if df else BASE + 2 * width),
                final_flags=MISMATCH_FLAGS | (0x400 if df else 0))
            add(f"s2_19g_rep_scas_{name}_all_equal_df{bit}", code=code, count=3,
                rdi=BASE + 2 * width if df else BASE, data_base=BASE,
                data=equal * 3, flags=initial_flags, final_count=0,
                final_rdi=BASE - width if df else BASE + 3 * width,
                final_flags=equal_flags)
            add(f"s2_19g_rep_scas_{name}_limit_1025_df{bit}", code=code,
                count=1025, rdi=BASE + (1024 * width if df else 0),
                data_base=BASE, data=equal * 1025, flags=initial_flags,
                final_count=1, final_rdi=BASE if df else BASE + 1024 * width,
                final_flags=equal_flags, outcome="rep_iteration_budget_exhausted")

        for successes in (0, 1):
            rdi = BASE
            data_base = BASE + 0x100 if successes == 0 else BASE
            after = BASE + width * successes
            add(f"s2_19g_rep_scas_{name}_unmapped_after_{successes}", code=code,
                count=3, rdi=rdi, data_base=data_base, data=equal,
                flags=2 | 4 | 0x40, final_count=3 - successes,
                final_rdi=after, final_flags=EQUAL_FLAGS if successes else 2 | 4 | 0x40,
                outcome="harness_error",
                fault={"access": "read", "address": hx(after)})
        bad = 0x0000800000000000
        add(f"s2_19g_rep_scas_{name}_noncanonical_first", code=code,
            count=1, rdi=bad, data_base=BASE, data=equal,
            flags=2 | 4 | 0x40, final_count=1, final_rdi=bad,
            final_flags=2 | 4 | 0x40, outcome="exception",
            fault={"vector": 13, "error_code": 0, "linear_address": None})
        add(f"s2_19g_rep_scas_{name}_budget_zero", code=code,
            count=1, rdi=BASE, data_base=BASE, data=equal,
            flags=2 | 4 | 0x40, final_count=1, final_rdi=BASE,
            final_flags=2 | 4 | 0x40, outcome="instruction_budget_exhausted", budget=0)

    for code, name, step in ((bytes.fromhex("f34866af"), "word_rex_before_66", 2),
                             (bytes.fromhex("f36648af"), "qword_rex_after_66", 8)):
        add(f"s2_19g_rep_scas_normalized_{name}", code=code, count=1, rdi=BASE,
                data_base=BASE, data=b"A" + b"\x00" * (step - 1), flags=PF_ZF, final_count=0,
            final_rdi=BASE + step, final_flags=EQUAL_FLAGS)

    for name, width, code in WIDTHS:
        equal = b"A" + b"\x00" * (width - 1)
        for kind in ("success", "fault_after_one", "test_undefined_af_reject"):
            scas = bytes.fromhex("f366af" if name == "word" else "f3af" if name == "dword" else "f348af")
            if kind == "success":
                program = bytes.fromhex("4839c0") + scas
                add(f"s2_19g_cmp_rax_rax_rep_scas_{name}_success", code=program,
                    count=1, rdi=BASE, data_base=BASE, data=equal,
                    flags=0x08D7, final_count=0, final_rdi=BASE + width,
                    final_flags=EQUAL_FLAGS, budget=2)
            elif kind == "fault_after_one":
                program = bytes.fromhex("4839c0") + scas
                add(f"s2_19g_cmp_rax_rax_rep_scas_{name}_fault_after_one", code=program,
                    count=2, rdi=BASE, data_base=BASE, data=equal,
                    flags=0x08D7, final_count=1, final_rdi=BASE + width,
                    final_flags=EQUAL_FLAGS, outcome="harness_error", budget=2,
                    rip=CODE_BASE + 3,
                    fault={"access": "read", "address": hx(BASE + width)})
            else:
                program = bytes.fromhex("4885c0") + scas
                add(f"s2_19g_test_rax_rep_scas_{name}_undefined_af_reject", code=program,
                    count=1, rdi=BASE, data_base=BASE, data=equal,
                    flags=0x0CC7, final_count=1, final_rdi=BASE,
                    final_flags=0x446, outcome="unsupported", budget=2,
                    rip=CODE_BASE + 3, rax=0, test_definedness=True)

    files["s2_19g_rep_scas_wdq_expected.json"] = dump(answers)
    return files


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    files = materialize()
    if args.check:
        stale = [name for name, content in files.items()
                 if not (FIXTURES / name).exists() or (FIXTURES / name).read_bytes() != content]
        if stale:
            raise SystemExit("stale S2-19g fixtures: " + ", ".join(stale))
    else:
        FIXTURES.mkdir(parents=True, exist_ok=True)
        for name, content in files.items():
            (FIXTURES / name).write_bytes(content)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
