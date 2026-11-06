#!/usr/bin/env python3
"""Verify the launcher reports a VMHost helper terminated by SIGSEGV."""

from __future__ import annotations

import os
import shlex
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_LAUNCHER = ROOT / "apps" / ".build" / "debug" / "open-desktop-launcher-smoke"
DEFAULT_VMHOST = ROOT / "apps" / ".build" / "debug" / "od-vmhost"


def main() -> int:
    if len(sys.argv) not in (1, 3):
        print(
            f"usage: {Path(sys.argv[0]).name} [LAUNCHER VMHOST]",
            file=sys.stderr,
        )
        return 2

    launcher = str(Path(sys.argv[1] if len(sys.argv) > 1 else DEFAULT_LAUNCHER).resolve())
    vmhost = str(Path(sys.argv[2] if len(sys.argv) > 1 else DEFAULT_VMHOST).resolve())
    with tempfile.TemporaryDirectory(prefix="od-vmhost-signal-") as temporary:
        directory = Path(temporary)
        pid_file = directory / "vmhost.pid"
        wrapper = directory / "vmhost-wrapper.sh"
        wrapper.write_text(
            "#!/bin/sh\n"
            f"printf '%s\\n' \"$$\" > {shlex.quote(str(pid_file))}\n"
            f"exec {shlex.quote(vmhost)} \"$@\"\n",
            encoding="utf-8",
        )
        wrapper.chmod(0o700)

        process = subprocess.Popen(
            [
                launcher,
                "--vmhost",
                str(wrapper),
                "--hex",
                "75 fe",
                "--budget",
                str((1 << 64) - 1),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and process.poll() is None:
                if pid_file.exists():
                    break
                time.sleep(0.02)
            if not pid_file.exists():
                raise RuntimeError("VMHost helper did not start")

            helper_pid = int(pid_file.read_text(encoding="utf-8").strip())
            os.kill(helper_pid, signal.SIGSEGV)
            stdout, stderr = process.communicate(timeout=5)
            if process.returncode != 1 or "VMHost terminated by SIGSEGV" not in stderr:
                raise RuntimeError(
                    "launcher did not report helper SIGSEGV: "
                    f"exit={process.returncode}, stdout={stdout!r}, stderr={stderr!r}"
                )

            print("launcher reported VMHost terminated by SIGSEGV")
            return 0
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
        print(f"vmhost launcher signal smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
