#!/usr/bin/env python3
"""S11-04 smoke: control protocol v2 lifecycle against `od-vmhost --control-v2` (test-only profiles, no guest).

Acts as the controlling process of protocols/control/CONTROL_PROTOCOL_V2.md over a socketpair on fd 3 (and a second
socketpair on fd 5 for the frame transport in the main scenario): HELLO/READY, CONFIGURE (valid and invalid), START,
running, PAUSE (the poll counter of STATS stops), RESUME, STOP, EOF, exit status 0; request ID replay, a non-zero
operation ID, a configuration with a disk, the bundle lock (second VMHost -> busy), ATTACH_RESOURCE with descriptors
(SCM_RIGHTS) and the protocol errors that close the connection (status 4). A machine here is a display session of a test
profile: the only observable sign of "running" is the poll counter in STATS.

Usage: python3 tests/system/vmhost_control_v2_lifecycle_smoke.py [VMHOST_BINARY]
Default binary: apps/.build/debug/od-vmhost (`cd apps && swift build`; the staticlib first: `cargo build -p od-host --offline`).
"""

from __future__ import annotations

import fcntl
import os
import select
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BINARY = ROOT / "apps" / ".build" / "debug" / "od-vmhost"
TIMEOUT = 10.0

# Kinds (section 3)
HELLO, READY, CONFIGURE, ATTACH, PREPARE_DISK, START, PAUSE, RESUME, STOP, SHUTDOWN, QUERY, SET_CLIPBOARD = (
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)
SUSPEND = 40
ACK, STATE, STATE_CHANGED, PROGRESS, STATS, ERROR = 64, 65, 66, 67, 68, 255
# States and reasons
STOPPED, RUNNING, PAUSED, FAILED = 1, 2, 3, 5
USER, SYSTEM_SLEEP, SYSTEM_WAKE, HOST_FAILURE = 1, 2, 3, 5
# Error codes (section 12.1)
(INVALID_STATE, INVALID_CONFIG, NOT_CONFIGURED, UNSUPPORTED, UNSUPPORTED_DURABILITY, UNSUPPORTED_NO_AGENT, BUSY,
 LOCK_CHANGED, LOCK_INVALID) = range(1, 10)
INVALID_NAME, INVALID_RESOURCE, RESOURCE_CONFLICT = 14, 17, 18
INVALID_ARGUMENT, REQUEST_ID_CONFLICT, REQUEST_ID_STALE, HOST_MEMORY_EXCEEDED = 20, 22, 23, 12
BOOT_MEDIA, DISK, SHARED_FOLDER_ROOT, DROP_FILE = 1, 2, 3, 4
FLAG_REPLAYED, FLAG_FATAL = 1, 2


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def header(kind: int, rid: int, length: int, version: int = 2, magic: bytes = b"ODCP") -> bytes:
    return magic + struct.pack("<HHII", version, kind, rid, length)


def frame(kind: int, rid: int, payload: bytes = b"", version: int = 2) -> bytes:
    return header(kind, rid, len(payload), version) + payload


def configure_payload(profile: int = 1, memory: int = 4096, vcpu: int = 1, screens: int = 1, network: int = 0,
                      audio: int = 0, clipboard: int = 0, boot: int = 0, disks: list | None = None,
                      folders: list | None = None, reserved: int = 0) -> bytes:
    disks = disks or []
    folders = folders or []
    out = struct.pack("<IIHHBBBBBBH", profile, memory, vcpu, screens, network, audio, clipboard, boot, len(disks),
                      len(folders), reserved)
    for index in range(4):
        if index < len(disks):
            source, fmt, writable, name = disks[index]
            out += bytes([source, fmt, writable, len(name)]) + name.ljust(252, b"\0")
        else:
            out += bytes(256)
    for index in range(8):
        if index < len(folders):
            read_only, guest = folders[index]
            out += bytes([read_only, len(guest), 0, 0]) + guest.ljust(64, b"\0")
        else:
            out += bytes(68)
    check(len(out) == 1588, "CONFIGURE builder length")
    return out


def attach_payload(role: int, index: int) -> bytes:
    return struct.pack("<HHI", role, index, 0)


def parse_error(payload: bytes) -> dict:
    code, failed_kind, flags, state, detail_len, reserved, aux0, aux1 = struct.unpack("<IHHIHHQQ", payload[:32])
    check(reserved == 0 and len(payload) == 32 + detail_len, "malformed ERROR payload")
    return {"code": code, "failed_kind": failed_kind, "flags": flags, "state": state, "aux0": aux0, "aux1": aux1,
            "detail": payload[32:].decode("utf-8", "replace")}


def parse_ack(payload: bytes) -> dict:
    code, kind, flags, state = struct.unpack("<IHHI", payload)
    return {"code": code, "kind": kind, "flags": flags, "state": state}


