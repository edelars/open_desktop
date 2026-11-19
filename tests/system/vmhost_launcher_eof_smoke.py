#!/usr/bin/env python3
"""Check launcher exit classification when the VMHost control channel closes first."""

import os
from pathlib import Path
import shlex
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time

from vmhost_result_state_smoke import HELLO, READY, RESULT, RESULT_BYTES, RUN, RUN_ACK, frame, read_frame


TIMEOUT_SECONDS = 5
TRANSPORT_ERROR = "control channel closed during a frame"
SCENARIOS = (
    ("signal", "VMHost terminated by SIGSEGV"),
    ("nonzero", f"VMHost exited with status {128 + signal.SIGSEGV}"),
    ("clean", TRANSPORT_ERROR),
    ("live", TRANSPORT_ERROR),
    ("bad-magic", "invalid control frame magic"),
)


def run_helper(mode, fd):
    if mode == "live":
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    with socket.socket(fileno=fd) as channel:
        channel.settimeout(TIMEOUT_SECONDS)
        if read_frame(channel) != (HELLO, 0, b""):
            raise RuntimeError("expected launcher HELLO")
        channel.sendall(frame(READY, 0, struct.pack("<I", 1)))
        kind, request_id, _ = read_frame(channel)
        if (kind, request_id) != (RUN, 1):
            raise RuntimeError("expected launcher RUN")
        channel.sendall(frame(RUN_ACK, 1, struct.pack("<I", 0)))
        if mode == "nonzero":
            # Give the launcher a complete RESULT header and only one payload byte.
            channel.sendall(frame(RESULT, 1, bytes(RESULT_BYTES))[:17])
        elif mode == "bad-magic":
            channel.sendall(b"BAD!" + frame(RESULT, 1, bytes(RESULT_BYTES))[4:])

    if mode == "live":
        while True:
            signal.pause()
    # Make EOF observable before there is any child wait status to collect.
    time.sleep(0.075)
    if mode in ("signal", "bad-magic"):
        signal.signal(signal.SIGSEGV, signal.SIG_DFL)
        os.kill(os.getpid(), signal.SIGSEGV)
        raise RuntimeError("SIGSEGV did not terminate helper")
    return 128 + signal.SIGSEGV if mode == "nonzero" else 0


def check_scenario(launcher, helper, mode, expected):
    command = shlex.join([sys.executable, "-B", str(Path(__file__).resolve()), "--helper", mode])
    helper.write_text(f"#!/bin/sh\nexec {command} \"$@\"\n", encoding="utf-8")
    helper.chmod(0o700)
    env = os.environ.copy()
    env.pop("OD_ISSUE027_TRACE", None)
    process = subprocess.Popen(
        [launcher, "--vmhost", str(helper), "--hex", "75 fe", "--budget", "8"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=TIMEOUT_SECONDS)
        if (process.returncode, stdout, stderr) != (1, "", f"open-desktop-launcher-smoke: {expected}\n"):
            raise RuntimeError(f"{mode}: exit={process.returncode}, stdout={stdout!r}, stderr={stderr!r}")
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            return
        raise RuntimeError(f"{mode}: a launcher child survived cleanup")
    finally:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=TIMEOUT_SECONDS)


def interrupt(signum, _frame):
    raise RuntimeError(f"launcher EOF smoke interrupted by signal {signum}")


def main(args):
    if len(args) == 4 and args[0] == "--helper" and args[2] == "--socket-fd":
        return run_helper(args[1], int(args[3]))
    if len(args) != 1:
        print(f"usage: {Path(__file__).name} LAUNCHER", file=sys.stderr)
        return 2
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    launcher = str(Path(args[0]).resolve())
    with tempfile.TemporaryDirectory(prefix="od-vmhost-launcher-eof-") as directory:
        helper = Path(directory) / "helper"
        for mode, expected in SCENARIOS:
            check_scenario(launcher, helper, mode, expected)
    print("launcher EOF smoke passed: exit classification, bounded cleanup, and malformed-frame precedence")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError) as error:
        print(f"vmhost launcher EOF smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
