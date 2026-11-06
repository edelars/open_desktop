#!/usr/bin/env python3
"""S11-08 smoke: `PREPARE_DISK` of control protocol v2 against `od-vmhost --control-v2` (CONTROL_PROTOCOL_V2.md section 9).

Acts as the controlling process over a socketpair on fd 3. A temporary bundle (`<name>.odvm` with `disks/`, `logs/` and a
minimal `config.plist`, the shape of VM_BUNDLE.md 5.1 steps 1 and 4) is given to the VMHost with `--bundle`. Scenarios:

  * HELLO, CONFIGURE (without `disks`: the image does not exist yet, VM_BUNDLE 5.1 step 3), `PREPARE_DISK disk0.img` of 1 MiB:
    `PROGRESS` (completed == total) then `ACK`; `os.stat` of `disks/disk0.img` shows a regular file of 1 MiB, mode 0600;
  * the same request ID again in the live connection: a status (`ACK` with the replayed flag), no second creation, same
    request ID with another kind: `request_id_conflict`;
  * an existing name: `disk_exists` and the file is untouched; names `..`, `.`, `a/b`, a control character, invalid UTF-8:
    `invalid_name`; capacity not a multiple of 512: `invalid_argument` (aux0 = 8); capacity above i64::MAX: `invalid_argument`;
    zero capacity and format other than raw: `invalid_argument`; capacity 0x7FFFFFFFFFFFFE00: `insufficient_space` with the
    required and available bytes, nothing created;
  * `PREPARE_DISK` while `running` (START of a test-only profile): `invalid_state`; after STOP the process exits with status 0;
  * a bundle without `disks/`: `io_error` (aux0 = errno); `disks` as a symlink: `io_error` (the link is not followed).

Usage: python3 tests/system/vmhost_control_v2_prepare_disk_smoke.py [VMHOST_BINARY]
Default binary: apps/.build/debug/od-vmhost (`cargo build -p od-host --offline`, then `cd apps && swift build`).
Prints `ok` and exits 0 on success.
"""

from __future__ import annotations

import os
import plistlib
import shutil
import socket
import stat
import struct
import sys
import tempfile
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import vmhost_control_v2_lifecycle_smoke as v2  # noqa: E402  (frame builders, Host, Conn)
from vmhost_control_v2_lifecycle_smoke import (  # noqa: E402
    ACK, ERROR, INVALID_ARGUMENT, INVALID_NAME, INVALID_STATE, PREPARE_DISK, PROGRESS, QUERY, RUNNING, STOPPED,
    Failure, Host, check, configure_payload, expect_exit, frame, parse_ack, parse_error,
)

DEFAULT_BINARY = v2.DEFAULT_BINARY
MIB = 1 << 20
DISK_EXISTS, INSUFFICIENT_SPACE, IO_ERROR = 15, 16, 24
REQUEST_ID_CONFLICT = 22
I64_MAX = (1 << 63) - 1
BIG_CAPACITY = 0x7FFFFFFFFFFFFE00  # i64::MAX rounded down to 512: valid for the ABI, above any free space


def prepare_payload(name: bytes, capacity: int, fmt: int = 1) -> bytes:
    return struct.pack("<HHIQ", fmt, len(name), 0, capacity) + name


def make_bundle(root: str, with_disks: bool = True) -> str:
    """`<root>/Smoke.odvm` with `disks/`, `logs/` and a minimal config.plist (the VMHost reads only the directory)."""
    bundle = os.path.join(root, "Smoke.odvm")
    os.mkdir(bundle, 0o700)
    if with_disks:
        os.mkdir(os.path.join(bundle, "disks"), 0o700)
    os.mkdir(os.path.join(bundle, "logs"), 0o700)
    config = {
        "schema_version": 1,
        "identity": str(uuid.uuid4()),
        "name": "Smoke",
        "machine_profile": "display-test-pattern",
        "memory_mib": 4096,
        "vcpu_count": 1,
        "disks": [],
        "display": {"screens": 1},
        "network": "none",
        "audio": False,
        "shared_folders": [],
        "clipboard_sync": False,
    }
    with open(os.path.join(bundle, "config.plist"), "wb") as handle:
        plistlib.dump(config, handle, fmt=plistlib.FMT_XML)
    os.chmod(os.path.join(bundle, "config.plist"), 0o600)
    return bundle


def disk_entries(bundle: str) -> list[str]:
    return sorted(os.listdir(os.path.join(bundle, "disks")))


def prepare_ok(conn, name: bytes, capacity: int):
    """One PREPARE_DISK that succeeds: returns (progress frames, ack); the last event before ACK is the final PROGRESS."""
    rid = conn.next_id
    conn.next_id += 1
    conn.send(frame(PREPARE_DISK, rid, prepare_payload(name, capacity)))
    progress = []
    while True:
        item = conn.recv_event()
        check(item is not None, "EOF while waiting for the answer to PREPARE_DISK")
        kind, got_rid, payload = item
        check(got_rid == rid, f"answer carries request id {got_rid}, expected {rid}")
        if kind == PROGRESS:
            request_kind, reserved, completed, total = struct.unpack("<IIQQ", payload)
            check(request_kind == PREPARE_DISK and reserved == 0, "PROGRESS header")
            progress.append((completed, total))
            continue
        check(kind == ACK, f"ACK expected, got kind {kind}: {parse_error(payload) if kind == ERROR else ''}")
        ack = parse_ack(payload)
        check(ack["code"] == 0 and ack["kind"] == PREPARE_DISK and ack["state"] == STOPPED, f"ACK of PREPARE_DISK: {ack}")
        return progress, ack, rid


