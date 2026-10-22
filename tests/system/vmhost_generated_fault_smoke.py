#!/usr/bin/env python3
"""Verify launcher diagnostics for a generated-code fault in its VMHost child."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 3:
        print(
            f"usage: {Path(sys.argv[0]).name} LAUNCHER GENERATED_FAULT_HELPER",
            file=sys.stderr,
        )
        return 2

    launcher = str(Path(sys.argv[1]).resolve())
    helper = str(Path(sys.argv[2]).resolve())
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
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        stdout, stderr = process.communicate()
        raise RuntimeError(
            f"launcher did not return after helper fault: stdout={stdout!r}, stderr={stderr!r}"
        )

    if process.returncode != 1 or "VMHost terminated by SIGSEGV" not in stderr:
        raise RuntimeError(
            "launcher did not report generated-code SIGSEGV: "
            f"exit={process.returncode}, stdout={stdout!r}, stderr={stderr!r}"
        )

    print("launcher reported VMHost terminated by SIGSEGV after generated-code fault")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"generated-code fault smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
