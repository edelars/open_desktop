#!/usr/bin/env python3
"""S10-09 end-to-end slice: guest probe -> display device -> VMHost -> shared memory -> Metal -> checked frame.

One command runs the whole chain of stage 10 on the own guest code of S10-08:

  1. `open-desktop-display-viewer --vmhost VMHOST --profile probe --dump-frame PATH` starts VMHost itself in the
     test-only mode `--display-smoke probe` (profile OD_DISPLAY_PROFILE_STAGE10_PROBE). VMHost advances the guest in
     quanta through od_display_session_step until HLT, publishes the frame of the device into the shared memory of
     frame transport v3 (FRAME_TRANSPORT_V1.md) and notifies the viewer; the viewer uploads it into a Metal texture and
     renders it offscreen 1:1 (no window).
  2. The script compares the dump with the reference frame named in the S10-08 manifest
     (tests/fixtures/guest/stage10/display_probe_bars_64x48_seed0.json) byte for byte, by SHA-256 of the manifest and through
     tests/system/display_compare.py at tolerance 0 (DISPLAY.md, section 3). It also checks that VMHost reported HLT after
     the instruction count of the manifest (so the pixels came from the guest, not from a built-in pattern) and generation 1.
  3. Without Metal: VMHost-only checks that need no GPU (the instruction budget and the arguments of the mode).

Exit status: 0 = every check passed; 1 = a check failed; 3 = BLOCKED, the viewer reports no Metal device, so the
chain Metal -> checked frame was NOT verified. 3 is never a pass: the script prints no "ok" line then.

Usage: python3 tests/system/display_end_to_end_smoke.py [--repeat N] [VIEWER_BINARY [VMHOST_BINARY]]
Defaults: apps/.build/debug/open-desktop-display-viewer and apps/.build/debug/od-vmhost
(`cargo build -p od-host --offline`, then `cd apps && swift build`).
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_VIEWER = ROOT / "apps" / ".build" / "debug" / "open-desktop-display-viewer"
DEFAULT_VMHOST = ROOT / "apps" / ".build" / "debug" / "od-vmhost"
FIXTURES = ROOT / "tests" / "fixtures" / "guest" / "stage10"
MANIFEST = FIXTURES / "display_probe_bars_64x48_seed0.json"
COMPARE = ROOT / "tests" / "system" / "display_compare.py"
TIMEOUT = 30
EXIT_BLOCKED = 3
NO_METAL = "no Metal device"


class Failure(Exception):
    pass


class Blocked(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def run(command: list[str]) -> subprocess.CompletedProcess[str]:
    """Runs a command in its own process group; whatever is left in the group (the VMHost the viewer spawned) is
    killed and reaped at the end, and a VMHost that outlived the viewer is a failure."""
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


def load_manifest() -> dict:
    manifest = json.loads(MANIFEST.read_text())
    reference = FIXTURES / manifest["reference_frame"]
    check(reference.exists(), f"missing {reference}")
    data = reference.read_bytes()
    check(len(data) == manifest["frame_bytes"], "reference size differs from the manifest")
    check(hashlib.sha256(data).hexdigest() == manifest["expected_frame_sha256"], "reference hash differs from the manifest")
    check(manifest["width"] * manifest["height"] * 4 == manifest["frame_bytes"], "manifest size is inconsistent")
    manifest["reference_bytes"] = data
    manifest["reference_path"] = reference
    return manifest


def hello(version: int = 3) -> bytes:
    return b"ODFT" + struct.pack("<HHII", version, 1, 0, 0)  # kind HELLO, empty payload


def run_vmhost_alone(vmhost: Path, args: list[str], wait_for_exit: bool) -> tuple[int | None, str]:
    """Starts VMHost with its frame socket on fd 3 (like the launcher). With `wait_for_exit` the socket stays open and
    VMHost must end by itself (a failure, not the EOF teardown); returns (exit status or None, stderr)."""
    with tempfile.NamedTemporaryFile(prefix="vmhost_stderr_", delete=False) as stderr_file:
        stderr_path = stderr_file.name
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    child_fd = fcntl.fcntl(child.fileno(), fcntl.F_DUPFD, 10)
    try:
        pid = os.posix_spawn(
            str(vmhost),
            [str(vmhost), "--socket-fd", "3", *args],
            os.environ,
            file_actions=[
                (os.POSIX_SPAWN_DUP2, child_fd, 3),
                (os.POSIX_SPAWN_OPEN, 2, stderr_path, os.O_WRONLY | os.O_TRUNC, 0o600),
            ],
        )
    finally:
        os.close(child_fd)
        child.close()
    status: int | None = None
    try:
        try:
            parent.sendall(hello())
        except OSError:
            pass  # VMHost already refused the arguments and closed its end
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            done, raw = os.waitpid(pid, os.WNOHANG)
            if done:
                status = raw
                break
            time.sleep(0.01)
        if status is None:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
            if wait_for_exit:
                raise Failure(f"VMHost {' '.join(args)} did not end by itself")
        text = Path(stderr_path).read_text(errors="replace")
    finally:
        parent.close()
        os.unlink(stderr_path)
    return (os.waitstatus_to_exitcode(status) if status is not None else None), text


def check_vmhost_without_gpu(vmhost: Path, manifest: dict) -> None:
    """The probe must end in a failure of VMHost, not in a silent no-frame state, when it cannot finish (no GPU needed)."""
    retired = manifest["expected_retired"]
    budget = retired // 2
    code, text = run_vmhost_alone(vmhost, ["--display-smoke", "probe", "--display-probe-budget", str(budget)], True)
    check(code == 1, f"budget {budget} (< {retired} instructions): exit {code}, expected 1; stderr: {text.strip()}")
    check(f"did not reach HLT within {budget} instructions" in text, f"budget exhaustion not reported: {text.strip()}")
    check("published generation" not in text, "a frame was published although the guest did not finish")
    for bad in (["--display-smoke", "probe", "--display-screens", "2"], ["--display-smoke", "pattern", "--display-probe-budget", "5"]):
        code, text = run_vmhost_alone(vmhost, bad, True)
        check(code == 1 and "usage" in text, f"{' '.join(bad)}: exit {code}, expected a usage error; stderr: {text.strip()}")


def check_chain(viewer: Path, vmhost: Path, manifest: dict, workdir: Path) -> None:
    width, height = manifest["width"], manifest["height"]
    dump = workdir / f"frame_{width}x{height}.raw"
    result = run([str(viewer), "--vmhost", str(vmhost), "--profile", "probe", "--dump-frame", str(dump)])
    if result.returncode != 0 and NO_METAL in result.stderr:
        raise Blocked(result.stderr.strip())
    print(result.stdout.strip())
    check(result.returncode == 0, f"viewer exit {result.returncode}: {result.stderr.strip()}")
    check(f"state=SHOWING width={width} height={height}" in result.stdout, f"viewer did not report SHOWING {width}x{height}")
    check(f"generation={manifest['expected_generation']} " in result.stdout, "viewer shows another generation than the manifest")
    # VMHost stderr is inherited by the viewer: HLT after the manifest instruction count proves that the guest produced the frame.
    check(
        f"probe halted after {manifest['expected_retired']} instructions" in result.stderr,
        f"VMHost did not report HLT after {manifest['expected_retired']} instructions: {result.stderr.strip()}",
    )
    check(dump.exists() and dump.stat().st_size == manifest["frame_bytes"], "dump file missing or wrong size")
    actual = dump.read_bytes()
    check(hashlib.sha256(actual).hexdigest() == manifest["expected_frame_sha256"], "dump hash differs from the manifest")
    check(actual == manifest["reference_bytes"], "dump differs from the reference byte for byte")
    compare = run(
        [sys.executable, str(COMPARE), str(manifest["reference_path"]), str(dump), "--width", str(width), "--height", str(height), "--tolerance", "0"]
    )
    print(compare.stdout.strip())
    check(compare.returncode == 0 and "differing_pixels=0" in compare.stdout, f"display_compare.py: exit {compare.returncode}: {compare.stderr.strip()}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--repeat", type=int, default=1, help="number of consecutive runs of the whole chain (all must pass)")
    parser.add_argument("viewer", nargs="?", type=Path, default=DEFAULT_VIEWER)
    parser.add_argument("vmhost", nargs="?", type=Path, default=DEFAULT_VMHOST)
    args = parser.parse_args(argv[1:])
    try:
        check(args.repeat >= 1, "--repeat must be >= 1")
        for path in (args.viewer, args.vmhost, MANIFEST, COMPARE):
            check(path.exists(), f"missing {path}")
        manifest = load_manifest()
        for number in range(1, args.repeat + 1):
            print(f"run {number}/{args.repeat}")
            check_vmhost_without_gpu(args.vmhost, manifest)
            with tempfile.TemporaryDirectory(prefix="display_end_to_end_") as workdir:
                check_chain(args.viewer, args.vmhost, manifest, Path(workdir))
    except Blocked as blocked:
        print(
            f"BLOCKED: {blocked}\nThe VMHost-only checks passed, but the chain Metal -> checked frame was NOT verified; this is not a pass.",
            file=sys.stderr,
        )
        return EXIT_BLOCKED
    except (Failure, subprocess.TimeoutExpired) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print(f"display end-to-end smoke ok ({args.repeat} consecutive run(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