def parse_stats(payload: bytes) -> dict:
    state, reserved, polls, frames, rss = struct.unpack("<IIQQQ", payload)
    check(reserved == 0, "STATS reserved")
    return {"state": state, "polls": polls, "frames": frames, "rss": rss}


class Host:
    """`od-vmhost --control-v2` child: control socket on fd 3, optional frame socket on fd 5 (posix_spawn)."""

    def __init__(self, binary: Path, bundle: str | None, *, display: bool = False, fd3: bool = True,
                 args: list[str] | None = None) -> None:
        self.stderr_path = tempfile.NamedTemporaryFile(prefix="vmhost_v2_stderr_", delete=False).name
        self.sock: socket.socket | None = None
        self.display: socket.socket | None = None
        actions = [(os.POSIX_SPAWN_OPEN, 2, self.stderr_path, os.O_WRONLY | os.O_TRUNC, 0o600)]
        to_close = []
        argv = [str(binary), "--control-v2"]
        if fd3:
            parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
            child_fd = fcntl.fcntl(child.fileno(), fcntl.F_DUPFD, 10)
            actions.append((os.POSIX_SPAWN_DUP2, child_fd, 3))
            to_close += [child_fd, child]
            self.sock = parent
            argv += ["--socket-fd", "3"]
        if display:
            dparent, dchild = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
            dchild_fd = fcntl.fcntl(dchild.fileno(), fcntl.F_DUPFD, 11)
            actions.append((os.POSIX_SPAWN_DUP2, dchild_fd, 5))
            to_close += [dchild_fd, dchild]
            self.display = dparent
            argv += ["--display-fd", "5"]
        if bundle is not None:
            argv += ["--bundle", bundle]
        argv += args or []
        try:
            self.pid = os.posix_spawn(str(binary), argv, os.environ, file_actions=actions)
        finally:
            for item in to_close:
                if isinstance(item, int):
                    os.close(item)
                else:
                    item.close()
        self.status: int | None = None
        self.conn = Conn(self.sock) if self.sock else None

    def wait(self, timeout: float = TIMEOUT) -> int:
        deadline = time.monotonic() + timeout
        while True:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = os.waitstatus_to_exitcode(status)
                return self.status
            if time.monotonic() > deadline:
                os.kill(self.pid, 9)
                os.waitpid(self.pid, 0)
                raise Failure("VMHost did not exit: " + self.stderr_text())
            time.sleep(0.01)

    def stderr_text(self) -> str:
        return Path(self.stderr_path).read_text(errors="replace")

    def cleanup(self) -> None:
        if self.status is None:
            try:
                os.kill(self.pid, 9)
                os.waitpid(self.pid, 0)
            except OSError:
                pass
        for sock in (self.sock, self.display):
            if sock:
                sock.close()
        try:
            os.unlink(self.stderr_path)
        except OSError:
            pass