def scenario_create_and_replay(binary: Path, bundle: str) -> None:
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        conn.expect_ok(v2.CONFIGURE, configure_payload())
        progress, _, rid = prepare_ok(conn, b"disk0.img", MIB)
        check(progress and progress[-1] == (MIB, MIB), f"final PROGRESS is (1 MiB, 1 MiB), got {progress}")
        check(all(0 <= done <= total == MIB for done, total in progress), "PROGRESS values stay within total")
        path = os.path.join(bundle, "disks", "disk0.img")
        info = os.stat(path)
        check(stat.S_ISREG(info.st_mode) and info.st_size == MIB, f"disks/disk0.img: regular file of 1 MiB, got size {info.st_size}")
        check(stat.S_IMODE(info.st_mode) == 0o600, f"mode 0600, got {oct(stat.S_IMODE(info.st_mode))}")
        check(disk_entries(bundle) == ["disk0.img"], f"only the image appears in disks/: {disk_entries(bundle)}")
        with open(path, "rb") as handle:
            check(handle.read(MIB) == bytes(MIB), "the image is zero-filled")
        # Replay in the live connection: the same request ID and kind is a status (ACK, replayed), not a second creation.
        before = os.stat(path)
        conn.send(frame(PREPARE_DISK, rid, prepare_payload(b"disk0.img", MIB)))
        item = conn.recv_event()
        check(item is not None and item[0] == ACK and item[1] == rid, "replay answers ACK with the same request id")
        ack = parse_ack(item[2])
        check(ack["code"] == 0 and ack["flags"] & v2.FLAG_REPLAYED, f"replay carries the replayed flag: {ack}")
        after = os.stat(path)
        check((before.st_ino, before.st_size, before.st_mtime_ns) == (after.st_ino, after.st_size, after.st_mtime_ns),
              "replay does not touch the image")
        # Same request ID, another kind: request_id_conflict.
        conn.send(frame(QUERY, rid))
        item = conn.recv_event()
        check(item is not None and item[0] == ERROR, "ERROR expected for a conflicting kind")
        check(parse_error(item[2])["code"] == REQUEST_ID_CONFLICT, "request_id_conflict")
        # The machine is still stopped and configured; a second, different image works.
        conn.expect_state(STOPPED, 1, 0)
        prepare_ok(conn, b"disk1.img", 4096)
        check(os.stat(os.path.join(bundle, "disks", "disk1.img")).st_size == 4096, "second image of 4096 bytes")
        # A machine whose `disks` is empty is stopped with STOP (no durability refusal), the process exits 0.
        conn.send(frame(v2.STOP, conn.next_id, struct.pack("<Q", 0)))
        conn.next_id += 1
        item = conn.recv_event()
        check(item is not None and item[0] == ACK, "STOP of the stopped machine")
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def scenario_refusals(binary: Path, bundle: str) -> None:
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        # PREPARE_DISK before CONFIGURE is allowed (section 9: stopped, configured or not).
        prepare_ok(conn, b"disk0.img", MIB)
        path = os.path.join(bundle, "disks", "disk0.img")
        os.pwrite(os.open(path, os.O_WRONLY), b"keep", 0)
        # An existing name: disk_exists, the file is neither overwritten nor removed.
        conn.expect_error(PREPARE_DISK, DISK_EXISTS, prepare_payload(b"disk0.img", 2 * MIB), aux0=0, aux1=0)
        with open(path, "rb") as handle:
            check(handle.read(4) == b"keep" and os.stat(path).st_size == MIB, "the existing image is untouched")
        # An existing entry of another kind (a symlink, even a dangling one) is also an existing name and is not followed.
        os.symlink("/nonexistent-target", os.path.join(bundle, "disks", "link.img"))
        conn.expect_error(PREPARE_DISK, DISK_EXISTS, prepare_payload(b"link.img", MIB))
        check(not os.path.exists("/nonexistent-target"), "the symlink target was not created")
        os.unlink(os.path.join(bundle, "disks", "link.img"))
        # Names: only <name>, no path components, no control characters, valid UTF-8.
        for name in (b"..", b".", b"a/b", b"disks/x.img", b"x\x01y", b"x\x7fy", b"\xff\xfe", b"a\x00b", b"/abs", b""):
            conn.expect_error(PREPARE_DISK, INVALID_NAME, prepare_payload(name, MIB), aux0=0, aux1=0)
        conn.expect_error(PREPARE_DISK, INVALID_NAME, prepare_payload(b"n" * 248 + b"/", MIB))
        # Capacity and format.
        conn.expect_error(PREPARE_DISK, INVALID_ARGUMENT, prepare_payload(b"odd.img", MIB + 1), aux0=8)
        conn.expect_error(PREPARE_DISK, INVALID_ARGUMENT, prepare_payload(b"odd.img", 511), aux0=8)
        conn.expect_error(PREPARE_DISK, INVALID_ARGUMENT, prepare_payload(b"zero.img", 0), aux0=8)
        conn.expect_error(PREPARE_DISK, INVALID_ARGUMENT, prepare_payload(b"big.img", I64_MAX + 1), aux0=8)
        conn.expect_error(PREPARE_DISK, INVALID_ARGUMENT, prepare_payload(b"big.img", (1 << 64) - 512), aux0=8)
        conn.expect_error(PREPARE_DISK, INVALID_ARGUMENT, prepare_payload(b"fmt.img", MIB, fmt=2), aux0=0)
        # Not enough space: the numbers are required and available bytes; nothing is created.
        error = conn.expect_error(PREPARE_DISK, INSUFFICIENT_SPACE, prepare_payload(b"huge.img", BIG_CAPACITY))
        check(error["aux0"] == BIG_CAPACITY, f"insufficient_space aux0 is the required bytes, got {error['aux0']}")
        check(0 < error["aux1"] < BIG_CAPACITY, f"insufficient_space aux1 is the available bytes, got {error['aux1']}")
        check(disk_entries(bundle) == ["disk0.img"], f"refusals create nothing: {disk_entries(bundle)}")
        conn.expect_state(STOPPED, 0, 0)
        # Every refusal above left the connection alive and the machine not configured.
        conn.sock.shutdown(socket.SHUT_WR)
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def scenario_running(binary: Path, bundle: str) -> None:
    host = Host(binary, bundle, display=True)
    try:
        conn = host.conn
        conn.handshake()
        conn.expect_ok(v2.CONFIGURE, configure_payload())
        events, _ = conn.expect_ok(v2.START, struct.pack("<Q", 0))
        check(events and events[0][:2] == (STOPPED, RUNNING), "START moves to running")
        conn.expect_error(PREPARE_DISK, INVALID_STATE, prepare_payload(b"late.img", MIB))
        check(disk_entries(bundle) == [], f"no image is created while running: {disk_entries(bundle)}")
        conn.expect_ok(v2.PAUSE, struct.pack("<II", v2.USER, 0))
        conn.expect_error(PREPARE_DISK, INVALID_STATE, prepare_payload(b"late.img", MIB))
        conn.expect_ok(v2.RESUME, struct.pack("<II", v2.USER, 0))
        conn.expect_ok(v2.STOP, struct.pack("<Q", 0))
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def scenario_directory_problems(binary: Path, root: str) -> None:
    # No `disks/` in the bundle: the VMHost does not create it; io_error with errno (ENOENT = 2).
    bundle = make_bundle(os.path.join(root, "nodisks"), with_disks=False)
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        conn.expect_error(PREPARE_DISK, IO_ERROR, prepare_payload(b"disk0.img", MIB), aux0=2)
        check(not os.path.exists(os.path.join(bundle, "disks")), "disks/ is not created by the VMHost")
        conn.sock.shutdown(socket.SHUT_WR)
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()
    # `disks` is a symlink to a directory: not followed (O_NOFOLLOW), nothing is created behind it.
    bundle = make_bundle(os.path.join(root, "linked"), with_disks=False)
    elsewhere = os.path.join(root, "elsewhere")
    os.mkdir(elsewhere)
    os.symlink(elsewhere, os.path.join(bundle, "disks"))
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        error = conn.expect_error(PREPARE_DISK, IO_ERROR, prepare_payload(b"disk0.img", MIB))
        check(error["aux0"] != 0, "io_error carries errno")
        check(os.listdir(elsewhere) == [], "nothing was created behind the symlink")
        conn.sock.shutdown(socket.SHUT_WR)
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def main() -> int:
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_BINARY
    check(binary.exists(), f"{binary} does not exist (cd apps && swift build)")
    scenarios = [
        ("create, replay, conflict", scenario_create_and_replay),
        ("refusals: name, capacity, space, existing", scenario_refusals),
        ("running and paused", scenario_running),
    ]
    for name, scenario in scenarios:
        root = tempfile.mkdtemp(prefix="v2_prepare_disk_")
        try:
            bundle = make_bundle(root)
            scenario(binary, bundle)
        except Failure as failure:
            print(f"FAIL {name}: {failure}", file=sys.stderr)
            return 1
        finally:
            shutil.rmtree(root, ignore_errors=True)
        print(f"ok   {name}")
    root = tempfile.mkdtemp(prefix="v2_prepare_disk_")
    try:
        for sub in ("nodisks", "linked"):
            os.mkdir(os.path.join(root, sub))
        scenario_directory_problems(binary, root)
    except Failure as failure:
        print(f"FAIL directory problems: {failure}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print("ok   directory problems")
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
