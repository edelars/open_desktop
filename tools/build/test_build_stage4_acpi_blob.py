#!/usr/bin/env python3
"""Tests of the Stage 4 ACPI blob packaging (S4-31d).

The extraction is exercised on real freestanding builds: two clean builds into different cargo
target directories must give the same bytes, and those bytes must equal the committed fixture
(which the firmware host test compares with the run-time serializers). Corrupted sections must
be refused by the packaging sanity checks.
"""

from __future__ import annotations

import importlib.util
import struct
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/build/build_stage4_acpi_blob.py"
FIXTURE = ROOT / "tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"
SPEC = importlib.util.spec_from_file_location("stage4_acpi_blob_builder", SCRIPT)
assert SPEC and SPEC.loader
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)


class Stage4AcpiBlobPackagingTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.tmp = tempfile.TemporaryDirectory()
        base = Path(cls.tmp.name)
        cls.metadata = []
        cls.elves = []
        for name in ("a", "b"):
            elf = BUILDER.build_elf(base / name / "target")
            cls.elves.append(elf.read_bytes())
            cls.metadata.append(BUILDER.package(elf, base / name / "out"))
            cls.out = base

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tmp.cleanup()

    def blob(self, name: str) -> bytes:
        return (self.out / name / "out/stage4_acpi_blob.bin").read_bytes()

    def test_two_clean_builds_give_the_same_blob_and_hash(self) -> None:
        self.assertEqual(self.blob("a"), self.blob("b"))
        self.assertEqual(self.metadata[0]["blob_sha256"], self.metadata[1]["blob_sha256"])
        self.assertEqual(self.metadata[0], self.metadata[1])
        self.assertEqual(self.elves[0], self.elves[1])

    def test_extracted_section_equals_the_committed_fixture(self) -> None:
        self.assertEqual(self.blob("a"), FIXTURE.read_bytes())

    def test_metadata_reports_version_hash_range_and_rsdp(self) -> None:
        meta = self.metadata[0]
        self.assertEqual(meta["version"], 1)
        self.assertEqual(len(meta["manifest_hash"]), 64)
        self.assertEqual(meta["load_range"]["start"], 0xE0000)
        self.assertLessEqual(meta["load_range"]["end"], 0xEF000)
        self.assertEqual(meta["rsdp_address"], 0)
        self.assertEqual([t["signature"] for t in meta["tables"]], ["DSDT", "MCFG"])

    def test_corrupted_blob_is_refused(self) -> None:
        good = self.blob("a")
        info = BUILDER.parse_blob(good)
        payload = info["payload_offset"]
        table = info["tables"][0]
        broken = bytearray(good)
        broken[payload + table["payload_offset"] + 0x20] ^= 1  # checksum
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.parse_blob(bytes(broken))
        for offset, value in ((0, ord("X")), (8, 2), (0x4C, 0)):
            bad = bytearray(good)
            bad[offset] = value
            with self.assertRaises(BUILDER.BuildError):
                BUILDER.parse_blob(bytes(bad))
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.parse_blob(good[:-1])

    def test_section_lookup_rejects_missing_or_foreign_files(self) -> None:
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.extract_section(b"not an elf at all" * 8)
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.extract_section(self.elves[0], ".no_such_section")
        broken = bytearray(self.elves[0])
        struct.pack_into("<Q", broken, 0x28, len(broken) + 64)  # e_shoff outside the file
        with self.assertRaises(BUILDER.BuildError):
            BUILDER.extract_section(bytes(broken))


if __name__ == "__main__":
    unittest.main()