class Conn:
    """Controlling process end of the control socket: frames out, frames in, request IDs 1, 2, 3, ..."""

    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self.buffer = b""
        self.next_id = 1
        self.stats: list[tuple[float, dict]] = []

    def send(self, data: bytes, fds: list[int] | None = None) -> None:
        if fds:
            socket.send_fds(self.sock, [data], fds)
        else:
            self.sock.sendall(data)

    def _fill(self, size: int, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while len(self.buffer) < size:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise Failure("timeout waiting for a frame")
            self.sock.settimeout(remaining)
            try:
                data = self.sock.recv(65536)
            except ConnectionResetError:
                data = b""
            if not data:
                check(not self.buffer, "EOF inside a frame")
                return False
            self.buffer += data
        return True

    def recv(self, timeout: float = TIMEOUT):
        """One frame (kind, rid, payload); None on EOF between frames."""
        if not self._fill(16, timeout):
            return None
        magic, version, kind, rid, length = struct.unpack("<4sHHII", self.buffer[:16])
        check(magic == b"ODCP" and version == 2, "bad header from the VMHost")
        check(self._fill(16 + length, timeout), "EOF inside a frame")
        payload = self.buffer[16:16 + length]
        self.buffer = self.buffer[16 + length:]
        if kind == STATS:
            self.stats.append((time.monotonic(), parse_stats(payload)))
        return kind, rid, payload

    def recv_event(self, timeout: float = TIMEOUT):
        """Next frame that is not STATS (STATS go to `self.stats`)."""
        while True:
            item = self.recv(timeout)
            if item is None or item[0] != STATS:
                return item

    def rpc(self, kind: int, payload: bytes = b"", fds: list[int] | None = None, rid: int | None = None):
        """Sends one request with the next ID; returns (events, terminal) where terminal is (kind, payload)."""
        if rid is None:
            rid = self.next_id
            self.next_id += 1
        if kind in (START, STOP, SHUTDOWN) and not payload:
            payload = struct.pack("<Q", 0)  # operation ID 0
        self.send(frame(kind, rid, payload), fds)
        events = []
        while True:
            item = self.recv_event()
            check(item is not None, f"EOF while waiting for the answer to kind {kind}")
            got_kind, got_rid, got_payload = item
            check(got_rid == rid, f"answer carries request id {got_rid}, expected {rid}")
            if got_kind == STATE_CHANGED:
                events.append(struct.unpack("<III", got_payload))
                continue
            check(got_kind in (ACK, STATE, ERROR), f"unexpected terminal kind {got_kind}")
            return events, (got_kind, got_payload)

    def handshake(self) -> None:
        self.send(frame(HELLO, 0))
        item = self.recv_event()
        check(item is not None and item[0] == READY and item[1] == 0, "READY expected")
        check(item[2] == struct.pack("<II", 2, 0), "READY payload (profile 2, reserved 0)")

    def expect_ok(self, kind: int, payload: bytes = b"", fds: list[int] | None = None):
        events, (got, body) = self.rpc(kind, payload, fds)
        check(got == ACK, f"ACK expected for kind {kind}, got {got}: {parse_error(body) if got == ERROR else ''}")
        ack = parse_ack(body)
        check(ack["code"] == 0 and ack["kind"] == kind, f"ACK ok for kind {kind}")
        return events, ack

    def expect_error(self, kind: int, code: int, payload: bytes = b"", fds: list[int] | None = None, **fields):
        events, (got, body) = self.rpc(kind, payload, fds)
        check(got == ERROR, f"ERROR {code} expected for kind {kind}, got kind {got}")
        error = parse_error(body)
        check(error["code"] == code and error["failed_kind"] == kind,
              f"kind {kind}: expected code {code}, got {error['code']} ({error['detail']})")
        check(not events, f"kind {kind}: a refusal must not emit STATE_CHANGED")
        for name, value in fields.items():
            check(error[name] == value, f"kind {kind} code {code}: {name} = {error[name]}, expected {value}")
        return error

    def expect_state(self, state: int, flags: int = 0, last_reason: int | None = None) -> None:
        events, (got, body) = self.rpc(QUERY)
        check(got == STATE and not events, "STATE expected for QUERY")
        got_state, got_flags, got_reason = struct.unpack("<III", body)
        check(got_state == state and got_flags == flags, f"STATE {(got_state, got_flags)} != {(state, flags)}")
        if last_reason is not None:
            check(got_reason == last_reason, f"last_reason {got_reason} != {last_reason}")

    def expect_closed(self, timeout: float = TIMEOUT) -> None:
        item = self.recv_event(timeout)
        check(item is None, f"the connection must be closed without a frame, got kind {item[0] if item else None}")


def expect_exit(host: Host, status: int) -> None:
    got = host.wait()
    check(got == status, f"exit status {got}, expected {status}; stderr: {host.stderr_text().strip()}")


# ---------------------------------------------------------------------------------------------------------------------
# scenarios


def scenario_lifecycle(binary: Path, bundle: str) -> None:
    host = Host(binary, bundle, display=True)
    try:
        conn = host.conn
        conn.handshake()
        # Before CONFIGURE.
        conn.expect_state(STOPPED, 0, 0)
        conn.expect_error(START, NOT_CONFIGURED)
        conn.expect_error(PAUSE, INVALID_STATE, struct.pack("<II", USER, 0))
        conn.expect_error(RESUME, INVALID_STATE, struct.pack("<II", USER, 0))
        conn.expect_error(ATTACH, NOT_CONFIGURED, attach_payload(BOOT_MEDIA, 0), fds=[os.open("/dev/null", os.O_RDONLY)])
        # Five invalid configurations and more; each leaves the machine not configured.
        conn.expect_error(CONFIGURE, INVALID_CONFIG, configure_payload(profile=2), aux0=0)
        conn.expect_error(CONFIGURE, INVALID_CONFIG, configure_payload(memory=255), aux0=4)
        conn.expect_error(CONFIGURE, INVALID_CONFIG, configure_payload(memory=65537), aux0=4)
        conn.expect_error(CONFIGURE, INVALID_CONFIG, configure_payload(vcpu=2), aux0=8)
        conn.expect_error(CONFIGURE, INVALID_CONFIG, configure_payload(profile=1, screens=2), aux0=10)
        conn.expect_error(CONFIGURE, INVALID_CONFIG, configure_payload(network=2), aux0=12)
        conn.expect_error(CONFIGURE, INVALID_NAME, configure_payload(disks=[(1, 1, 1, b"a/b")]), aux0=24)
        conn.expect_error(CONFIGURE, INVALID_NAME, configure_payload(disks=[(1, 1, 1, b"\xff\xfe")]), aux0=24)
        conn.expect_error(CONFIGURE, INVALID_CONFIG, configure_payload(folders=[(1, b"a:b")]), aux0=1048)
        conn.expect_error(CONFIGURE, INVALID_CONFIG,
                          configure_payload(disks=[(1, 1, 1, b"d"), (1, 1, 1, b"d")]), aux0=20 + 256 + 4)
        conn.expect_state(STOPPED, 0, 0)
        # A valid configuration; a second one is refused.
        _, ack = conn.expect_ok(CONFIGURE, configure_payload())
        check(ack["state"] == STOPPED, "ACK of CONFIGURE reports stopped")
        conn.expect_error(CONFIGURE, INVALID_STATE, configure_payload())
        conn.expect_state(STOPPED, 1, 0)
        # Replay: the same request ID with the same kind is a status request (the ACK again, flag replayed).
        configure_id = conn.next_id - 3  # the valid CONFIGURE, before the refused CONFIGURE and QUERY
        conn.send(frame(CONFIGURE, configure_id, configure_payload()))
        item = conn.recv_event()
        check(item[0] == ACK and item[1] == configure_id, "replayed ACK")
        ack = parse_ack(item[2])
        check(ack["flags"] & FLAG_REPLAYED and ack["code"] == 0, "replay carries the replayed flag")
        conn.expect_state(STOPPED, 1, 0)
        # Another kind with a used ID: request_id_conflict (a refusal that does not use up an ID).
        conn.send(frame(PAUSE, configure_id, struct.pack("<II", USER, 0)))
        item = conn.recv_event()
        check(item[0] == ERROR and parse_error(item[2])["code"] == REQUEST_ID_CONFLICT, "request_id_conflict")
        # A non-zero operation ID cannot be recorded before S9-06.
        conn.expect_error(START, UNSUPPORTED_DURABILITY, struct.pack("<Q", 7))
        conn.expect_error(SHUTDOWN, UNSUPPORTED_DURABILITY, struct.pack("<Q", 7))
        conn.expect_state(STOPPED, 1, 0)
        # PREPARE_DISK is S11-08: rules are checked, then `unsupported`.
        conn.expect_error(PREPARE_DISK, UNSUPPORTED, struct.pack("<HHIQ", 1, 3, 0, 1 << 20) + b"d.i")
        conn.expect_error(PREPARE_DISK, INVALID_NAME, struct.pack("<HHIQ", 1, 3, 0, 1 << 20) + b"a/b", aux0=0)
        conn.expect_error(PREPARE_DISK, INVALID_ARGUMENT, struct.pack("<HHIQ", 2, 3, 0, 1 << 20) + b"d.i", aux0=0)
        # START: STATE_CHANGED first, then ACK.
        events, ack = conn.expect_ok(START, struct.pack("<Q", 0))
        check(events == [(STOPPED, RUNNING, USER)] and ack["state"] == RUNNING, "START: stopped -> running (user), ACK")
        conn.expect_state(RUNNING, 1, USER)
        # The frame transport on the second socket: HELLO, ANNOUNCE with a descriptor, MODE, FRAME.
        check_frame_transport(host.display)
        # STATS: running, polls and frames grow, at most once per second.
        started_at = time.monotonic()
        _, first = wait_stats(conn, 0, lambda s: s["state"] == RUNNING and s["polls"] > 0)
        check(first["frames"] >= 1 and first["rss"] > 0, f"STATS of a running machine: {first}")
        # Operations that do not apply while running.
        conn.expect_error(START, INVALID_STATE, struct.pack("<Q", 0))
        conn.expect_error(CONFIGURE, INVALID_STATE, configure_payload())
        conn.expect_error(RESUME, INVALID_STATE, struct.pack("<II", USER, 0))
        conn.expect_error(PAUSE, INVALID_ARGUMENT, struct.pack("<II", SYSTEM_WAKE, 0), aux0=0)
        conn.expect_error(PREPARE_DISK, INVALID_STATE, struct.pack("<HHIQ", 1, 3, 0, 1 << 20) + b"d.i")
        conn.expect_error(ATTACH, INVALID_STATE, attach_payload(BOOT_MEDIA, 0), fds=[os.open("/dev/null", os.O_RDONLY)])
        conn.expect_error(SET_CLIPBOARD, UNSUPPORTED)
        conn.expect_error(SUSPEND, UNSUPPORTED)
        conn.expect_error(SHUTDOWN, UNSUPPORTED_DURABILITY, struct.pack("<Q", 0))
        # PAUSE.
        events, ack = conn.expect_ok(PAUSE, struct.pack("<II", USER, 0))
        check(events == [(RUNNING, PAUSED, USER)] and ack["state"] == PAUSED, "PAUSE: running -> paused (user)")
        conn.expect_state(PAUSED, 1, USER)
        conn.expect_error(PAUSE, INVALID_STATE, struct.pack("<II", USER, 0))
        drain(host.display)
        index_a, paused_a = wait_stats(conn, 0, lambda s: s["state"] == PAUSED)
        index_b, paused_b = wait_stats(conn, index_a + 1, lambda s: s["state"] == PAUSED)
        check(paused_a["polls"] == paused_b["polls"] and paused_a["frames"] == paused_b["frames"],
              f"the poll counter must not grow while paused: {paused_a} {paused_b}")
        check(paused_b["polls"] >= first["polls"], "the counter does not go back")
        check(not select.select([host.display], [], [], 0)[0], "no MODE or FRAME while paused")
        # RESUME.
        events, ack = conn.expect_ok(RESUME, struct.pack("<II", SYSTEM_WAKE, 0))
        check(events == [(PAUSED, RUNNING, SYSTEM_WAKE)] and ack["state"] == RUNNING, "RESUME: paused -> running (system_wake)")
        conn.expect_state(RUNNING, 1, SYSTEM_WAKE)
        _, resumed = wait_stats(conn, index_b + 1, lambda s: s["state"] == RUNNING and s["polls"] > paused_b["polls"])
        check(resumed["polls"] > paused_b["polls"], "the counter grows again after RESUME")
        # STATS are not more frequent than once per second (receive times are not exact, so the count is bounded).
        elapsed = conn.stats[-1][0] - started_at
        check(len(conn.stats) <= elapsed / 0.95 + 1, f"{len(conn.stats)} STATS in {elapsed:.2f} s")
        gaps = [b[0] - a[0] for a, b in zip(conn.stats, conn.stats[1:])]
        check(all(gap >= 0.9 for gap in gaps), f"two STATS closer than 1 s: {[round(g, 3) for g in gaps]}")
        # STOP: STATE_CHANGED, ACK, close, status 0.
        events, ack = conn.expect_ok(STOP, struct.pack("<Q", 0))
        check(events == [(RUNNING, STOPPED, USER)] and ack["state"] == STOPPED, "STOP: running -> stopped (user), ACK")
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def check_frame_transport(display: socket.socket) -> None:
    display.settimeout(TIMEOUT)
    display.sendall(b"ODFT" + struct.pack("<HHII", 3, 1, 0, 0))
    buffer = b""
    announced = False
    seen_frame = False
    deadline = time.monotonic() + TIMEOUT
    while not seen_frame:
        check(time.monotonic() < deadline, "no FRAME on the frame transport")
        if len(buffer) < 16:
            data, ancillary, flags, _ = display.recvmsg(4096, socket.CMSG_SPACE(4))
            check(data, "EOF on the frame transport")
            for level, kind, payload in ancillary:
                check(level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS, "ancillary data on the frame transport")
                for fd in struct.unpack(f"={len(payload) // 4}i", payload[: len(payload) // 4 * 4]):
                    os.close(fd)
                    check(not announced, "one descriptor per ANNOUNCE")
                    announced = True
            buffer += data
            continue
        magic, version, kind, _, length = struct.unpack("<4sHHII", buffer[:16])
        check(magic == b"ODFT" and version == 3, "frame transport header")
        while len(buffer) < 16 + length:
            buffer += display.recv(4096)
        buffer = buffer[16 + length:]
        if kind == 2:
            check(announced, "ANNOUNCE arrives with a descriptor")
        elif kind == 4:
            seen_frame = True
    check(announced, "ANNOUNCE with SCM_RIGHTS was received")


def drain(sock: socket.socket) -> None:
    sock.setblocking(False)
    try:
        while True:
            if not sock.recv(65536):
                break
    except BlockingIOError:
        pass
    sock.settimeout(TIMEOUT)


def wait_stats(conn: Conn, start: int, predicate, timeout: float = 6.0) -> tuple[int, dict]:
    """The first STATS at index `start` or later (arrival order) that satisfies the predicate: (index, STATS)."""
    deadline = time.monotonic() + timeout
    index = start
    while True:
        while index < len(conn.stats):
            if predicate(conn.stats[index][1]):
                return index, conn.stats[index][1]
            index += 1
        if time.monotonic() >= deadline:
            raise Failure("no matching STATS in time")
        try:
            item = conn.recv(max(0.05, deadline - time.monotonic()))
        except Failure:
            raise Failure("no matching STATS in time")
        check(item is not None, "EOF while waiting for STATS")
        check(item[0] == STATS, f"unexpected frame kind {item[0]} while waiting for STATS")


def scenario_disk_and_unsupported(binary: Path, bundle: str) -> None:
    # A machine with a disk: START and STOP answer unsupported_durability; closing the connection in `stopped` is status 0.
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        conn.expect_ok(CONFIGURE, configure_payload(disks=[(1, 1, 1, b"disk0.img")]))
        conn.expect_state(STOPPED, 3, 0)
        conn.expect_error(START, UNSUPPORTED_DURABILITY, struct.pack("<Q", 0))
        conn.expect_error(STOP, UNSUPPORTED_DURABILITY, struct.pack("<Q", 0))
        conn.expect_error(SHUTDOWN, INVALID_STATE, struct.pack("<Q", 0))
        conn.sock.shutdown(socket.SHUT_WR)
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()
    # Devices that the test profile does not have: `unsupported`, aux0 = offset of the first field.
    for name, kwargs, offset in (("network", {"network": 1}, 12), ("audio", {"audio": 1}, 13),
                                 ("clipboard_sync", {"clipboard": 1}, 14), ("boot_media", {"boot": 1}, 15),
                                 ("shared folder", {"folders": [(1, b"share")]}, 17)):
        host = Host(binary, bundle)
        try:
            conn = host.conn
            conn.handshake()
            conn.expect_ok(CONFIGURE, configure_payload(**kwargs))
            conn.expect_error(START, UNSUPPORTED, struct.pack("<Q", 0), aux0=offset)
            conn.expect_state(STOPPED, 1, 0)
            conn.send(frame(STOP, conn.next_id, struct.pack("<Q", 0)))
            conn.next_id += 1
            item = conn.recv_event()
            check(item[0] == ACK, f"{name}: STOP of a stopped machine")
            conn.expect_closed()
            expect_exit(host, 0)
        finally:
            host.cleanup()
    # Memory above half of the host memory (checked on START). Only on hosts where 65536 MiB is above that limit.
    memsize = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"]).strip())
    if memsize // 2 < 65536 * 1024 * 1024:
        host = Host(binary, bundle)
        try:
            conn = host.conn
            conn.handshake()
            conn.expect_ok(CONFIGURE, configure_payload(memory=65536))
            conn.expect_error(START, HOST_MEMORY_EXCEEDED, struct.pack("<Q", 0), aux0=65536, aux1=memsize // 2 // (1 << 20))
        finally:
            host.cleanup()
    else:
        print("note: host memory is at least 128 GiB, host_memory_exceeded is not exercised")


def scenario_request_ids(binary: Path, bundle: str) -> None:
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        for _ in range(70):
            conn.expect_state(STOPPED, 0, 0)
        # Request ID 1 left the ring of 64; ID 70 is in it (QUERY is answered by a fresh STATE).
        conn.send(frame(QUERY, 1))
        item = conn.recv_event()
        check(item[0] == ERROR and parse_error(item[2])["code"] == REQUEST_ID_STALE, "request_id_stale")
        conn.send(frame(QUERY, 70))
        item = conn.recv_event()
        check(item[0] == STATE and item[1] == 70, "replay of QUERY is a fresh STATE")
        conn.send(frame(PAUSE, 70, struct.pack("<II", USER, 0)))
        item = conn.recv_event()
        check(item[0] == ERROR and parse_error(item[2])["code"] == REQUEST_ID_CONFLICT, "request_id_conflict")
        conn.expect_state(STOPPED, 0, 0)  # the next new ID (71) is still accepted
        conn.sock.shutdown(socket.SHUT_WR)
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def scenario_eof_while_running(binary: Path, bundle: str) -> None:
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        conn.expect_ok(CONFIGURE, configure_payload(profile=4, screens=2))
        conn.expect_ok(START, struct.pack("<Q", 0))
        conn.expect_state(RUNNING, 1, USER)
        conn.sock.close()
        expect_exit(host, 5)
    finally:
        host.cleanup()


def scenario_attach(binary: Path, bundle: str) -> None:
    iso = os.path.join(bundle, "boot.iso")
    Path(iso).write_bytes(b"\0" * 2048)
    host = Host(binary, bundle)
    try:
        conn = host.conn
        conn.handshake()
        conn.expect_ok(CONFIGURE, configure_payload(boot=1, disks=[(2, 1, 0, b"")]))
        read_only = lambda: os.open(iso, os.O_RDONLY)
        read_write = lambda: os.open(iso, os.O_RDWR)
        directory = lambda: os.open(bundle, os.O_RDONLY)

        def attach(role: int, index: int, opener, expected: int | None, **fields):
            fd = opener()
            try:
                if expected is None:
                    return conn.expect_ok(ATTACH, attach_payload(role, index), fds=[fd])
                return conn.expect_error(ATTACH, expected, attach_payload(role, index), fds=[fd], **fields)
            finally:
                os.close(fd)

        attach(BOOT_MEDIA, 0, read_write, INVALID_RESOURCE)  # access mode: read only
        attach(BOOT_MEDIA, 0, directory, INVALID_RESOURCE)  # type: regular file
        attach(BOOT_MEDIA, 1, read_only, INVALID_RESOURCE)  # no such slot
        attach(9, 0, read_only, INVALID_RESOURCE)  # unknown role
        attach(SHARED_FOLDER_ROOT, 0, directory, INVALID_RESOURCE)  # no shared folder in the configuration
        attach(DISK, 1, read_only, INVALID_RESOURCE)  # slot 1 does not exist
        attach(DISK, 0, read_write, INVALID_RESOURCE)  # writable = 0: read only expected
        attach(DROP_FILE, 0, read_only, INVALID_STATE)  # drop_file only while running or paused
        attach(BOOT_MEDIA, 0, read_only, None)  # accepted
        attach(BOOT_MEDIA, 0, read_only, RESOURCE_CONFLICT)  # the slot holds a descriptor
        attach(DISK, 0, read_only, None)
        conn.expect_state(STOPPED, 3, 0)
        # The VMHost still answers; the descriptors were stored, nothing was done with them.
        conn.expect_error(START, UNSUPPORTED_DURABILITY, struct.pack("<Q", 0))
        conn.sock.shutdown(socket.SHUT_WR)
        conn.expect_closed()
        expect_exit(host, 0)
    finally:
        host.cleanup()


def scenario_fatal_protocol_errors(binary: Path, bundle: str) -> None:
    null = os.open("/dev/null", os.O_RDONLY)
    cases = []

    def case(name, first=None, after_ready=None, close_write=False, fds=None, handshake=True):
        cases.append((name, first, after_ready, close_write, fds, handshake))

    q = lambda rid, **kw: frame(QUERY, rid, **kw)
    case("first frame with version 1", first=header(HELLO, 0, 0, version=1), handshake=False)
    case("first frame with version 3", first=header(HELLO, 0, 0, version=3), handshake=False)
    case("first frame with a foreign magic", first=header(HELLO, 0, 0, magic=b"ODFT"), handshake=False)
    case("garbage instead of HELLO", first=b"\x00" * 16, handshake=False)
    case("QUERY before HELLO", first=q(1), handshake=False)
    case("version 1 after READY", after_ready=header(QUERY, 1, 0, version=1))
    case("unknown kind", after_ready=header(13, 1, 0))
    case("kind toward the controller", after_ready=header(ACK, 1, 12) + bytes(12))
    case("payload length above the maximum", after_ready=header(SET_CLIPBOARD, 1, 65521))
    case("QUERY with a payload", after_ready=header(QUERY, 1, 1) + b"\0")
    case("CONFIGURE with a wrong length", after_ready=frame(CONFIGURE, 1, bytes(100)))
    case("CONFIGURE with non-zero reserved", after_ready=frame(CONFIGURE, 1, configure_payload(reserved=1)))
    case("CONFIGURE with disk_count 5", after_ready=frame(CONFIGURE, 1, configure_payload()[:16] + bytes([5]) + configure_payload()[17:]))
    case("repeated HELLO", after_ready=frame(HELLO, 0))
    case("request ID 0", after_ready=q(0))
    case("skipped request ID", after_ready=q(2))
    case("PAUSE with non-zero reserved", after_ready=frame(PAUSE, 1, struct.pack("<II", USER, 1)))
    case("ATTACH_RESOURCE without a descriptor", after_ready=frame(ATTACH, 1, attach_payload(DROP_FILE, 0)))
    case("a descriptor with QUERY", after_ready=q(1), fds=[null])
    case("two descriptors with ATTACH_RESOURCE", after_ready=frame(ATTACH, 1, attach_payload(DROP_FILE, 0)), fds=[null, null])
    case("truncated frame, then EOF", after_ready=q(1)[:10], close_write=True)
    for name, first, after_ready, close_write, fds, handshake in cases:
        host = Host(binary, bundle)
        try:
            conn = host.conn
            if handshake:
                conn.handshake()
                conn.send(after_ready, fds)
            else:
                conn.send(first)
            if close_write:
                conn.sock.shutdown(socket.SHUT_WR)
            conn.expect_closed()
            expect_exit(host, 4)
        except Failure as failure:
            raise Failure(f"{name}: {failure}")
        finally:
            host.cleanup()
    os.close(null)


def scenario_lock(binary: Path, bundle: str) -> None:
    first = Host(binary, bundle)
    try:
        first.conn.handshake()
        lock = os.path.join(bundle, "lock")
        info = os.lstat(lock)
        check(os.path.isfile(lock) and info.st_size == 0 and info.st_mode & 0o777 == 0o600, "lock is an empty 0600 file")
        # A second VMHost on the same bundle: ERROR busy instead of READY, fatal, failed_kind 0, request id 0, status 3.
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
        # Without a valid HELLO the lock outcome is not reported: close, status 4.
        third = Host(binary, bundle)
        try:
            third.conn.send(b"\x01" * 16)
            third.conn.expect_closed()
            expect_exit(third, 4)
        finally:
            third.cleanup()
        # EOF before HELLO with a failed lock is also status 4 (the same as garbage instead of HELLO), not 0.
        fourth = Host(binary, bundle)
        try:
            fourth.conn.sock.shutdown(socket.SHUT_WR)
            fourth.conn.expect_closed()
            expect_exit(fourth, 4)
        finally:
            fourth.cleanup()
        first.conn.expect_state(STOPPED, 0, 0)
        first.conn.sock.shutdown(socket.SHUT_WR)
        first.conn.expect_closed()
        expect_exit(first, 0)
    finally:
        first.cleanup()
    # The lock is free once the first process has gone; the file stays (EEXIST path of the acquisition).
    # With the lock held, EOF before HELLO is an EOF in `stopped` without a request in flight: status 0 (section 12.3).
    early = Host(binary, bundle)
    try:
        early.conn.sock.shutdown(socket.SHUT_WR)
        early.conn.expect_closed()
        expect_exit(early, 0)
    finally:
        early.cleanup()
    again = Host(binary, bundle)
    try:
        again.conn.handshake()
        again.conn.sock.shutdown(socket.SHUT_WR)
        again.conn.expect_closed()
        expect_exit(again, 0)
    finally:
        again.cleanup()
    # An object that is not a regular file: lock_invalid.
    for kind in ("directory", "symlink", "fifo"):
        broken = tempfile.mkdtemp(prefix="v2_lockbundle_", suffix=".odvm")
        if kind == "directory":
            os.mkdir(os.path.join(broken, "lock"))
        elif kind == "fifo":
            os.mkfifo(os.path.join(broken, "lock"), 0o600)
        else:
            os.symlink("/dev/null", os.path.join(broken, "lock"))
        host = Host(binary, broken)
        try:
            host.conn.send(frame(HELLO, 0))
            item = host.conn.recv_event()
            check(item is not None and item[0] == ERROR, f"lock as {kind}: ERROR expected")
            error = parse_error(item[2])
            check(error["code"] == LOCK_INVALID and error["flags"] & FLAG_FATAL, f"lock as {kind}: {error}")
            expect_exit(host, 3)
        finally:
            host.cleanup()
            if kind == "directory":
                os.rmdir(os.path.join(broken, "lock"))
            else:
                os.unlink(os.path.join(broken, "lock"))
            os.rmdir(broken)


def scenario_usage(binary: Path, bundle: str) -> None:
    cases = [
        ("no --bundle", Host(binary, None)),
        ("relative bundle", Host(binary, "relative.odvm")),
        ("missing bundle", Host(binary, bundle + "/missing")),
        ("no control descriptor", Host(binary, bundle, fd3=False, args=["--socket-fd", "3"])),
        ("unknown option", Host(binary, bundle, args=["--no-such-option", "1"])),
        ("--display-smoke together with --control-v2", Host(binary, bundle, args=["--display-smoke", "pattern"])),
        ("bundle path of 1024 bytes", Host(binary, "/" + "a" * 1023)),
    ]
    for name, host in cases:
        try:
            expect_exit(host, 2)
            if host.sock:
                host.sock.settimeout(1.0)
                check(host.sock.recv(16) == b"", f"{name}: no frames are sent")
        except Failure as failure:
            raise Failure(f"{name}: {failure}")
        finally:
            host.cleanup()


def main() -> int:
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_BINARY
    check(binary.exists(), f"{binary} does not exist (cd apps && swift build)")
    scenarios = [
        ("lifecycle", scenario_lifecycle),
        ("disk, unsupported devices, memory", scenario_disk_and_unsupported),
        ("request ids", scenario_request_ids),
        ("EOF while running", scenario_eof_while_running),
        ("ATTACH_RESOURCE", scenario_attach),
        ("protocol errors", scenario_fatal_protocol_errors),
        ("lock", scenario_lock),
        ("usage errors", scenario_usage),
    ]
    for name, scenario in scenarios:
        # A bundle per scenario: only the VMHost's own `lock` appears in it.
        bundle = tempfile.mkdtemp(prefix="v2_smoke_", suffix=".odvm")
        try:
            scenario(binary, bundle)
        except Failure as failure:
            print(f"FAIL {name}: {failure}", file=sys.stderr)
            return 1
        finally:
            for entry in os.listdir(bundle):
                os.unlink(os.path.join(bundle, entry))
            os.rmdir(bundle)
        print(f"ok   {name}")
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
