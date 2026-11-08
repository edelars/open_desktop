#!/usr/bin/env python3
"""S11-11 smoke: how the controlling process sees the end of a VMHost (crash, STOP while paused, a second launch), no UI.

The application of S11-11 (`MachineController`) decides the state of a machine from two facts only: the events of control v2 and the
exit status of the process it spawned. This smoke is the controlling process in Python over a socketpair on fd 3 (the same
descriptors as `vmhost_control_v2_lifecycle_smoke.py`, whose `Host` and `Conn` it reuses) and checks those facts on the real
`od-vmhost` (test-only profile, no guest):

  a. SIGSEGV is sent to a running VMHost from outside (VMHost has no sanctioned way to fault itself): the control connection ends
     without `STATE_CHANGED -> stopped` and without an `ERROR`, and the status that the parent collects is "killed by signal 11".
     `derive_end` is the rule that the Swift side applies (`ControlClientExit`, `ErrorCatalog.entry(for:)`): a signal is a crash
     ("stopped, process of the machine ended abnormally"), never a normal stop. The second half of the scenario is the same for
     SIGKILL (signal 9).
  b. `STOP` while `paused`: `STATE_CHANGED paused -> stopped (user)`, `ACK`, EOF, exit status 0 (a normal end, no crash).
  c. Two launches on one bundle: the second VMHost answers `ERROR busy` (fatal, request id 0) instead of `READY` and exits with
     status 3; the first one is not disturbed (STOP still works). After the first one has gone the lock is free again: a third
     launch gets `READY` (the kernel dropped `flock` when the process died; nothing was cleaned by hand).

Usage: python3 tests/system/desktop_vmhost_crash_smoke.py [VMHOST_BINARY]
Default binary: apps/.build/debug/od-vmhost (`cd apps && swift build`; the staticlib first: `cargo build -p od-host --offline`).
Prints `ok` and exits 0 on success. Every scenario uses its own temporary bundle directory, removed at the end (including `lock`).
"""

from __future__ import annotations

import os
import shutil
import signal
import struct
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import vmhost_control_v2_lifecycle_smoke as v2  # noqa: E402  (frame builders, Host, Conn)
from vmhost_control_v2_lifecycle_smoke import (  # noqa: E402
    BUSY, ERROR, FLAG_FATAL, HELLO, PAUSE, PAUSED, RUNNING, START, STOP, STOPPED, USER, Failure, Host, check,
    configure_payload, expect_exit, frame, parse_error,
)

DEFAULT_BINARY = v2.DEFAULT_BINARY
CONFIGURE = v2.CONFIGURE


def start_running(binary: Path, bundle: str, *, pause: bool = False) -> Host:
    """Spawns a VMHost, configures it and brings it to `running` (or `paused`)."""
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        conn.expect_ok(CONFIGURE, configure_payload())
        events, ack = conn.expect_ok(START, struct.pack("<Q", 0))
        check(events == [(STOPPED, RUNNING, USER)] and ack["state"] == RUNNING, "START: stopped -> running")
        if pause:
            events, ack = conn.expect_ok(PAUSE, struct.pack("<II", USER, 0))
            check(events == [(RUNNING, PAUSED, USER)] and ack["state"] == PAUSED, "PAUSE: running -> paused")
    except BaseException:
        host.cleanup()
        raise
    return host


def scenario_signal(binary: Path, bundle: str) -> None:
    for sig in (signal.SIGSEGV, signal.SIGKILL):
        host = start_running(binary, bundle)
        try:
            conn = host.conn
            conn.expect_state(RUNNING, 1, USER)
            os.kill(host.pid, sig)
            # The connection ends. Frames that were already in flight (STATS) are allowed; a state change to `stopped`, an ERROR or
            # an ACK are not: the process never got to say anything.
            saw_stopped = False
            deadline = time.monotonic() + v2.TIMEOUT
            while True:
                check(time.monotonic() < deadline, f"{sig.name}: the control connection did not end")
                item = conn.recv(max(0.1, deadline - time.monotonic()))
                if item is None:
                    break
                kind, _, payload = item
                check(kind != ERROR, f"{sig.name}: ERROR after the signal: {parse_error(payload)}")
                if kind == v2.STATE_CHANGED:
                    _, to, _ = struct.unpack("<III", payload)
                    saw_stopped = saw_stopped or to == STOPPED
            check(not saw_stopped, f"{sig.name}: STATE_CHANGED -> stopped from a killed process")
            status = host.wait()
            check(status == -int(sig), f"{sig.name}: the parent must collect 'killed by signal {int(sig)}', got status {status}")
        finally:
            host.cleanup()
        # The kernel released the lock with the process: a new VMHost on the same bundle is not `busy`.
        again = Host(binary, bundle)
        try:
            again.conn.handshake()
            again.conn.sock.shutdown(v2.socket.SHUT_WR)
            again.conn.expect_closed()
            expect_exit(again, 0)
        finally:
            again.cleanup()


def scenario_stop_while_paused(binary: Path, bundle: str) -> None:
    host = start_running(binary, bundle, pause=True)
    try:
        conn = host.conn
        conn.expect_state(PAUSED, 1, USER)
        events, ack = conn.expect_ok(STOP, struct.pack("<Q", 0))
        check(events == [(PAUSED, STOPPED, USER)] and ack["state"] == STOPPED, f"STOP while paused: paused -> stopped (user), got {events}")
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def scenario_busy(binary: Path, bundle: str) -> None:
    first = start_running(binary, bundle)
    try:
        second = Host(binary, bundle)
        try:
            second.conn.send(frame(HELLO, 0))
            item = second.conn.recv_event()
            check(item is not None and item[0] == ERROR and item[1] == 0, "ERROR instead of READY")
            error = parse_error(item[2])
            check(error["code"] == BUSY and error["failed_kind"] == 0 and error["flags"] & FLAG_FATAL, f"busy: {error}")
            second.conn.expect_closed()
            expect_exit(second, 3)
        finally:
            second.cleanup()
        # The first machine did not notice the second launch.
        first.conn.expect_state(RUNNING, 1, USER)
        events, ack = first.conn.expect_ok(STOP, struct.pack("<Q", 0))
        check(events == [(RUNNING, STOPPED, USER)] and ack["state"] == STOPPED, "STOP of the first machine")
        first.conn.expect_closed()
        expect_exit(first, 0)
    finally:
        first.cleanup()
    # The process is gone: the lock is free, a third launch is accepted (no manual cleaning of `lock`).
    third = Host(binary, bundle)
    try:
        third.conn.handshake()
        third.conn.sock.shutdown(v2.socket.SHUT_WR)
        third.conn.expect_closed()
        expect_exit(third, 0)
    finally:
        third.cleanup()


def main() -> int:
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_BINARY
    check(binary.exists(), f"{binary} does not exist (cd apps && swift build)")
    scenarios = [
        ("signal 11 (and 9) of a running VMHost", scenario_signal),
        ("STOP while paused", scenario_stop_while_paused),
        ("second launch: busy, status 3", scenario_busy),
    ]
    for name, scenario in scenarios:
        bundle = tempfile.mkdtemp(prefix="desktop_crash_smoke_", suffix=".odvm")
        try:
            scenario(binary, bundle)
        except Failure as failure:
            print(f"FAIL {name}: {failure}", file=sys.stderr)
            return 1
        finally:
            shutil.rmtree(bundle, ignore_errors=True)
        print(f"ok   {name}")
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
