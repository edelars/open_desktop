#!/usr/bin/env python3
"""Serialize literal S2-18n address-size-32 CMPS fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
EXPLICIT_MANIFEST = ROOT / "work/s2-18n-files.json"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
FLAGS_CLEAR = 0x08D7
FLAGS_DF = 0x0CD7
FLAGS_EQ_CLEAR = 0x0046
FLAGS_EQ_DF = 0x0446
ARITHMETIC_DEFINED = 0x0CD7
TEST_DEFINED = 0x0CC7
CODE_GPRS = [
    0x1122334455667788, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
    0x0000000000200000, 0x0000000000300080, 0x0000000000300080,
    0xA5A5A5A500200009, 0x5A5A5A5A00200003, 0x0123456789ABCDEF,
    0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
    0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
    0xFFFFFFFFFFFFFFFF,
]

# id, encoding, width, ESI, EDI, DF, outcome, budget, map regions, prefix kind.
# Expected states are recorded independently in the literal EXPECTED table below.
CASES = [
    ("s2_18n_pos_b_df0", "67a6", 1, 0x200004, 0x200008, 0, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_pos_b_df1", "67a6", 1, 0x200004, 0x200008, 1, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_pos_w_df0", "6766a7", 2, 0x200004, 0x200008, 0, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_pos_w_df1", "6766a7", 2, 0x200004, 0x200008, 1, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_pos_d_df0", "67a7", 4, 0x200004, 0x200008, 0, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_pos_d_df1", "67a7", 4, 0x200004, 0x200008, 1, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_pos_q_df0", "6748a7", 8, 0x200004, 0x200008, 0, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_pos_q_df1", "6748a7", 8, 0x200004, 0x200008, 1, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_b_src", "67a6", 1, 0x300000, 0x200008, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_b_dst", "67a6", 1, 0x200004, 0x300000, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_w_src", "6766a7", 2, 0x300000, 0x200008, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_w_dst", "6766a7", 2, 0x200004, 0x300000, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_d_src", "67a7", 4, 0x300000, 0x200008, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_d_dst", "67a7", 4, 0x200004, 0x300000, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_q_src", "6748a7", 8, 0x300000, 0x200008, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_unmapped_q_dst", "6748a7", 8, 0x200004, 0x300000, 0, "harness_error", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_budget0_b", "67a6", 1, 0x200004, 0x200008, 0, "instruction_budget_exhausted", 0, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_budget0_w", "6766a7", 2, 0x200004, 0x200008, 0, "instruction_budget_exhausted", 0, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_budget0_d", "67a7", 4, 0x200004, 0x200008, 0, "instruction_budget_exhausted", 0, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_budget0_q", "6748a7", 8, 0x200004, 0x200008, 0, "instruction_budget_exhausted", 0, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
]

for width, suffix, opcode in [(1, "b", "67a6"), (2, "w", "6766a7"), (4, "d", "67a7"), (8, "q", "6748a7")]:
    for df, direction in [(0, "up"), (1, "down")]:
        for pointer, reg in [("src", "esi"), ("dst", "edi")]:
            start = (0xFFFFFFFF - (width - 1)) if direction == "up" else 0
            regions = [(DATA_BASE, "00000000000000000000000000000000"), (start, "00000000000000000000000000000000")]
            esi, edi = 0x200004, 0x200008
            if pointer == "src": esi = start
            else: edi = start
            CASES.append((f"s2_18n_wrap_{suffix}_{direction}_{reg}", opcode, width, esi, edi, df, "stop_address", 1, regions, "normal"))

for width, suffix, opcode in [(2, "w", "6766a7"), (4, "d", "67a7"), (8, "q", "6748a7")]:
    for pointer, reg in [("src", "esi"), ("dst", "edi")]:
        esi, edi = 0x200004, 0x200008
        if pointer == "src": esi = 0xFFFFFFFF
        else: edi = 0xFFFFFFFF
        CASES.append((f"s2_18n_cross4g_{suffix}_{reg}", opcode, width, esi, edi, 0, "stop_address", 1,
                      [(0xFFFFFFF8, "00000000000000000000000000000000"), (DATA_BASE, "00000000000000000000000000000000")], "normal"))

CASES.extend([
    ("s2_18n_normalize_rex67_d", "4867a7", 4, 0x200004, 0x200008, 0, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_normalize_676648_q", "676648a7", 8, 0x200004, 0x200008, 0, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"),
    ("s2_18n_test_success", "4885c067a6", 1, 0x200004, 0x200008, 0, "stop_address", 2, [(DATA_BASE, "00000000000000000000000000000000")], "test"),
    ("s2_18n_test_first_fault", "4885c067a6", 1, 0x300000, 0x200008, 0, "harness_error", 2, [(DATA_BASE, "00000000000000000000000000000000")], "test"),
    ("s2_18n_test_second_fault", "4885c067a6", 1, 0x200004, 0x300000, 0, "harness_error", 2, [(DATA_BASE, "00000000000000000000000000000000")], "test"),
])

for width, suffix, cmps in [(1, "b", "67a6"), (2, "w", "6766a7"), (4, "d", "67a7"), (8, "q", "6748a7")]:
    for mnemonic, prefix, df in [("cld", "fc", 0), ("std", "fd", 1)]:
        # Start in the opposite direction state so the prefix effect is tested.
        initial_df = 1 - df
        CASES.append((f"s2_18n_{mnemonic}_{suffix}", prefix + cmps, width, 0x200004, 0x200008,
                      df, "stop_address", 2, [(DATA_BASE, "00000000000000000000000000000000")], f"{mnemonic}:{initial_df}"))

for width, suffix, opcode in [(1, "b", "67a6"), (2, "w", "6766a7"), (4, "d", "67a7"), (8, "q", "6748a7")]:
    CASES.append((f"s2_18n_alias_{suffix}", opcode, width, 0x200004, 0xA5A5A5A500200004,
                  0, "stop_address", 1, [(DATA_BASE, "00000000000000000000000000000000")], "normal"))

for width, suffix, opcode in [(1, "b", "67a6"), (2, "w", "6766a7"), (4, "d", "67a7"), (8, "q", "6748a7")]:
    CASES.append((f"s2_18n_dual_unmapped_{suffix}", opcode, width, 0x300000, 0x400000,
                  0, "harness_error", 1, [], "normal"))

assert len(CASES) == 63

# Independently derived, literal full expected snapshots; serialization only.
EXPECTED = json.loads(r'''
{
  "s2_18n_pos_b_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200005",
      "0x0000000000200009",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_pos_b_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200003",
      "0x0000000000200007",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_pos_w_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200006",
      "0x000000000020000a",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_pos_w_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200002",
      "0x0000000000200006",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_pos_d_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200008",
      "0x000000000020000c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_pos_d_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200000",
      "0x0000000000200004",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_pos_q_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020000c",
      "0x0000000000200010",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_pos_q_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000001ffffc",
      "0x0000000000200000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_unmapped_b_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_unmapped_b_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00300000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_unmapped_w_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_unmapped_w_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00300000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_unmapped_d_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_unmapped_d_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00300000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_unmapped_q_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_unmapped_q_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00300000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_budget0_b": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0
  },
  "s2_18n_budget0_w": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0
  },
  "s2_18n_budget0_d": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0
  },
  "s2_18n_budget0_q": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 0
  },
  "s2_18n_wrap_b_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x0000000000200009",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_b_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200005",
      "0x0000000000000000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_b_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000ffffffff",
      "0x0000000000200007",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_b_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200003",
      "0x00000000ffffffff",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_w_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x000000000020000a",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_w_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200006",
      "0x0000000000000000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_w_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000fffffffe",
      "0x0000000000200006",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_w_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200002",
      "0x00000000fffffffe",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_d_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x000000000020000c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_d_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200008",
      "0x0000000000000000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_d_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000fffffffc",
      "0x0000000000200004",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_d_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200000",
      "0x00000000fffffffc",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_q_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x0000000000200010",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_q_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020000c",
      "0x0000000000000000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_q_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000fffffff8",
      "0x0000000000200000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_wrap_q_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000001ffffc",
      "0x00000000fffffff8",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_cross4g_w_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000001",
      "0x000000000020000a",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_cross4g_w_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200006",
      "0x0000000000000001",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_cross4g_d_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000003",
      "0x000000000020000c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_cross4g_d_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200008",
      "0x0000000000000003",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_cross4g_q_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000007",
      "0x0000000000200010",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_cross4g_q_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020000c",
      "0x0000000000000007",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000",
      "data1": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_normalize_rex67_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200008",
      "0x000000000020000c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "4867a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_normalize_676648_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020000c",
      "0x0000000000200010",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100004",
    "memory": {
      "code": "676648a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_test_success": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x0000000000000000",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200005",
      "0x0000000000200009",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100005",
    "memory": {
      "code": "4885c067a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_test_first_fault": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x0000000000000000",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cc7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "4885c067a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_test_second_fault": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x0000000000000000",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200004",
      "0x5a5a5a5a00300000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cc7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "4885c067a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_cld_b": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200005",
      "0x0000000000200009",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fc67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_std_b": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200003",
      "0x0000000000200007",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fd67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_cld_w": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200006",
      "0x000000000020000a",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fc6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_std_w": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200002",
      "0x0000000000200006",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fd6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_cld_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200008",
      "0x000000000020000c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fc67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_std_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200000",
      "0x0000000000200004",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fd67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_cld_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020000c",
      "0x0000000000200010",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fc6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_std_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000001ffffc",
      "0x0000000000200000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000446",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fd6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 2
  },
  "s2_18n_alias_b": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200005",
      "0x0000000000200005",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a6",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_alias_w": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200006",
      "0x0000000000200006",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_alias_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200008",
      "0x0000000000200008",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_alias_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020000c",
      "0x000000000020000c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000046",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a7",
      "data0": "00000000000000000000000000000000"
    },
    "retired": 1
  },
  "s2_18n_dual_unmapped_b": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00400000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a6"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_dual_unmapped_w": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00400000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6766a7"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_dual_unmapped_d": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00400000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "67a7"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18n_dual_unmapped_q": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00400000",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100000",
    "memory": {
      "code": "6748a7"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  }
}
''')


def hx(value: int) -> str:
    return f"0x{value:016x}"


def dump(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    generated: dict[str, bytes] = {}
    hashes: dict[str, str] = {}
    for case_id, code_hex, width, esi, edi, df, outcome, budget, regions, prefix_kind in CASES:
        code = bytes.fromhex(code_hex)
        before = CODE_GPRS.copy()
        before[6] = 0xA5A5A5A500000000 | esi
        before[7] = 0x5A5A5A5A00000000 | edi
        flags_in = FLAGS_DF if df else FLAGS_CLEAR
        if prefix_kind == "test":
            before[0] = 0
            flags_in = FLAGS_EQ_CLEAR
        elif prefix_kind.startswith("cld:") or prefix_kind.startswith("std:"):
            flags_in = FLAGS_DF if prefix_kind.endswith(":1") else FLAGS_CLEAR
        memory = [{"region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code),
                   "permissions": {"read": False, "write": False, "execute": True},
                   "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)}]
        for index, (base, data_hex) in enumerate(regions):
            memory.append({"region_id": f"data{index}", "guest_address": hx(base), "length": len(bytes.fromhex(data_hex)),
                           "permissions": {"read": True, "write": False, "execute": False},
                           "initial_bytes_hex": data_hex, "comparison_mask_hex": "ff" * len(bytes.fromhex(data_hex))})
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Handwritten encodings, pointer values, memory bytes, and expected state; tests/cpu/generate_stage2_cmps_addr32.py serializes these constants only.",
                           "specification": "Intel SDM revision 093, Vol.2A CMPS; address-size 32 zero-extends ESI/EDI for addresses and wraps successful index updates at 32 bits. Local two-read order is ESI then EDI.",
                           "specification_version": "Intel SDM revision 093; project S2-18n bounded local profile"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(v) for v in before], "rip": hx(CODE_BASE), "rflags": hx(flags_in)},
            "memory": memory, "stop_rip": hx(CODE_BASE + len(code)) if outcome == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7), "expected_exit": outcome,
        }
        fixture_bytes = dump(fixture)
        filename = f"{case_id}.case.json"
        generated[filename] = fixture_bytes
        hashes[filename] = hashlib.sha256(fixture_bytes).hexdigest()
    generated["s2_18n_cmps_addr32_expected.json"] = dump(EXPECTED)
    generated["s2_18n_cmps_addr32_manifest.json"] = dump({
        "fixture_ids": sorted(EXPECTED), "fixture_sha256": hashes,
        "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    })
    generated["work/s2-18n-files.json"] = dump({
        "substage": "S2-18n",
        "generator": "tests/cpu/generate_stage2_cmps_addr32.py",
        "fixture_files": sorted(hashes),
        "expected_file": "tests/fixtures/x86/stage2/s2_18n_cmps_addr32_expected.json",
        "fixture_manifest": "tests/fixtures/x86/stage2/s2_18n_cmps_addr32_manifest.json",
        "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    })
    return generated


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    generated = materialize()
    for name, contents in generated.items():
        path = ROOT / name if name.startswith("work/") else FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != contents:
                stale.append(str(path.relative_to(ROOT)))
        else:
            path.write_bytes(contents)
    if stale:
        print("stale generated files: " + ", ".join(stale))
        return 1
    print("S2-18n CMPS address-size-32 fixtures are fresh" if args.check else f"wrote {len(generated)} S2-18n fixture files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
