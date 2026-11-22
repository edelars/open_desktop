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

    def test_variable_dumps_are_decoded_compared_and_the_onlcr_expansion_is_undone(self) -> None:
        # The value holds a 0x0A and a 0x0D 0x0A: on the wire the tty wrote 0D 0A and 0D 0D 0A.
        option = b"\x01\x00\x0a\x0d\x0a\x02"
        expected = {"BOOTORDER": b"\x01\x00", "BOOT0001": option, "ODBOOTCOUNT": b"\x03\x00\x00\x00"}
        seven = b"\x07\x00\x00\x00"
        wire_option = option.replace(b"\n", b"\r\n")
        com1 = (b"OD_S7_31O_BOOTORDER_BEGIN\r\n" + seven + b"\x01\x00OD_S7_31O_BOOTORDER_END\r\n"
                b"OD_S7_31O_BOOT0001_BEGIN\r\n" + seven + wire_option + b"OD_S7_31O_BOOT0001_END\r\n"
                b"OD_S7_31O_ODBOOTCOUNT_BEGIN\r\n" + b"\x06\x00\x00\x00" + b"\x03\x00\x00\x00"
                b"OD_S7_31O_ODBOOTCOUNT_END\r\n")
        result = record.variable_values(com1, expected)
        self.assertTrue(result["BOOTORDER"]["attributes_ok"] and result["BOOTORDER"]["value_ok"])
        self.assertEqual(result["BOOT0001"]["value_hex"], option.hex())
        self.assertTrue(result["BOOT0001"]["attributes_ok"] and result["BOOT0001"]["value_ok"])
        # A wrong attribute word is reported, not accepted; the value is still read.
        self.assertFalse(result["ODBOOTCOUNT"]["attributes_ok"])
        self.assertEqual(result["ODBOOTCOUNT"]["attributes"], "0x00000006")
        self.assertTrue(result["ODBOOTCOUNT"]["value_ok"])
        # Both markers without bytes between them (openat or read failed in /init) is no dump.
        empty = record.variable_values(com1.replace(b"OD_S7_31O_BOOT0001_BEGIN\r\n" + seven + wire_option, b"OD_S7_31O_BOOT0001_BEGIN\r\n"), expected)
        self.assertFalse(empty["BOOT0001"]["dumped"])
        self.assertFalse(empty["BOOT0001"].get("attributes_ok", False))
        # A wrong value and a missing dump.
        expected["BOOTORDER"] = b"\x02\x00"
        again = record.variable_values(com1.replace(b"OD_S7_31O_BOOT0001_BEGIN", b"x"), expected)
        self.assertFalse(again["BOOTORDER"]["value_ok"])
        self.assertFalse(again["BOOT0001"]["dumped"])

    def test_dmesg_severity_is_taken_from_the_level_prefix(self) -> None:
        lines = ["<6>[    0.1] ACPI: RSDP", "<3>[    1.4] Unable to read current time from RTC",
                 "<4>[    1.4] ACPI: setting ELCR to 0200 (from 0000)", "<0>[    2.0] panic",
                 "<6>[    1.7] acpi PNP0A08:00: _OSC: platform retains control (AE_ERROR)",
                 "<6>[    1.8] pci 0000:00:00.0: _OSC failed (AE_NOT_FOUND)", "<7>[    1.9] debug", "no prefix"]
        result = record.classify_dmesg(lines)
        self.assertEqual(result["errors_level_0_to_3"], [lines[1], lines[3]])
        self.assertEqual(result["warnings_level_4"], [lines[2]])
        # AE_ lines of any level and without the word acpi are kept.
        self.assertEqual(result["ae_lines"], [lines[4], lines[5]])
        self.assertEqual(result["levels"], {"0": 1, "3": 1, "4": 1, "6": 3, "7": 1, "none": 1})
        # The ELCR line has no word a word filter would catch: a warning is still recorded.
        self.assertNotRegex(lines[2], record.ACPI_ERROR_RE)

    def test_the_nvram_seed_carries_the_vendor_variable(self) -> None:
        image = record.snapshot([("ODBootCount", record.OD_VENDOR, b"\x03\0\0\0")])
        self.assertTrue(image.startswith(b"ODNVBANK"))
        self.assertIn("ODBootCount".encode("utf-16le"), image)
        self.assertIn(record.OD_VENDOR, image)


if __name__ == "__main__":
    unittest.main()
