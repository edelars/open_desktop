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
            + ["<6>[    1.1] [drm] Initialized simpledrm 1.0.0 for simple-framebuffer.0 on minor 0",
               "<6>[    0.7] acpi PNP0A08:00: _OSC: platform retains control of PCIe features (AE_ERROR)",
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

    def test_a_missing_simpledrm_line_fails(self) -> None:
        lines = [line for line in good_lines() if "simpledrm" not in line]
        self.assertEqual(failed(com1(lines)), ["simpledrm bound to the GOP framebuffer"])

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
        results, _ = boot.evaluate(com1(good_lines(["<3>[    1.4] Unable to read current time from RTC"])), REPORT, EXPECTED)
        detail = next(r.detail for r in results if r.name == "no RTC read error")
        self.assertIn("Unable to read current time from RTC", detail)
        good, _ = boot.evaluate(com1(), REPORT, EXPECTED)
        self.assertIn("PM: RTC time", next(r.detail for r in good if r.name == "no RTC read error"))
        self.assertIn("no unknown <0>-<3> lines", names)
        self.assertIn("no unprefixed dmesg lines", failed(com1(good_lines(["[    1.4] no level prefix"]))))


import firmware_protocol_trace as ptrace  # noqa: E402

ROM = (0x200000, 0x300000)
LOADED_IMAGE = "5b1b31a1-9562-11d2-8e3f-00a0c969723b"


def raw_guid(text: str) -> str:
    import uuid
    return uuid.UUID(text).bytes_le.hex()


def call(name: str, retired: int, caller: int, guid: str | None = None, variable: str | None = None) -> dict:
    return {"name": name, "retired": retired, "cpl": 0, "caller": hex(caller),
            "guid": None if guid is None else raw_guid(guid), "variable_name": variable}


class Trace(unittest.TestCase):
    def test_arguments_come_from_the_rom_symbols_and_missing_services_are_named(self) -> None:
        symbols = [(0x22aa90, "od_firmware::bs::protocol::locate_protocol"),
                   (0x22ff00, "od_firmware::rt::variable::get_variable")]
        arguments, missing = ptrace.trace_arguments(symbols)
        self.assertIn("LocateProtocol=0x22aa90:guid=0", arguments)
        self.assertIn("GetVariable=0x22ff00:guid=1:name=0:slot=72", arguments)
        # Services that exist only as a runtime table slot need no ROM symbol.
        self.assertIn("GetTime=0x0:slot=24", arguments)
        self.assertIn("ExitBootServices", missing)
        self.assertNotIn("GetTime", missing)
        self.assertEqual(ptrace.system_table_address(b"OD-FW S7-05 Boot Services utilities verified ST=0x0000000007F7FEF0\r\n"), 0x7F7FEF0)
        self.assertIsNone(ptrace.system_table_address(b"no table"))

    def test_calls_fold_by_caller_phase_guid_and_name(self) -> None:
        trace = {"total": 6, "dropped": 0, "calls": [
            call("HandleProtocol", 10, 0x200100, LOADED_IMAGE),          # the firmware's own call
            call("HandleProtocol", 20, 0x7000000, LOADED_IMAGE),         # the stub
            call("HandleProtocol", 21, 0x7000000, LOADED_IMAGE),
            call("GetVariable", 30, 0x7000000, "8be4df61-93ca-11d2-aa0d-00e098032b8c", "SecureBoot"),
            call("ExitBootServices", 40, 0x7000000),
            call("GetVariable", 50, 0xffffffff80000000, "8be4df61-93ca-11d2-aa0d-00e098032b8c", "BootOrder"),
        ]}
        summary = ptrace.summarize(trace, ROM)
        self.assertEqual(summary["exit_boot_services_at_retired"], 40)
        self.assertEqual(summary["calls_by_service_and_caller"]["HandleProtocol"], {"firmware": 1, "guest": 2})
        before = {(r["call"], r["caller"]): r for r in summary["phases"]["before_exit_boot_services"]}
        self.assertEqual(before[("HandleProtocol", "guest")]["calls"], 2)
        self.assertEqual(before[("HandleProtocol", "guest")]["guid_name"], "EFI_LOADED_IMAGE_PROTOCOL")
        after = summary["phases"]["from_exit_boot_services"]
        self.assertEqual([(r["call"], r["variable_name"]) for r in after if r["call"] == "GetVariable"], [("GetVariable", "BootOrder")])

    def test_an_unknown_guid_is_reported_as_unknown(self) -> None:
        summary = ptrace.summarize({"total": 1, "dropped": 0, "calls": [
            call("LocateProtocol", 5, 0x7000000, "12345678-1234-1234-1234-123456789abc")]}, ROM)
        self.assertTrue(summary["phases"]["before_exit_boot_services"][0]["guid_name"].startswith("unknown"))

    def test_the_trace_checks_fail_without_a_guest_call_a_trace_or_a_symbol(self) -> None:
        trace = {"total": 5, "dropped": 0, "calls": [
            call("GetMemoryMap", 10, 0x7000000), call("HandleProtocol", 11, 0x7000000, LOADED_IMAGE),
            call("ExitBootServices", 40, 0x7000000)]}
        trace["calls"] += [call("GetNextVariableName", 60, 0xffffffff80000000, "8be4df61-93ca-11d2-aa0d-00e098032b8c", "A")]
        good = boot.trace_checks(ptrace.summarize(trace, ROM), [], 0x7F7FEF0)
        self.assertTrue(all(r.ok for r in good), [r.line() for r in good])
        # No runtime calls (or no system table) means the slots were not followed.
        self.assertFalse(all(r.ok for r in boot.trace_checks(ptrace.summarize(trace, ROM), [])))
        self.assertFalse(boot.trace_checks(None, [])[0].ok)
        self.assertFalse(boot.trace_checks(ptrace.summarize(trace, ROM), ["LocateProtocol"])[0].ok)
        firmware_only = {"total": 1, "dropped": 0, "calls": [call("HandleProtocol", 11, 0x200100, LOADED_IMAGE)]}
        self.assertFalse(all(r.ok for r in boot.trace_checks(ptrace.summarize(firmware_only, ROM), [])))
        dropped = {**trace, "dropped": 3}
        self.assertFalse(boot.trace_checks(ptrace.summarize(dropped, ROM), [])[0].ok)


class Png(unittest.TestCase):
    def test_a_bgrx_frame_becomes_an_rgb_png(self) -> None:
        import zlib
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "f.png"
            # 2x1 pixels, stride 12: B=0x50 G=0x27 R=0x5e X=0, then white.
            boot.write_png(bytes([0x50, 0x27, 0x5E, 0, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0]), 2, 1, 12, out)
            png = out.read_bytes()
        self.assertTrue(png.startswith(b"\x89PNG\r\n\x1a\n"))
        self.assertEqual(struct.unpack(">II", png[16:24]), (2, 1))
        start = png.index(b"IDAT") + 4
        length = struct.unpack(">I", png[start - 8:start - 4])[0]
        self.assertEqual(zlib.decompress(png[start:start + length]), bytes([0, 0x5E, 0x27, 0x50, 0xFF, 0xFF, 0xFF]))


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
