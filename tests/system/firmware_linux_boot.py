#!/usr/bin/env python3
"""S7-31 acceptance: Ubuntu 7.0.0-38 boots through our UEFI to /init, one unified-loop run (stock kernel).

Every check prints PASS or FAIL with its evidence; nothing is skipped silently. Exit codes: 0 all checks
passed, 1 a check failed or an input is missing under OD_REQUIRE_UBUNTU_VMLINUZ=1, 77 the kernel is not
staged and OD_REQUIRE_UBUNTU_VMLINUZ is not 1 (a loud skip, never a pass). The host test of the loader on
the real kernel (cargo test ... ubuntu_kernel) is run here with OD_REQUIRE_UBUNTU_VMLINUZ=1 and must pass.

Checks: the host loader test; the run ends at its budget (no stop); the kernel and firmware banners; the
/init lines (Run /init, markers, the normal end of the stage 4 style init); efivarfs mounted with the
BootOrder, Boot0001 and ODBootCount values equal to the seed; ACPI tables parsed and no ACPI error by
level (<0>-<3> errors and <4> warnings must be on the explicit known-issue lists below, anything else fails);
no RTC read error; all other <0>-<3> lines on the known list (ISSUE-644). Recorded but not a pass
criterion: the framebuffer dump of the display at the end of the run, and the protocols the loader asked
for (NOT TAKEN: the firmware does not log them).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests" / "system"))
import firmware_linux_uefi_efivars_acpi as rec  # noqa: E402
import firmware_protocol_trace as ptrace  # noqa: E402

stop = rec.stop
esp_smoke = rec.esp_smoke
SKIP_EXIT = 77
STAMP = re.compile(r"^<\d>\[\s*[\d.]+\]\s*")
# Known-issue lines, matched without the level and timestamp prefix. Anything else fails the check.
KNOWN_ACPI = {
    r"acpi PNP0A08:00: _OSC: platform retains control of PCIe features \(AE_ERROR\)": "ISSUE-642 (deferred)",
    r"ACPI: setting ELCR to 0200 \(from 0000\)": "ISSUE-643",
}
KNOWN_ERRORS = {
    r"CPU: vendor_id 'OpenDesktop ' unknown, using generic init\.": "ISSUE-644",
    r"CPU: Your system may be unstable\.": "ISSUE-644",
    r"KHO: Failed to reserve lowmem scratch buffer": "ISSUE-644",
    r"mce: Unable to init MCE device \(rc: -5\)": "ISSUE-644",
    r"microcode: no support for this CPU vendor": "ISSUE-644",
}
KNOWN_WARNINGS = {
    r"KHO: Failed to reserve scratch area, disabling kexec handover": "ISSUE-644",
    **{pattern: issue for pattern, issue in KNOWN_ACPI.items() if "ELCR" in pattern},
}
ACPI_TABLES = ("RSDP", "XSDT", "FACP", "DSDT", "APIC", "MCFG")
INIT_PANIC = "Kernel panic - not syncing: Attempted to kill init! exitcode=0x00000000"


class Result:
    def __init__(self, name: str, ok: bool, detail: str) -> None:
        self.name, self.ok, self.detail = name, ok, detail

    def line(self) -> str:
        return f"{'PASS' if self.ok else 'FAIL'} {self.name}: {self.detail}"


def known(line: str, table: dict[str, str]) -> str | None:
    text = STAMP.sub("", line)
    return next((issue for pattern, issue in table.items() if re.fullmatch(pattern, text)), None)


def kernel_gate(kernel: Path, environ: dict[str, str]) -> int | None:
    """None: the kernel is there. Otherwise the exit code of the run (1 under the require flag, else 77)."""
    if kernel.is_file():
        return None
    required = environ.get("OD_REQUIRE_UBUNTU_VMLINUZ") == "1"
    print(f"{'FAIL' if required else 'SKIPPED'} kernel: {kernel} is not staged"
          + ("" if required else " (set OD_REQUIRE_UBUNTU_VMLINUZ=1 to make this a failure)"), file=sys.stderr)
    return 1 if required else SKIP_EXIT


def evaluate(com1: bytes, report: dict, expected: dict[str, bytes]) -> tuple[list[Result], dict]:
    """The COM1 checks. Pure: no files, no processes."""
    analysis = rec.analyse(com1, expected)
    text = rec.undo_onlcr(com1).decode("latin1")
    results: list[Result] = []

    def check(name: str, ok: bool, detail: str) -> None:
        results.append(Result(name, bool(ok), detail))

    check("run ends at its budget", report.get("exit") == "budget_exhausted",
          f"exit={report.get('exit')} detail={report.get('exit_detail')} retired={report.get('retired')}")
    version = re.search(r"Linux version (\S+)", text)
    check("kernel banner", version is not None and version.group(1).startswith("7.0.0-38-generic"),
          f"Linux version {version.group(1) if version else None}")
    check("firmware banner", "OD-FW version " in text, "OD-FW version line" + (" present" if "OD-FW version " in text else " missing"))
    markers = analysis["markers"]
    check("/init reached", "Run /init as init process" in text and markers["OD_S7_31N_BEGIN"],
          "'Run /init as init process' and OD_S7_31N_BEGIN")
    check("/init ran to its end", markers["OD_S7_31N_END"] and INIT_PANIC in text,
          f"OD_S7_31N_END and '{INIT_PANIC}' (init exit 0)")
    check("efivarfs mounted", analysis["efivarfs_mounted"], "OD_S7_31N_EFIVARFS_MOUNT_OK")
    names = analysis["efivars_names"] or []
    wanted = list(rec.VARIABLES)
    check("efivarfs lists the seeded variables", all(name in names for name in wanted),
          f"{len(names)} names; missing={[n for n in wanted if n not in names]}")
    for short, entry in (analysis["variables"] or {}).items():
        check(f"efivarfs value {short}", entry.get("dumped") and entry.get("attributes_ok") and entry.get("value_ok"),
              f"attributes={entry.get('attributes')} value={entry.get('value_hex', '')[:48]} expected={entry.get('expected_value_hex', '')[:48]}")
    acpi = analysis["dmesg_acpi_lines"]
    missing = [table for table in ACPI_TABLES if not any(f"ACPI: {table} 0x" in line for line in acpi)]
    check("ACPI tables parsed", not missing, f"missing={missing}")
    severity = analysis["dmesg_severity"]
    bad_acpi, used = [], {}
    for line in acpi:
        level = re.match(r"^<(\d)>", line)
        if level and int(level.group(1)) <= 4 or "AE_" in line or re.search(r"ACPI (BIOS )?(Error|Warning)", line):
            issue = known(line, KNOWN_ACPI)
            if issue:
                used[STAMP.sub("", line)] = issue
            else:
                bad_acpi.append(line)
    check("ACPI without errors", not bad_acpi, f"unexpected={bad_acpi}; known and allowed={used}")
    other_ae = [line for line in severity["ae_lines"] if not known(line, KNOWN_ACPI)]
    check("no other AE_ line", not other_ae, f"unexpected={other_ae}")
    rtc_errors = [line for line in text.splitlines() if "Unable to read" in line]
    rtc_lines = analysis["dmesg_rtc_lines"]
    drm = [STAMP.sub("", line) for line in text.splitlines() if "Initialized simpledrm" in line]
    check("simpledrm bound to the GOP framebuffer", bool(drm), f"lines={drm[:2]}")
    check("no RTC read error", not rtc_errors,
          f"offending lines={rtc_errors}" if rtc_errors else
          f"no line with 'Unable to read'; rtc lines in dmesg={[STAMP.sub('', line) for line in rtc_lines]}")
    err_bad = [line for line in severity["errors_level_0_to_3"] if not known(line, KNOWN_ERRORS)]
    warn_bad = [line for line in severity["warnings_level_4"] if not known(line, KNOWN_WARNINGS)]
    check("no unknown <0>-<3> lines", not err_bad, f"unexpected={err_bad}")
    check("no unknown <4> lines", not warn_bad, f"unexpected={warn_bad}")
    check("no unprefixed dmesg lines", "none" not in severity["levels"], f"levels={severity['levels']}")
    return results, analysis


def trace_checks(summary: dict | None, missing: list[str], system_table: int | None = None) -> list[Result]:
    """The call trace checks (S7-31q). Pure: they read the folded trace, nothing else."""
    results: list[Result] = []

    def check(name: str, ok: bool, detail: str) -> None:
        results.append(Result(name, bool(ok), detail))

    if summary is None:
        check("call trace present", False, "the runner returned no call trace")
        return results
    guest = [row for phase in summary["phases"].values() for row in phase if row["caller"] == "guest"]
    check("call trace present", summary["total_calls"] > 0 and summary["dropped"] == 0 and not missing,
          f"total={summary['total_calls']} dropped={summary['dropped']} services without a ROM symbol={missing}")
    check("guest called GetMemoryMap and ExitBootServices",
          any(r["call"] == "GetMemoryMap" for r in guest) and summary["exit_boot_services_at_retired"] is not None,
          f"ExitBootServices by the guest at retired={summary['exit_boot_services_at_retired']}")
    # The runtime services are traced through the table slots (the runtime driver replaced the ROM functions):
    # the kernel lists the variables for efivarfs after ExitBootServices with GetNextVariableName.
    runtime = [r for r in summary["phases"]["from_exit_boot_services"]
               if r["caller"] == "guest" and r["call"] in ("GetNextVariableName", "GetVariable")]
    check("runtime services traced through the table slots", system_table is not None and bool(runtime),
          f"system table={None if system_table is None else hex(system_table)}; guest runtime calls after ExitBootServices="
          f"{[(r['call'], r['calls']) for r in runtime]}")
    protocols = [r for r in guest if r["call"] in ("HandleProtocol", "OpenProtocol", "LocateProtocol", "LocateHandleBuffer",
                                                   "LocateHandle", "LocateDevicePath")]
    check("guest asked for protocols by GUID", bool(protocols) and all("guid" in r for r in protocols),
          f"{[(r['call'], r.get('guid_name')) for r in protocols][:12]}")
    return results


def write_png(data: bytes, width: int, height: int, stride: int, out: Path) -> None:
    """BGRX8888 (display format 1: byte 0 = B, 1 = G, 2 = R, 3 = X) to an RGB PNG."""
    import zlib

    def chunk(kind: bytes, body: bytes) -> bytes:
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    raw = bytearray()
    for y in range(height):
        raw.append(0)
        row = data[y * stride: y * stride + width * 4]
        for x in range(width):
            raw += bytes((row[4 * x + 2], row[4 * x + 1], row[4 * x]))
    out.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                    + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def framebuffer_note(path: Path, report: dict) -> dict:
    info = report.get("framebuffer")
    if info is None or not path.is_file():
        return {"taken": False, "reason": "the runner reported no framebuffer"}
    data = path.read_bytes()
    nonzero = sum(1 for byte in data if byte)
    pixels = struct.unpack(f"<{len(data) // 4}I", data[: len(data) // 4 * 4])
    counts: dict[int, int] = {}
    for pixel in pixels:
        counts[pixel] = counts.get(pixel, 0) + 1
    top = sorted(counts.items(), key=lambda item: -item[1])[:8]
    return {"taken": True, "distinct_pixel_values": len(counts),
            "top_pixel_values": [{"value": f"0x{value:08x}", "pixels": count} for value, count in top], "mode": info, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "nonzero_bytes": nonzero, "all_zero": nonzero == 0,
            "note": "the display frame at the end of the run (guest console is ttyS0); not a screenshot claim"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--stage4-dir", type=Path, default=ROOT / "work" / "stage4")
    parser.add_argument("--work-dir", type=Path, default=None)
    parser.add_argument("--out", type=Path, default=None, help="JSON record to write")
    parser.add_argument("--budget", type=int, default=6_000_000_000)
    parser.add_argument("--png-out", type=Path, default=None, help="write the framebuffer dump as a PNG (BGRX8888 only)")
    parser.add_argument("--trace-out", type=Path, default=None, help="write the protocol and service call trace as JSON")
    args = parser.parse_args()
    kernel = args.stage4_dir / "linux-7.0.0-38.38" / "vmlinuz-7.0.0-38-generic"
    gate = kernel_gate(kernel, dict(os.environ))
    if gate is not None:
        raise SystemExit(gate)
    results: list[Result] = []
    host = subprocess.run(["cargo", "test", "--manifest-path", "firmware/uefi/Cargo.toml", "--offline", "--lib",
                           "ubuntu_kernel"], cwd=ROOT, capture_output=True, text=True,
                          env={**os.environ, "OD_REQUIRE_UBUNTU_VMLINUZ": "1"}, check=False)
    results.append(Result("host loader test on the real kernel", host.returncode == 0 and " 1 passed" in host.stdout,
                          f"exit={host.returncode} " + next((l for l in host.stdout.splitlines() if l.startswith("test result")), "no result line")))
    subprocess.run([sys.executable, rec.BUILD_INITRAMFS, "--out-dir", rec.DEFAULT_INITRAMFS.parent], check=True, cwd=ROOT,
                   stdout=subprocess.DEVNULL)
    runner = ROOT / Path(os.environ.get("CARGO_TARGET_DIR", "target")) / "release" / "od-runner"
    subprocess.run(["cargo", "build", "--release", "-p", "od-runner", "--offline"], check=True, cwd=ROOT)
    with tempfile.TemporaryDirectory(prefix="od-linux-boot-") as temporary:
        scratch = args.work_dir or Path(temporary)
        scratch.mkdir(parents=True, exist_ok=True)
        rom = scratch / "stock.bin"
        subprocess.run([sys.executable, stop.BUILDER, "--out", rom, "--target-dir", scratch / "rom"], check=True,
                       cwd=ROOT, stdout=subprocess.DEVNULL)
        option = esp_smoke.load_option(esp_smoke.file_node("BOOTX64.EFI"), (stop.CMDLINE + "\0").encode("utf-16le"))
        nvram = scratch / "seed.nv"
        nvram.write_bytes(rec.snapshot([
            ("Boot0001", esp_smoke.GLOBAL, option),
            ("BootOrder", esp_smoke.GLOBAL, struct.pack("<H", 1)),
            ("ODBootCount", rec.OD_VENDOR, struct.pack("<I", rec.ODBOOTCOUNT)),
        ]))
        expected = {"BOOTORDER": struct.pack("<H", 1), "BOOT0001": option, "ODBOOTCOUNT": struct.pack("<I", rec.ODBOOTCOUNT)}
        esp = scratch / "esp.raw"
        subprocess.run([sys.executable, stop.DISK, "--esp-fat32", "--bootx64", kernel, "--boot-file",
                        f"INITRD.IMG={rec.DEFAULT_INITRAMFS}", "--out", esp], check=True, cwd=ROOT)
        framebuffer = scratch / "framebuffer.bin"
        elf = next((scratch / "rom").glob("fv/*/x86_64-unknown-none/release/od-firmware"))
        trace_arguments, missing_services = ptrace.trace_arguments(stop.symbols(elf))
        # The runtime driver replaces the ROM runtime services: the trace follows the table slots from the
        # system table, whose address the firmware prints early (deterministic for this ROM and board).
        _, early_com1 = stop.boot(runner, rom, esp, nvram, scratch, "early", 300_000_000, [], "unified", None)
        system_table = ptrace.system_table_address(early_com1)
        if system_table is not None:
            trace_arguments += ["--trace-system-table", f"0x{system_table:x}"]
        report, com1 = stop.boot(runner, rom, esp, nvram, scratch, "stock-0", args.budget, [], "unified", None,
                                 ("--framebuffer-out", str(framebuffer), *trace_arguments))
        checks, analysis = evaluate(com1, report, expected)
        results += checks
        raw_trace = (report.get("unified") or {}).get("call_trace")
        rom_range = (stop.LINK_BASE, stop.LINK_BASE + rom.stat().st_size)
        summary = ptrace.summarize(raw_trace, rom_range) if raw_trace else None
        results += trace_checks(summary, missing_services, system_table)
        note = framebuffer_note(framebuffer, report)
        if args.png_out and note["taken"] and note["mode"]["format"] == 1:
            write_png(framebuffer.read_bytes(), note["mode"]["width"], note["mode"]["height"], note["mode"]["stride"],
                      args.png_out)
            note["png"] = args.png_out.name
        for result in results:
            print(result.line(), flush=True)
        print(f"INFO framebuffer: {note}")
        print(f"INFO call trace: {None if summary is None else {k: v for k, v in summary.items() if k != 'phases'}}")
        failed = [r for r in results if not r.ok]
        if args.trace_out and summary is not None:
            args.trace_out.parent.mkdir(parents=True, exist_ok=True)
            args.trace_out.write_text(json.dumps({
                "schema": "od-s7-31q-protocol-trace-v1",
                "inputs": {"vmlinuz_sha256": stop.sha256(kernel), "initramfs_sha256": stop.sha256(rec.DEFAULT_INITRAMFS),
                           "firmware_rom_sha256": stop.sha256(rom), "nvram_seed_sha256": stop.sha256(nvram),
                           "cmdline": stop.CMDLINE, "loader": "kernel EFI stub as BOOTX64.EFI (no shim or grub)"},
                "run": {key: report.get(key) for key in ("exit", "exit_detail", "rip", "retired", "quanta")},
                "com1_sha256": hashlib.sha256(com1).hexdigest(),
                "method": "the runner watches the entry (first instruction) of the ROM functions in services, by physical "
                          "address, and records the GUID and UCS-2 name the arguments point to (Microsoft x64 ABI); "
                          "the caller is the return address at [RSP] (ROM range = firmware itself, otherwise guest code); "
                          "values are never read",
                "services": {name: {"rom_symbol": symbol, "runtime_table_slot": slot, "guid_arg": g, "name_arg": n}
                             for name, (symbol, slot, g, n) in ptrace.SERVICES.items()},
                "system_table": None if system_table is None else hex(system_table),
                "services_without_a_rom_symbol": missing_services,
                "rom_address_range": [hex(rom_range[0]), hex(rom_range[1])],
                "not_traced": "fields the guest reads directly from the system table or boot services table (for example "
                              "ConOut, ConIn, RuntimeServices) are not calls and are not recorded; services outside the list "
                              "above are not traced",
                "guid_names_source": "table in tests/system/firmware_protocol_trace.py (UEFI 2.11 names; several names come from "
                                     "the Linux kernel's include/linux/efi.h and libstub/tpm.c, marked in each name); an unknown "
                                     "GUID is reported as unknown",
                "trace": summary,
            }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        if args.out:
            args.out.parent.mkdir(parents=True, exist_ok=True)
            args.out.write_text(json.dumps({
                "schema": "od-s7-31p-linux-boot-acceptance-v1",
                "verdict": "FAIL" if failed else "PASS",
                "inputs": {"vmlinuz_sha256": stop.sha256(kernel), "initramfs_sha256": stop.sha256(rec.DEFAULT_INITRAMFS),
                           "firmware_rom_sha256": stop.sha256(rom), "nvram_seed_sha256": stop.sha256(nvram),
                           "cmdline": stop.CMDLINE, "loader": "kernel EFI stub as BOOTX64.EFI (no shim or grub)"},
                "run": {key: report.get(key) for key in ("exit", "exit_detail", "rip", "retired", "quanta", "com1_bytes")},
                "com1_sha256": hashlib.sha256(com1).hexdigest(),
                "checks": [{"name": r.name, "ok": r.ok, "detail": r.detail} for r in results],
                "known_issue_lists": {"acpi": KNOWN_ACPI, "errors": KNOWN_ERRORS, "warnings": KNOWN_WARNINGS},
                "efivars_names": analysis["efivars_names"], "variables": analysis["variables"],
                "acpi_table_lines": [l for l in analysis["dmesg_acpi_lines"] if any(f"ACPI: {t} 0x" in l for t in (*ACPI_TABLES, "FACS"))],
                "dmesg_severity": {k: v for k, v in analysis["dmesg_severity"].items()},
                "dmesg_rtc_lines": analysis["dmesg_rtc_lines"],
                "framebuffer": note,
                "protocol_trace": summary,
            }, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"verdict: {'FAIL' if failed else 'PASS'} ({len(results) - len(failed)}/{len(results)} checks)")
        raise SystemExit(1 if failed else 0)


if __name__ == "__main__":
    main()
