#!/usr/bin/env python3
"""Run every firmware check in one command and print a result table (S7-40).

Order: runner build, ROM builds, every tests/system/firmware_*_smoke.py, host tests of
firmware/uefi, the two ignored od-machine acceptance tests (TPM_RC_FAILURE at Startup,
PCR7 replay) and the Python self-tests of the firmware tools. All steps run even after
a failure; the exit code is non-zero if any step failed or was skipped. Child output
goes to per-step logs in <work>/logs; the table shows exit code and a key line.
"""

from __future__ import annotations

import argparse
import fnmatch
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
SYSTEM = ROOT / "tests" / "system"
BUILDER = ROOT / "tools" / "build" / "build_firmware.py"
DISK = ROOT / "tools" / "build" / "make_test_disk.py"
APPS = ROOT / "firmware" / "uefi" / "tests" / "apps"
TARGET = "x86_64-unknown-uefi"
PYTHON = sys.executable


class Step:
    """One table row. run(context, log) raises StepFailed or returns normally."""

    def __init__(self, name, needs, run, script=None):
        self.name, self.needs, self.run, self.script = name, tuple(needs), run, script


class StepFailed(Exception):
    def __init__(self, code, command):
        super().__init__(f"exit {code}: {' '.join(map(str, command))}")
        self.code = code


class Context:
    def __init__(self, work, reset_cycles):
        self.work = work
        self.reset_cycles = reset_cycles
        target = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
        self.runner = ROOT / target / "release" / "od-runner"
        self.stock_rom = work / "stock.bin"
        self.apps_rom = work / "apps.bin"
        self.apps_target = work / "apps-target"
        self.tpm_app = self.apps_target / "test-apps" / TARGET / "release" / "tpm.efi"
        self.sb_work = work / "secure-boot"


def call(command, log, cwd=ROOT, env=None):
    log.write(f"$ {' '.join(map(str, command))}\n")
    log.flush()
    done = subprocess.run([str(part) for part in command], cwd=cwd, env=env,
                          stdout=log, stderr=subprocess.STDOUT, check=False)
    if done.returncode != 0:
        raise StepFailed(done.returncode, command)


def clean_environment():
    """Environment for builds of EFI apps: host rustflags must not reach the UEFI target."""
    env = dict(os.environ)
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    return env


def script_step(name, script, arguments=lambda context: (), needs=("build-runner",)):
    def run(context, log):
        call([PYTHON, SYSTEM / script, *arguments(context)], log)
    return Step(name, needs, run, script=script)


def build_runner(context, log):
    call(["cargo", "build", "--release", "-p", "od-runner", "--offline"], log)


def build_stock_rom(context, log):
    call([PYTHON, BUILDER, "--out", context.stock_rom,
          "--target-dir", context.work / "stock-target"], log)


def build_apps_rom(context, log):
    # The reboot app (--with-reboot-app) is added only by firmware_reset_smoke.py, which builds
    # its own ROM: the flag shifts the memory map of every COM1 fixture (ISSUE-624).
    call([PYTHON, BUILDER, "--with-test-apps", "--out", context.apps_rom,
          "--target-dir", context.apps_target], log)
    if not context.tpm_app.is_file():
        raise StepFailed(1, [f"missing {context.tpm_app}"])


def host_tests(context, log):
    call(["cargo", "test", "--manifest-path", ROOT / "firmware/uefi/Cargo.toml", "--offline"], log)


def tpm_startup_failure(context, log):
    work = context.work / "tpm-failure"
    work.mkdir()
    call(["cargo", "build", "--manifest-path", APPS / "Cargo.toml", "--bin", "tpm-absent",
          "--offline", "--release", "--target", TARGET, "--target-dir", work / "app-target"],
         log, env=clean_environment())
    esp = work / "esp.raw"
    call([PYTHON, DISK, "--esp-fat32", "--bootx64",
          work / "app-target" / TARGET / "release" / "tpm-absent.efi", "--out", esp], log)
    env = dict(os.environ, OD_TPM_FAILURE_ROM=str(context.stock_rom), OD_TPM_FAILURE_ESP=str(esp))
    call(["cargo", "test", "--offline", "--release", "-p", "od-machine", "--lib",
          "firmware_boot_continues_after_tpm_startup_failure", "--", "--ignored", "--nocapture"],
         log, env=env)


