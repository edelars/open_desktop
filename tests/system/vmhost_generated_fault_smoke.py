#!/usr/bin/env python3
"""Verify launcher diagnostics for a generated-code fault in its VMHost child."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path


def main() -> int:
    if len(sys.argv) not in (3, 5):
        print(
            f"usage: {Path(sys.argv[0]).name} LAUNCHER GENERATED_FAULT_HELPER [--diagnostics-dir DIR]",
            file=sys.stderr,
        )
        return 2

    launcher = str(Path(sys.argv[1]).resolve())
    helper = str(Path(sys.argv[2]).resolve())
    diagnostics_dir = Path(sys.argv[4]).resolve() if len(sys.argv) == 5 else None
    if len(sys.argv) == 5 and sys.argv[3] != "--diagnostics-dir":
        print("expected --diagnostics-dir before diagnostics path", file=sys.stderr)
        return 2
    if diagnostics_dir is not None and (
        diagnostics_dir.is_symlink() or not diagnostics_dir.is_dir()
    ):
        print(f"invalid diagnostics directory: {diagnostics_dir}", file=sys.stderr)
        return 2
    stdout_path = diagnostics_dir / "launcher.stdout.log" if diagnostics_dir else None
    stderr_path = diagnostics_dir / "launcher.stderr.log" if diagnostics_dir else None
    stdout_file = stdout_path.open("x", encoding="utf-8") if stdout_path else subprocess.PIPE
    stderr_file = stderr_path.open("x", encoding="utf-8") if stderr_path else subprocess.PIPE
    process = subprocess.Popen(
        [
            launcher,
            "--vmhost",
            helper,
            "--hex",
            "48 2b c1 75 fb",
            "--rax",
            "3",
            "--rcx",
            "1",
            "--budget",
            "32",
        ],
        stdout=stdout_file,
        stderr=stderr_file,
        text=diagnostics_dir is None,
        start_new_session=diagnostics_dir is not None,
    )
    timed_out = False

    def interrupt(signum, _frame):
        raise RuntimeError(f"generated-fault smoke interrupted by signal {signum}")

    def group_exists() -> bool:
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        return True

    def cleanup_group() -> bool:
        if not group_exists():
            return False
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return False
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline and group_exists():
            time.sleep(0.05)
        if group_exists():
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                return True
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline and group_exists():
            time.sleep(0.05)
        return group_exists()

    try:
        if diagnostics_dir is not None:
            signal.signal(signal.SIGTERM, interrupt)
            signal.signal(signal.SIGINT, interrupt)
        try:
            stdout, stderr = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            timed_out = True
            if diagnostics_dir is not None:
                stdout_file.flush()
                stderr_file.flush()
                observed_stdout = stdout_path.read_text(errors="replace")
                observed_stderr = stderr_path.read_text(errors="replace")
                print(
                    "generated_fault_timeout_diagnostics="
                    + json.dumps(
                        {"launcher_stdout": observed_stdout, "launcher_stderr": observed_stderr},
                        sort_keys=True,
                        separators=(",", ":"),
                    ),
                    flush=True,
                )
            try:
                process.kill()
            except ProcessLookupError:
                pass
            if diagnostics_dir is None:
                stdout, stderr = process.communicate()
            else:
                process.wait()
                stdout, stderr = "", ""
            raise RuntimeError(
                "launcher did not return after helper fault: "
                f"stdout={stdout!r}, stderr={stderr!r}"
            )

        if diagnostics_dir is not None:
            stdout_file.flush()
            stderr_file.flush()
            stdout = stdout_path.read_text(errors="replace")
            stderr = stderr_path.read_text(errors="replace")
        if process.returncode != 1 or "VMHost terminated by SIGSEGV" not in stderr:
            raise RuntimeError(
                "launcher did not report generated-code SIGSEGV: "
                f"exit={process.returncode}, stdout={stdout!r}, stderr={stderr!r}"
            )

        print(
            "generated_fault_observation="
            + json.dumps(
                {
                    "launcher_returncode": process.returncode,
                    "launcher_stdout": stdout,
                    "launcher_stderr": stderr,
                    "expected_reason": "VMHost terminated by SIGSEGV",
                },
                sort_keys=True,
                separators=(",", ":"),
            )
        )
        print("launcher reported VMHost terminated by SIGSEGV after generated-code fault")
        return 0
    finally:
        group_remained = cleanup_group() if diagnostics_dir is not None else False
        process.wait()
        if diagnostics_dir is not None:
            stdout_file.close()
            stderr_file.close()
        if group_remained:
            print("generated_fault_cleanup_failed=helper process group remained", file=sys.stderr)
            if not timed_out and sys.exc_info()[0] is None:
                raise RuntimeError("generated-fault helper process group remained after cleanup")


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"generated-code fault smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
