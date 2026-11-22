#!/usr/bin/env python3
"""S10-07 smoke: the display viewer renders the VMHost test pattern offscreen byte for byte.

Runs `open-desktop-display-viewer --vmhost VMHOST --dump-frame PATH` (the viewer starts the
test-only VMHost `--display-smoke pattern` itself, receives the frame through frame transport v1
and renders it through Metal into an offscreen texture without a window), then compares the dump
with tests/fixtures/guest/stage10/bars_64x48_seed0.raw through tests/system/display_compare.py at
tolerance 0 (1:1 mode). Also checks that an unsupported profile and a missing VMHost fail.

Usage: python3 tests/system/display_viewer_dump_smoke.py [VIEWER_BINARY [VMHOST_BINARY]]
Defaults: apps/.build/debug/open-desktop-display-viewer and apps/.build/debug/od-vmhost (`cargo build -p od-host --offline`, then `cd apps && swift build`).
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_VIEWER = ROOT / "apps" / ".build" / "debug" / "open-desktop-display-viewer"
DEFAULT_VMHOST = ROOT / "apps" / ".build" / "debug" / "od-vmhost"
REFERENCE = ROOT / "tests" / "fixtures" / "guest" / "stage10" / "bars_64x48_seed0.raw"
COMPARE = ROOT / "tests" / "system" / "display_compare.py"
TIMEOUT = 30


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def run(command: list[str]) -> subprocess.CompletedProcess[str]:
    """Runs the viewer in its own process group; whatever is left in the group (the VMHost it spawned) is killed
    and reaped at the end, and a VMHost that outlived the viewer is a failure."""
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
    group = process.pid
    try:
        stdout, stderr = process.communicate(timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        os.killpg(group, signal.SIGKILL)
        process.communicate()
        raise
    finally:
        leftover = True
        try:
            os.killpg(group, 0)
        except ProcessLookupError:
            leftover = False
        if leftover:
            os.killpg(group, signal.SIGKILL)
    if leftover:
        raise Failure("a process of the viewer's group (VMHost) outlived the viewer")
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def main(argv: list[str]) -> int:
    viewer = Path(argv[1]) if len(argv) > 1 else DEFAULT_VIEWER
    vmhost = Path(argv[2]) if len(argv) > 2 else DEFAULT_VMHOST
    for path in (viewer, vmhost, REFERENCE):
        check(path.exists(), f"missing {path}")
    try:
        with tempfile.TemporaryDirectory(prefix="display_viewer_dump_") as workdir:
            dump = Path(workdir) / "frame_64x48.raw"
            result = run([str(viewer), "--vmhost", str(vmhost), "--dump-frame", str(dump)])
            print(result.stdout.strip())
            check(result.returncode == 0, f"viewer exit {result.returncode}: {result.stderr.strip()}")
            check("state=SHOWING width=64 height=48" in result.stdout, "viewer did not report SHOWING 64x48")
            check(dump.exists() and dump.stat().st_size == 64 * 48 * 4, "dump file missing or wrong size")
            compare = run(
                [sys.executable, str(COMPARE), str(REFERENCE), str(dump), "--width", "64", "--height", "48", "--tolerance", "0"]
            )
            print(compare.stdout.strip())
            check(compare.returncode == 0, f"display_compare.py exit {compare.returncode}: {compare.stderr.strip()}")
            check("differing_pixels=0" in compare.stdout, "dump differs from the reference")

            # Profiles are `pattern` and `probe` (S10-09, tests/system/display_end_to_end_smoke.py): another one is a usage error,
            # not a silent fallback; `probe` has one screen only.
            result = run([str(viewer), "--vmhost", str(vmhost), "--profile", "unknown", "--dump-frame", str(dump)])
            check(result.returncode == 2, f"unsupported profile exit {result.returncode}, expected 2")
            result = run([str(viewer), "--vmhost", str(vmhost), "--profile", "probe", "--screens", "2", "--dump-frame", str(dump)])
            check(result.returncode == 2, f"probe with two screens exit {result.returncode}, expected 2")
            # A VMHost that cannot start is an error, not a hang.
            result = run([str(viewer), "--vmhost", str(Path(workdir) / "missing"), "--dump-frame", str(dump), "--timeout-ms", "2000"])
            check(result.returncode == 1, f"missing VMHost exit {result.returncode}, expected 1")
    except (Failure, subprocess.TimeoutExpired) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print("display viewer dump smoke ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