def secure_boot_pcr7(context, log):
    env = dict(os.environ, OD_SB_PCR7_ROM=str(context.sb_work / "firmware.bin"),
               OD_SB_PCR7_FIXTURES=str(context.sb_work))
    call(["cargo", "test", "--offline", "--release", "-p", "od-machine", "--lib",
          "firmware_secure_boot_pcr7_replays_setup_enrollment_and_revocation",
          "--", "--ignored", "--nocapture"], log, env=env)


def python_selftests(context, log):
    call([PYTHON, "-m", "unittest", "discover", "-s", "tools/build", "-p", "test_build_firmware.py"], log)
    call([PYTHON, "-m", "unittest", "discover", "-s", "tools/build", "-p", "test_s7_31a_rom_inventory.py"], log)
    call([PYTHON, "-m", "unittest", "discover", "-s", "tools/build", "-p", "test_firmware_check_all.py"], log)
    call([PYTHON, "-m", "unittest", "discover", "-s", "tests/system", "-p", "test_firmware_*.py"], log)


def steps():
    return [
        Step("build-runner", (), build_runner),
        Step("build-rom-stock", (), build_stock_rom),
        Step("build-rom-test-apps", (), build_apps_rom),
        script_step("firmware_boot_smoke", "firmware_boot_smoke.py"),
        script_step("firmware_esp_boot_smoke", "firmware_esp_boot_smoke.py"),
        script_step("firmware_nvram_smoke", "firmware_nvram_smoke.py"),
        script_step("firmware_failure_smoke", "firmware_failure_smoke.py"),
        script_step("firmware_install_media_smoke", "firmware_install_media_smoke.py"),
        script_step("firmware_pkcs7_smoke", "firmware_pkcs7_smoke.py",
                    lambda c: ("--runner", c.runner, "--rom", c.stock_rom),
                    needs=("build-runner", "build-rom-stock")),
        script_step("firmware_secure_boot_smoke", "firmware_secure_boot_smoke.py",
                    lambda c: ("--work", c.sb_work)),
        script_step("firmware_tpm_smoke", "firmware_tpm_smoke.py",
                    lambda c: ("--runner", c.runner, "--rom", c.apps_rom, "--app", c.tpm_app,
                               "--work", c.work / "tpm"),
                    needs=("build-runner", "build-rom-test-apps")),
        script_step("firmware_xhci_smoke", "firmware_xhci_smoke.py",
                    lambda c: ("--runner", c.runner, "--rom", c.apps_rom),
                    needs=("build-runner", "build-rom-test-apps")),
        script_step("firmware_reset_smoke", "firmware_reset_smoke.py",
                    lambda c: ("--cycles", str(c.reset_cycles))),
        Step("host-tests-firmware-uefi", (), host_tests),
        Step("ignored-tpm-startup-failure", ("build-rom-stock",), tpm_startup_failure),
        Step("ignored-secure-boot-pcr7", ("firmware_secure_boot_smoke",), secure_boot_pcr7),
        Step("python-selftests", (), python_selftests),
    ]


KEY_PATTERNS = (r"^OK\b", r"^Ran \d+ tests?", r"sha256:", r"\bboot cycles\b")
FAILURE_PATTERNS = (r"Unsupported \{", r"panicked", r"differs from", r"RuntimeError", r"smoke:", r"exited \d+",
                    r"^error", r"\bFAILED\b", r"Traceback", r"^FAIL")
