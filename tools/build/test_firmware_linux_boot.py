#!/usr/bin/env python3
"""Narrow tests for the S7-31 acceptance checks (tests/system/firmware_linux_boot.py): the pure parts."""
from __future__ import annotations

from pathlib import Path
import struct
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tests" / "system"))
import firmware_linux_boot as boot  # noqa: E402

SEVEN = b"\x07\x00\x00\x00"
OPTION = b"\x01\x00\x00\x00\x0a\x02"
EXPECTED = {"BOOTORDER": struct.pack("<H", 1), "BOOT0001": OPTION, "ODBOOTCOUNT": struct.pack("<I", 3)}
REPORT = {"exit": "budget_exhausted", "exit_detail": "BudgetExhausted", "retired": 6000000000}
TABLES = [f"<6>[    0.018] ACPI: {t} 0x00000000000E0000 000024 (v02 OPNDSK)" for t in
          ("RSDP", "XSDT", "FACP", "DSDT", "FACS", "APIC", "MCFG")]


def good_lines(extra: list[str] | None = None) -> list[str]:
    return (["<5>[    0.000000] Linux version 7.0.0-38-generic (buildd@lcy02) #38-Ubuntu"] + TABLES
            + ["<6>[    0.7] acpi PNP0A08:00: _OSC: platform retains control of PCIe features (AE_ERROR)",
               "<4>[    0.4] ACPI: setting ELCR to 0200 (from 0000)",
               "<3>[    0.0] CPU: vendor_id 'OpenDesktop ' unknown, using generic init.",
               "<3>[    1.1] mce: Unable to init MCE device (rc: -5)",
               "<4>[    0.0] KHO: Failed to reserve scratch area, disabling kexec handover",
               "<6>[    0.5] PM: RTC time: 00:00:02, date: 2024-01-01"] + (extra or []))


def com1(lines: list[str] | None = None, init: bool = True, values: dict | None = None) -> bytes:
    lines = good_lines() if lines is None else lines
    names = b"".join(b"\x01\x02" + name.encode() + b"\x00" for name in boot.rec.VARIABLES)
    values = values or {"BOOTORDER": EXPECTED["BOOTORDER"], "BOOT0001": OPTION, "ODBOOTCOUNT": EXPECTED["ODBOOTCOUNT"]}
    out = b"OD-FW version 0.1.0\r\n" + ("\r\n".join(lines) + "\r\n").encode()
    if init:
        out += b"<6>[    2.0] Run /init as init process\r\nOD_S7_31N_BEGIN\r\nOD_S7_31N_EFIVARFS_MOUNT_OK\r\n"
        out += b"\r\nOD_S7_31N_EFIVARS_BEGIN\r\n" + names + b"\r\nOD_S7_31N_EFIVARS_END\r\n"
        for short, value in values.items():
            out += f"OD_S7_31O_{short}_BEGIN\r\n".encode() + SEVEN + value.replace(b"\n", b"\r\n")
            out += f"OD_S7_31O_{short}_END\r\n".encode()
        out += b"OD_S7_31N_DMESG_BEGIN\r\n" + ("\r\n".join(lines) + "\r\n").encode() + b"\r\nOD_S7_31N_DMESG_END\r\n"
        out += b"OD_S7_31N_END\r\n" + (boot.INIT_PANIC + "\r\n").encode()
    return out


def failed(data: bytes, report: dict = REPORT) -> list[str]:
    results, _ = boot.evaluate(data, report, EXPECTED)
    return [r.name for r in results if not r.ok]


class Evaluate(unittest.TestCase):
    def test_a_good_run_passes_every_check(self) -> None:
        results, _ = boot.evaluate(com1(), REPORT, EXPECTED)
        self.assertEqual([r.name for r in results if not r.ok], [])
        self.assertGreaterEqual(len(results), 15)

    def test_a_run_that_stopped_fails(self) -> None:
        self.assertIn("run ends at its budget", failed(com1(), {**REPORT, "exit": "unsupported"}))

    def test_a_missing_init_or_efivarfs_fails(self) -> None:
        names = failed(com1(init=False))
        for name in ("/init reached", "/init ran to its end", "efivarfs mounted", "efivarfs lists the seeded variables"):
            self.assertIn(name, names)

    def test_a_wrong_value_fails_that_variable_only(self) -> None:
        names = failed(com1(values={"BOOTORDER": b"\x02\x00", "BOOT0001": OPTION, "ODBOOTCOUNT": EXPECTED["ODBOOTCOUNT"]}))
        self.assertEqual(names, ["efivarfs value BOOTORDER"])

    def test_an_acpi_error_or_an_unknown_ae_line_fails(self) -> None:
        self.assertIn("ACPI without errors", failed(com1(good_lines(["<3>[    1.0] ACPI BIOS Error (bug): AE_NOT_FOUND"]))))
        self.assertIn("ACPI without errors", failed(com1(good_lines(["<4>[    1.0] ACPI: something odd"]))))
        names = failed(com1(good_lines(["<6>[    1.0] pci 0000:00:00.0: _OSC failed (AE_NOT_FOUND)"])))
        self.assertEqual(names, ["no other AE_ line"])

    def test_known_issue_lines_are_allowed_and_listed_but_other_levels_are_not(self) -> None:
        results, _ = boot.evaluate(com1(), REPORT, EXPECTED)
        detail = next(r.detail for r in results if r.name == "ACPI without errors")
        self.assertIn("ISSUE-642", detail)
        self.assertIn("ISSUE-643", detail)
        self.assertIn("no unknown <0>-<3> lines", failed(com1(good_lines(["<3>[    1.0] Oops: something new"]))))
        self.assertIn("no unknown <4> lines", failed(com1(good_lines(["<4>[    1.0] something new"]))))

    def test_the_rtc_error_and_unprefixed_lines_fail(self) -> None:
        names = failed(com1(good_lines(["<3>[    1.4] Unable to read current time from RTC"])))
        self.assertIn("no RTC read error", names)
        self.assertIn("no unknown <0>-<3> lines", names)
        self.assertIn("no unprefixed dmesg lines", failed(com1(good_lines(["[    1.4] no level prefix"]))))


class Gate(unittest.TestCase):
    def test_a_missing_kernel_is_a_failure_under_the_require_flag_and_a_loud_skip_otherwise(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "vmlinuz"
            self.assertEqual(boot.kernel_gate(missing, {"OD_REQUIRE_UBUNTU_VMLINUZ": "1"}), 1)
            self.assertEqual(boot.kernel_gate(missing, {}), boot.SKIP_EXIT)
            self.assertEqual(boot.kernel_gate(missing, {"OD_REQUIRE_UBUNTU_VMLINUZ": "0"}), boot.SKIP_EXIT)
            missing.write_bytes(b"x")
            self.assertIsNone(boot.kernel_gate(missing, {}))


if __name__ == "__main__":
    unittest.main()
