#!/usr/bin/env python3
"""Serialize fixed S2-18o address-size-32 MOVS fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/x86/stage2"
CODE_BASE = 0x00100000
DATA_BASE = 0x00200000
FORMS = [(1, "b", "67a4"), (2, "w", "6766a5"), (4, "d", "67a5"), (8, "q", "6748a5")]
BASE_GPRS = [0x1122334455667788, 0xAABBCCDDEEFF0011, 0xDEADBEEFFEDCBA98,
             0x0000000000200000, 0x0000000000300080, 0x0000000000300080,
             0xA5A5A5A500200020, 0x5A5A5A5A00200040, 0x0123456789ABCDEF,
             0xFEDCBA9876543210, 0x8877665544332211, 0xAABBCCDDEEFF0011,
             0x1122112211221122, 0x3344334433443344, 0x778899AABBCCDDEE,
             0xFFFFFFFFFFFFFFFF]
FLAGS_DF = 0x0CD7
FLAGS_CLEAR = 0x08D7

# id, bytes, width, ESI, EDI, DF, budget, mapped regions, initial flags,
# optional prefix instruction bytes, initial prefix DF, special kind.
CASES: list[tuple] = []


def add(case_id, code, width, esi, edi, df=0, budget=1, regions=None,
        flags=None, prefix="", prefix_df=None, kind="success"):
    if regions is None:
        data = bytes((0x10 + i) & 0xFF for i in range(128))
        regions = [(DATA_BASE, data, True, True, "data0")]
    CASES.append((case_id, code, width, esi, edi, df, budget, regions,
                  (FLAGS_DF if df else FLAGS_CLEAR) if flags is None else flags,
                  prefix, prefix_df, kind))


for width, suffix, opcode in FORMS:
    for df, direction in [(0, "df0"), (1, "df1")]:
        regions = None
        if suffix == "q" and direction == "df0":
            data = bytes((0x50 + i) & 0xFF for i in range(width))
            regions = [(0x200020, data, True, False, "source_ro"),
                       (0x300040, bytes([0xCC] * width), False, True, "destination_wo")]
            edi = 0x300040
        else:
            edi = 0x200040
        add(f"s2_18o_pos_{suffix}_{direction}", opcode, width,
            0x200020, edi, df, regions=regions)

# One missing source and one missing destination per width. The latter runs
# after a successful source read and preserves the complete fixture snapshot.
for width, suffix, opcode in FORMS:
    add(f"s2_18o_unmapped_{suffix}_src", opcode, width, 0x300000, 0x200040,
        regions=[(DATA_BASE, bytes(range(128)), True, True, "data0")], kind="unmapped_src")
    add(f"s2_18o_unmapped_{suffix}_dst", opcode, width, 0x200020, 0x300000,
        regions=[(DATA_BASE, bytes(range(128)), True, True, "data0")], kind="unmapped_dst")

for width, suffix, opcode in FORMS:
    add(f"s2_18o_readonly_dst_{suffix}", opcode, width, 0x200020, 0x200040,
        regions=[(DATA_BASE, bytes(range(128)), True, False, "data0")], kind="readonly_dst")

for width, suffix, opcode in FORMS:
    add(f"s2_18o_budget0_{suffix}", opcode, width, 0x200020, 0x200040,
        budget=0, kind="budget")

for width, suffix, opcode in FORMS:
    for df, direction in [(0, "up"), (1, "down")]:
        for reg in ["esi", "edi"]:
            start = 0xFFFFFFFF - (width - 1) if df == 0 else 0
            esi, edi = (start, 0x200040) if reg == "esi" else (0x200020, start)
            data = bytes((0x80 + i) & 0xFF for i in range(width))
            add(f"s2_18o_wrap_{suffix}_{direction}_{reg}", opcode, width, esi, edi, df,
                regions=[(DATA_BASE, bytes(range(128)), True, True, "data0"),
                         (start, data, True, True, "edge")])

for width, suffix, opcode in FORMS[1:]:
    for reg in ["esi", "edi"]:
        esi, edi = (0xFFFFFFFF, 0x200040) if reg == "esi" else (0x200020, 0xFFFFFFFF)
        data = bytes((0x30 + i) & 0xFF for i in range(width))
        add(f"s2_18o_cross4g_{suffix}_{reg}", opcode, width, esi, edi,
            regions=[(DATA_BASE, bytes(range(128)), True, True, "data0"),
                     (0xFFFFFFFF, data, True, True, "edge")])

# Prefix ordering follows decoder normalization; these two execute as MOVSD/Q.
add("s2_18o_normalize_rex67_d", "4867a5", 4, 0x200020, 0x200040)
add("s2_18o_normalize_676648_q", "676648a5", 8, 0x200020, 0x200040)

# TEST creates defined arithmetic flags (AF remains undefined); MOVS preserves them.
for suffix, opcode, width in [("b", "67a4", 1)]:
    add(f"s2_18o_test_{suffix}_success", "4885c0" + opcode, width,
        0x200020, 0x200040, budget=2, flags=0x0046, prefix="test", kind="test")
add("s2_18o_test_first_fault", "4885c067a4", 1, 0x300000, 0x200040,
    budget=2, flags=0x0046, prefix="test", kind="unmapped_src")
add("s2_18o_test_second_fault", "4885c067a4", 1, 0x200020, 0x300000,
    budget=2, flags=0x0046, prefix="test", kind="unmapped_dst")

for width, suffix, opcode in FORMS:
    for mnemonic, prefix, df in [("cld", "fc", 0), ("std", "fd", 1)]:
        add(f"s2_18o_{mnemonic}_{suffix}", prefix + opcode, width,
            0x200020, 0x200040, df, budget=2, prefix=mnemonic,
            prefix_df=1 - df)

for width, suffix, opcode in FORMS:
    add(f"s2_18o_alias_{suffix}", opcode, width, 0x200020, 0xA5A5A5A500200020)

for width, suffix, opcode in FORMS:
    add(f"s2_18o_dual_unmapped_{suffix}", opcode, width, 0x300000, 0x310000,
        regions=[], kind="unmapped_src")

# Partial overlaps in both directions per width; expected destination bytes
# are derived from the complete pre-write source slice.
for width, suffix, opcode in FORMS:
    for direction, distance in [("forward", 1), ("backward", -1)]:
        if width == 1:
            continue
        esi = 0x200020
        edi = esi + (1 if direction == "forward" else -1)
        add(f"s2_18o_overlap_{suffix}_{direction}", opcode, width, esi, edi)

assert len(CASES) == 73, len(CASES)

EXPECTED = json.loads(r'''
{
  "s2_18o_pos_b_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200021",
      "0x0000000000200041",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f305152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_pos_b_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001f",
      "0x000000000020003f",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f305152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_pos_w_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200022",
      "0x0000000000200042",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_pos_w_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001e",
      "0x000000000020003e",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_pos_d_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
      "0x0000000000200044",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132335455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_pos_d_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001c",
      "0x000000000020003c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132335455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_pos_q_df0": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
      "0x0000000000300048",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "source_ro": "5051525354555657",
      "destination_wo": "5051525354555657"
    },
    "retired": 1
  },
  "s2_18o_pos_q_df1": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200018",
      "0x0000000000200038",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132333435363758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_unmapped_b_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200040",
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
      "code": "67a4",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_unmapped_b_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
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
      "code": "67a4",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Write"
    }
  },
  "s2_18o_unmapped_w_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200040",
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
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_unmapped_w_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
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
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Write"
    }
  },
  "s2_18o_unmapped_d_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200040",
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
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_unmapped_d_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
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
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Write"
    }
  },
  "s2_18o_unmapped_q_src": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200040",
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
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_unmapped_q_dst": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
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
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Write"
    }
  },
  "s2_18o_readonly_dst_b": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "67a4",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "PermissionDenied",
      "address": "0x0000000000200040",
      "access": "Write"
    }
  },
  "s2_18o_readonly_dst_w": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "PermissionDenied",
      "address": "0x0000000000200040",
      "access": "Write"
    }
  },
  "s2_18o_readonly_dst_d": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "PermissionDenied",
      "address": "0x0000000000200040",
      "access": "Write"
    }
  },
  "s2_18o_readonly_dst_q": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f"
    },
    "retired": 0,
    "harness_error": {
      "kind": "PermissionDenied",
      "address": "0x0000000000200040",
      "access": "Write"
    }
  },
  "s2_18o_budget0_b": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "67a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 0
  },
  "s2_18o_budget0_w": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 0
  },
  "s2_18o_budget0_d": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 0
  },
  "s2_18o_budget0_q": {
    "expected_exit": "instruction_budget_exhausted",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
      "0x5a5a5a5a00200040",
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
      "code": "6748a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 0
  },
  "s2_18o_wrap_b_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x0000000000200041",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a4",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f804142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "80"
    },
    "retired": 1
  },
  "s2_18o_wrap_b_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200021",
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
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a4",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "20"
    },
    "retired": 1
  },
  "s2_18o_wrap_b_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000ffffffff",
      "0x000000000020003f",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a4",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f804142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "80"
    },
    "retired": 1
  },
  "s2_18o_wrap_b_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001f",
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
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a4",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "20"
    },
    "retired": 1
  },
  "s2_18o_wrap_w_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x0000000000200042",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f808142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "8081"
    },
    "retired": 1
  },
  "s2_18o_wrap_w_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200022",
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
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "2021"
    },
    "retired": 1
  },
  "s2_18o_wrap_w_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000fffffffe",
      "0x000000000020003e",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f808142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "8081"
    },
    "retired": 1
  },
  "s2_18o_wrap_w_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001e",
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
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "2021"
    },
    "retired": 1
  },
  "s2_18o_wrap_d_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x0000000000200044",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f808182834445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "80818283"
    },
    "retired": 1
  },
  "s2_18o_wrap_d_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
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
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "20212223"
    },
    "retired": 1
  },
  "s2_18o_wrap_d_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000fffffffc",
      "0x000000000020003c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f808182834445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "80818283"
    },
    "retired": 1
  },
  "s2_18o_wrap_d_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001c",
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
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "20212223"
    },
    "retired": 1
  },
  "s2_18o_wrap_q_up_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000000",
      "0x0000000000200048",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f808182838485868748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "8081828384858687"
    },
    "retired": 1
  },
  "s2_18o_wrap_q_up_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
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
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "2021222324252627"
    },
    "retired": 1
  },
  "s2_18o_wrap_q_down_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x00000000fffffff8",
      "0x0000000000200038",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f808182838485868748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "8081828384858687"
    },
    "retired": 1
  },
  "s2_18o_wrap_q_down_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200018",
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
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "2021222324252627"
    },
    "retired": 1
  },
  "s2_18o_cross4g_w_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000001",
      "0x0000000000200042",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f303142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "3031"
    },
    "retired": 1
  },
  "s2_18o_cross4g_w_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200022",
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
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "2021"
    },
    "retired": 1
  },
  "s2_18o_cross4g_d_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000003",
      "0x0000000000200044",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f303132334445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "30313233"
    },
    "retired": 1
  },
  "s2_18o_cross4g_d_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
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
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "20212223"
    },
    "retired": 1
  },
  "s2_18o_cross4g_q_esi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000000007",
      "0x0000000000200048",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f303132333435363748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "3031323334353637"
    },
    "retired": 1
  },
  "s2_18o_cross4g_q_edi": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
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
    "rflags": "0x00000000000008d7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f",
      "edge": "2021222324252627"
    },
    "retired": 1
  },
  "s2_18o_normalize_rex67_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
      "0x0000000000200044",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "4867a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132335455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_normalize_676648_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
      "0x0000000000200048",
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
    "rip": "0x0000000000100004",
    "memory": {
      "code": "676648a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132333435363758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_test_b_success": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x0000000000000000",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200021",
      "0x0000000000200041",
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
    "rip": "0x0000000000100005",
    "memory": {
      "code": "4885c067a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f305152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_test_first_fault": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x0000000000000000",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00200040",
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
      "code": "4885c067a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_test_second_fault": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x0000000000000000",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500200020",
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
      "code": "4885c067a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Write"
    }
  },
  "s2_18o_cld_b": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200021",
      "0x0000000000200041",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fc67a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f305152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_std_b": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001f",
      "0x000000000020003f",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fd67a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f305152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_cld_w": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200022",
      "0x0000000000200042",
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
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fc6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_std_w": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001e",
      "0x000000000020003e",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fd6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_cld_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
      "0x0000000000200044",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fc67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132335455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_std_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x000000000020001c",
      "0x000000000020003c",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100003",
    "memory": {
      "code": "fd67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132335455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_cld_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
      "0x0000000000200048",
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
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fc6748a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132333435363758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_std_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200018",
      "0x0000000000200038",
      "0x0123456789abcdef",
      "0xfedcba9876543210",
      "0x8877665544332211",
      "0xaabbccddeeff0011",
      "0x1122112211221122",
      "0x3344334433443344",
      "0x778899aabbccddee",
      "0xffffffffffffffff"
    ],
    "rflags": "0x0000000000000cd7",
    "rflags_defined": "0x0000000000000cd7",
    "rip": "0x0000000000100004",
    "memory": {
      "code": "fd6748a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f303132333435363758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 2
  },
  "s2_18o_alias_b": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200021",
      "0x0000000000200021",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a4",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_alias_w": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200022",
      "0x0000000000200022",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_alias_d": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
      "0x0000000000200024",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_alias_q": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
      "0x0000000000200028",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_dual_unmapped_b": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00310000",
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
      "code": "67a4"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_dual_unmapped_w": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00310000",
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
      "code": "6766a5"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_dual_unmapped_d": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00310000",
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
      "code": "67a5"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_dual_unmapped_q": {
    "expected_exit": "harness_error",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0xa5a5a5a500300000",
      "0x5a5a5a5a00310000",
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
      "code": "6748a5"
    },
    "retired": 0,
    "harness_error": {
      "kind": "Unmapped",
      "address": "0x0000000000300000",
      "access": "Read"
    }
  },
  "s2_18o_overlap_w_forward": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200022",
      "0x0000000000200023",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303031333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_overlap_w_backward": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200022",
      "0x0000000000200021",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6766a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e30313132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_overlap_d_forward": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
      "0x0000000000200025",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303031323335363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_overlap_d_backward": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200024",
      "0x0000000000200023",
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
    "rip": "0x0000000000100002",
    "memory": {
      "code": "67a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e30313233333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_overlap_q_forward": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
      "0x0000000000200029",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303031323334353637393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  },
  "s2_18o_overlap_q_backward": {
    "expected_exit": "stop_address",
    "gpr": [
      "0x1122334455667788",
      "0xaabbccddeeff0011",
      "0xdeadbeeffedcba98",
      "0x0000000000200000",
      "0x0000000000300080",
      "0x0000000000300080",
      "0x0000000000200028",
      "0x0000000000200027",
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
    "rip": "0x0000000000100003",
    "memory": {
      "code": "6748a5",
      "data0": "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e30313233343536373738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f"
    },
    "retired": 1
  }
}
''')



def hx(value: int) -> str:
    return f"0x{value:016x}"


def encode_json(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def materialize() -> dict[str, bytes]:
    """Serialize fixture inputs and the frozen literal EXPECTED table."""
    generated: dict[str, bytes] = {}
    hashes: dict[str, str] = {}
    assert set(EXPECTED) == {case[0] for case in CASES}
    for (case_id, code_hex, width, esi, edi, df, budget, regions, flags,
         prefix, prefix_df, _kind) in CASES:
        code = bytes.fromhex(code_hex)
        before = BASE_GPRS.copy()
        before[6] = 0xA5A5A5A500000000 | esi
        before[7] = 0x5A5A5A5A00000000 | edi
        if prefix == "test":
            before[0] = 0
        initial_flags = (FLAGS_DF if prefix_df else FLAGS_CLEAR) if prefix_df is not None else flags
        memory = [{"region_id": "code", "guest_address": hx(CODE_BASE), "length": len(code),
                   "permissions": {"read": False, "write": False, "execute": True},
                   "initial_bytes_hex": code.hex(), "comparison_mask_hex": "ff" * len(code)}]
        for base, data, readable, writable, region_id in regions:
            data_hex = data.hex()
            memory.append({"region_id": region_id, "guest_address": hx(base), "length": len(data),
                           "permissions": {"read": readable, "write": writable, "execute": False},
                           "initial_bytes_hex": data_hex,
                           "comparison_mask_hex": "ff" * len(data)})
        expected_exit = EXPECTED[case_id]["expected_exit"]
        fixture = {
            "schema_version": 2, "case_id": case_id, "profile": "s2-long-integer-v1",
            "provenance": {"kind": "project_fixture", "source": "Handwritten MOVS encodings, addresses, memory contents, permissions, and expected state; tests/cpu/generate_stage2_movs_addr32.py serializes these constants only.",
                           "specification": "Intel SDM revision 093, Vol.2B MOVS; address-size 32 zero-extends ESI/EDI. Local execution reads source then writes destination.",
                           "specification_version": "Intel SDM revision 093; project S2-18o bounded local profile"},
            "seed": None, "converted_from_v1_sha256": None,
            "initial_state": {"mode": "long64", "gpr": [hx(v) for v in before], "rip": hx(CODE_BASE), "rflags": hx(initial_flags)},
            "memory": memory,
            "stop_rip": hx(CODE_BASE + len(code)) if expected_exit == "stop_address" else None,
            "instruction_budget": budget, "wall_timeout_ms": 2000, "rflags_mask": hx(0x0CD7),
            "expected_exit": expected_exit}
        fixture_bytes = encode_json(fixture)
        filename = f"{case_id}.case.json"
        generated[filename] = fixture_bytes
        hashes[filename] = hashlib.sha256(fixture_bytes).hexdigest()
    expected_path = "s2_18o_movs_addr32_expected.json"
    manifest_path = "s2_18o_movs_addr32_manifest.json"
    expected_bytes = encode_json(EXPECTED)
    generated[expected_path] = expected_bytes
    manifest = {"fixture_ids": sorted(EXPECTED), "fixture_files": sorted(hashes),
                "fixture_sha256": hashes, "expected_file": expected_path,
                "expected_sha256": hashlib.sha256(expected_bytes).hexdigest(),
                "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    generated[manifest_path] = encode_json(manifest)
    return generated

def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stale = []
    generated = materialize()
    for name, content in generated.items():
        path = FIXTURES / name
        if args.check:
            if not path.exists() or path.read_bytes() != content:
                stale.append(name)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
    if stale:
        print("stale generated files: " + ", ".join(stale))
        return 1
    print("S2-18o MOVS address-size-32 fixtures are fresh" if args.check else f"wrote {len(generated)} generated files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
