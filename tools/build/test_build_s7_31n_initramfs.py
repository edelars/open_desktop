#!/usr/bin/env python3
"""Narrow tests for the S7-31n initramfs build and for the COM1 analysis of the efivars/ACPI record."""
from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1] / "tests" / "system"))
import build_s7_31n_initramfs as build  # noqa: E402
import build_stage4_initramfs as stage4  # noqa: E402
import firmware_linux_uefi_efivars_acpi as record  # noqa: E402

BOOT_ORDER = b"BootOrder-8be4df61-93ca-11d2-aa0d-00e098032b8c"
COUNT = b"ODBootCount-6f1d52a4-26b0-4f0c-9a37-4e0b61c82d15"


class InitramfsBuild(unittest.TestCase):
    def test_archive_holds_one_static_init_without_host_paths(self) -> None:
        with tempfile.TemporaryDirectory() as out:
            archive = build.build(Path(out)).read_bytes()
        entries = stage4.parse_cpio(archive)
        self.assertEqual([entry["name"] for entry in entries], ["init"])
        elf = entries[0]["data"]
        self.assertEqual(elf[:4], b"\x7fELF")
        for marker in stage4.HOST_PATH_MARKERS:
            self.assertNotIn(marker, elf)
        for text in (b"OD_S7_31N_EFIVARS_BEGIN", b"OD_S7_31N_DMESG_END", b"/sys/firmware/efi/efivars"):
            self.assertIn(text, elf)


class Analysis(unittest.TestCase):
    def test_names_markers_and_acpi_error_lines_are_read_from_the_marked_sections(self) -> None:
        com1 = (b"[    1.0] ACPI: kernel console line\r\n"
                b"OD_S7_31N_BEGIN\nOD_S7_31N_EFIVARFS_MOUNT_OK\n"
                b"\nOD_S7_31N_EFIVARS_BEGIN\n\x01\x02" + BOOT_ORDER + b"\x00\x03" + COUNT + b"\x00"
                b"\nOD_S7_31N_EFIVARS_END\nOD_S7_31N_DMESG_BEGIN\n"
                b"<6>[    0.1] ACPI: RSDP 0xe0000\n<3>[    0.2] ACPI BIOS Error (bug): AE_NOT_FOUND\n"
                b"<6>[    0.3] unrelated\n\nOD_S7_31N_DMESG_END\nOD_S7_31N_END\n")
        result = record.analyse(com1)
        self.assertTrue(result["efivarfs_mounted"])
        self.assertEqual(result["efivars_names"], [BOOT_ORDER.decode(), COUNT.decode()])
        self.assertEqual(result["dmesg_lines"], 3)
        self.assertEqual(len(result["dmesg_acpi_lines"]), 2)
        self.assertEqual(len(result["dmesg_acpi_error_lines"]), 1)
        self.assertEqual(result["console_acpi_lines_total"], 3)

    def test_a_missing_section_is_not_an_empty_list(self) -> None:
        result = record.analyse(b"OD_S7_31N_BEGIN\nOD_S7_31N_EFIVARFS_MOUNT_FAIL\n")
        self.assertFalse(result["efivarfs_mounted"])
        self.assertIsNone(result["efivars_names"])
        self.assertEqual(result["dmesg_lines"], 0)

    def test_the_nvram_seed_carries_the_vendor_variable(self) -> None:
        image = record.snapshot([("ODBootCount", record.OD_VENDOR, b"\x03\0\0\0")])
        self.assertTrue(image.startswith(b"ODNVBANK"))
        self.assertIn("ODBootCount".encode("utf-16le"), image)
        self.assertIn(record.OD_VENDOR, image)


if __name__ == "__main__":
    unittest.main()