CARGO_RESULT = re.compile(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored")


def shorten(line):
    """A runner JSON report dumped into an exception message becomes its exit detail and rip."""
    detail = re.search(r"exit_detail['\"]?: ?['\"]([^'\"]+)['\"]", line)
    if detail is None or len(line) <= 160:
        return line[:160]
    rip = re.search(r"rip['\"]?: ?['\"](0x[0-9a-fA-F]+)['\"]", line)
    return f"{line[:50]} ... exit_detail={detail.group(1)}" + (f" rip={rip.group(1)}" if rip else "")


def key_line(text, failed):
    """The most informative single line of a step log (last match, else the last line)."""
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines:
        return "(no output)"
    marker = [line for line in lines if line.startswith("STEP FAILED")]
    if failed:
        lines = [line for line in lines if not line.startswith("STEP FAILED")] or lines
    results = [CARGO_RESULT.search(line) for line in lines]
    results = [match for match in results if match]
    if results and not failed:
        total = [sum(int(match.group(i)) for match in results) for i in (2, 3, 4)]
        return f"cargo test: {total[0]} passed; {total[1]} failed; {total[2]} ignored ({len(results)} result lines)"
    patterns = FAILURE_PATTERNS if failed else KEY_PATTERNS
    for pattern in patterns:
        for line in reversed(lines):
            if re.search(pattern, line, 0 if failed else re.IGNORECASE):
                return shorten(line)
    return shorten((marker or lines)[-1] if failed else lines[-1])


def table(rows):
    header = ("step", "exit", "seconds", "key line")
    widths = [max(len(str(row[i])) for row in (header, *rows)) for i in range(3)]
    out = []
    for row in (header, *rows):
        out.append("  ".join(str(row[i]).ljust(widths[i]) for i in range(3)) + "  " + row[3])
    return "\n".join(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--work", type=Path, help="empty or new directory for ROMs, fixtures and logs")
    parser.add_argument("--reset-cycles", type=int, default=5,
                        help="cycles of firmware_reset_smoke.py (default 5, about 13 minutes)")
    parser.add_argument("--only", action="append", default=[], metavar="GLOB",
                        help="run only steps matching the glob (prerequisites are added)")
    parser.add_argument("--skip", action="append", default=[], metavar="GLOB",
                        help="skip steps matching the glob; a skipped step counts as a failure")
    parser.add_argument("--table-out", type=Path, help="also write the final table to this file")
    parser.add_argument("--list", action="store_true", help="print the step names and exit")
    options = parser.parse_args(argv)
    chosen = steps()
    if options.list:
        print("\n".join(step.name for step in chosen))
        return 0
    if options.only:
        wanted = {step.name for step in chosen if any(fnmatch.fnmatch(step.name, g) for g in options.only)}
        for step in reversed(chosen):
            if step.name in wanted:
                wanted.update(step.needs)
        chosen = [step for step in chosen if step.name in wanted]
    work = options.work.resolve() if options.work else Path(tempfile.mkdtemp(prefix="od-firmware-check-"))
    work.mkdir(parents=True, exist_ok=True)
    if any(work.iterdir()):
        parser.error("--work must be empty")
    logs = work / "logs"
    logs.mkdir()
    context = Context(work, options.reset_cycles)
    print(f"work directory: {work}", flush=True)
    results, status = [], {}
    started = time.monotonic()
    for number, step in enumerate(chosen, 1):
        label = f"[{number}/{len(chosen)}] {step.name}"
        if any(g and fnmatch.fnmatch(step.name, g) for g in options.skip):
            results.append((step.name, "skipped", "0", "skipped by --skip (counts as failure)"))
            status[step.name] = False
            print(f"{label}: skipped", flush=True)
            continue
        broken = [need for need in step.needs if not status.get(need, False)]
        if broken:
            results.append((step.name, "skipped", "0", f"prerequisite failed: {', '.join(broken)}"))
            status[step.name] = False
            print(f"{label}: skipped, prerequisite failed: {', '.join(broken)}", flush=True)
            continue
        print(f"{label}: start", flush=True)
        begin, code = time.monotonic(), 0
        log_path = logs / f"{step.name}.log"
        with log_path.open("w", encoding="utf-8", errors="replace") as log:
            try:
                step.run(context, log)
            except StepFailed as error:
                code = error.code or 1
                log.write(f"\nSTEP FAILED: {error}\n")
            except Exception as error:  # a broken step must not hide the others
                code = 1
                log.write(f"\nSTEP FAILED: {type(error).__name__}: {error}\n")
        seconds = round(time.monotonic() - begin)
        text = log_path.read_text(encoding="utf-8", errors="replace")
        status[step.name] = code == 0
        results.append((step.name, str(code), str(seconds), key_line(text, code != 0)))
        print(f"{label}: exit {code} in {seconds} s", flush=True)
        if code != 0:
            print("--- last log lines:\n" + "\n".join(text.splitlines()[-25:]) + "\n---", flush=True)
    failed = [row for row in results if row[1] != "0"]
    summary = table(results)
    summary += f"\n\ntotal {round(time.monotonic() - started)} s; {len(results) - len(failed)} of {len(results)} steps passed"
    summary += f"\nlogs: {logs}"
    print("\n" + summary, flush=True)
    if options.table_out:
        options.table_out.parent.mkdir(parents=True, exist_ok=True)
        options.table_out.write_text(summary + "\n", encoding="utf-8")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
