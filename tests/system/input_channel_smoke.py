#!/usr/bin/env python3
"""S10-16 smoke: input transport v1 (protocols/control/INPUT_TRANSPORT_V1.md) against the test-only VMHost.

Starts `od-vmhost-smoke --socket-fd 3 --display-smoke pattern --input-fd 4` with two socketpairs (frame transport on fd 3,
input on fd 4), acts as the UI: ODFT HELLO on the frame socket, ODIT HELLO on the input socket, ACCEPT with
GUEST_DELIVERY = 0, FOCUS/USAGE/MOTION, STATUS counters, a full queue without a consumer (rejected_downs, queue depth
bounded, GAINED_PENDING), protocol errors that close only the input channel (frame socket stays open), the
test-only trace consumer (`--input-trace 1`), EOF on both sockets with exit status 0.

There is NO delivery to the guest: the checks stop at the VMHost queue.

Usage: python3 tests/system/input_channel_smoke.py [VMHOST_BINARY]
"""

from __future__ import annotations

import fcntl
import os
import select
import socket
import struct
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_VMHOST = ROOT / "target" / "od-vmhost-smoke"
TIMEOUT = 10.0

K_VERSION, K_HELLO, K_ACCEPT, K_FOCUS, K_USAGE, K_MOTION, K_STATUS = range(7)
PAYLOAD = {K_VERSION: 0, K_HELLO: 0, K_ACCEPT: 8, K_FOCUS: 8, K_USAGE: 8, K_MOTION: 48, K_STATUS: 32}


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


# --- ODIT messages -----------------------------------------------------------------------------------------------

def odit(kind: int, payload: bytes = b"", version: int = 1, length: int | None = None, reserved: int = 0) -> bytes:
    return b"ODIT" + struct.pack("<HHII", version, kind, reserved, len(payload) if length is None else length) + payload


def hello() -> bytes:
    return odit(K_HELLO)


def focus(gained: bool) -> bytes:
    return odit(K_FOCUS, struct.pack("<II", 1 if gained else 0, 0))


def usage(page: int, usage_id: int, down: bool) -> bytes:
    return odit(K_USAGE, struct.pack("<HHI", page, usage_id, 1 if down else 0))


def motion(dx: int = 0, dy: int = 0, wheel: int = 0, pan: int = 0, absolute: tuple[int, int, int, int, int] | None = None,
           flags: int | None = None) -> bytes:
    """`absolute` = (mode_change_count, x, y, width, height)."""
    mode, x, y, w, h = absolute if absolute else (0, 0, 0, 0, 0)
    f = (1 if absolute else 0) if flags is None else flags
    return odit(K_MOTION, struct.pack("<QiiiiIIIIII", mode, dx, dy, wheel, pan, x, y, w, h, f, 0))


class Closed(Exception):
    pass


class Host:
    """VMHost child: frame socket on fd 3, input socket on fd 4 (posix_spawn, like the viewer)."""

    def __init__(self, binary: Path, extra: list[str] | None = None) -> None:
        self.stderr_path = tempfile.NamedTemporaryFile(prefix="vmhost_input_stderr_", delete=False).name
        frame_parent, frame_child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        input_parent, input_child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        # High duplicates: dup2 onto 3 and 4 in the child must not clobber the other source.
        frame_src = fcntl.fcntl(frame_child.fileno(), fcntl.F_DUPFD_CLOEXEC, 10)
        input_src = fcntl.fcntl(input_child.fileno(), fcntl.F_DUPFD_CLOEXEC, 10)
        args = [str(binary), "--socket-fd", "3", "--display-smoke", "pattern", "--input-fd", "4", *(extra or [])]
        try:
            self.pid = os.posix_spawn(
                str(binary), args, os.environ,
                file_actions=[
                    (os.POSIX_SPAWN_DUP2, frame_src, 3),
                    (os.POSIX_SPAWN_DUP2, input_src, 4),
                    (os.POSIX_SPAWN_OPEN, 2, self.stderr_path, os.O_WRONLY | os.O_TRUNC, 0o600),
                ],
            )
        finally:
            os.close(frame_src)
            os.close(input_src)
            frame_child.close()
            input_child.close()
        self.frame = frame_parent
        self.input = input_parent
        self.frame.settimeout(TIMEOUT)
        self.input.settimeout(TIMEOUT)
        self.status: int | None = None
        self.buffer = b""
        self.mode: tuple[int, int, int] | None = None  # (mode_change_count, width, height)

    # frame socket -----------------------------------------------------------------------------------------------
    def frame_handshake(self) -> tuple[int, int, int]:
        """ODFT HELLO, then read ANNOUNCE (closing its descriptor) and MODE; returns (mode_change_count, width, height)."""
        self.frame.sendall(b"ODFT" + struct.pack("<HHII", 2, 1, 0, 0))
        buffer = b""
        deadline = time.monotonic() + TIMEOUT
        while self.mode is None:
            check(time.monotonic() < deadline, "no MODE on the frame socket")
            data, ancillary, flags, _ = self.frame.recvmsg(4096, socket.CMSG_SPACE(16))
            check(data != b"", "frame socket closed before MODE")
            for level, kind, payload in ancillary:
                for fd in struct.unpack(f"={len(payload) // 4}i", payload[: len(payload) // 4 * 4]):
                    os.close(fd)
            buffer += data
            while len(buffer) >= 16:
                kind = struct.unpack_from("<H", buffer, 6)[0]
                length = struct.unpack_from("<I", buffer, 12)[0]
                if len(buffer) < 16 + length:
                    break
                body = buffer[16:16 + length]
                buffer = buffer[16 + length:]
                if kind == 3:  # MODE
                    count, mode_flags, width, height = struct.unpack_from("<QIII", body, 0)
                    check(mode_flags == 1, "MODE not ACTIVE")
                    self.mode = (count, width, height)
        return self.mode

    def frame_alive(self, seconds: float = 0.3) -> bool:
        """True while the frame socket has no EOF and VMHost has not exited."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ready, _, _ = select.select([self.frame], [], [], 0.05)
            if ready:
                try:
                    data = self.frame.recv(4096)
                except OSError:
                    return False
                if data == b"":
                    return False
            pid, _ = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = -1
                return False
        return True

    # input socket -----------------------------------------------------------------------------------------------
    def send(self, *messages: bytes) -> None:
        self.input.sendall(b"".join(messages))

    def read_message(self, timeout: float = TIMEOUT) -> tuple[int, bytes]:
        """One ODIT message from VMHost; raises Closed on EOF."""
        deadline = time.monotonic() + timeout
        while True:
            if len(self.buffer) >= 16:
                check(self.buffer[:4] == b"ODIT", "VMHost message with a foreign magic")
                version, kind, reserved, length = struct.unpack_from("<HHII", self.buffer, 4)
                check(version == 1 and reserved == 0 and kind in PAYLOAD and PAYLOAD[kind] == length,
                      f"malformed VMHost header {self.buffer[:16].hex()}")
                if len(self.buffer) >= 16 + length:
                    body = self.buffer[16:16 + length]
                    self.buffer = self.buffer[16 + length:]
                    return kind, body
            remaining = deadline - time.monotonic()
            check(remaining > 0, "timeout waiting for an ODIT message")
            self.input.settimeout(remaining)
            try:
                data = self.input.recv(4096)
            except socket.timeout:
                raise Failure("timeout waiting for an ODIT message") from None
            if data == b"":
                raise Closed()
            self.buffer += data

    def input_handshake(self) -> int:
        self.send(hello())
        kind, body = self.read_message()
        check(kind == K_ACCEPT, f"first message is kind {kind}, not ACCEPT")
        flags, reserved = struct.unpack("<II", body)
        check(reserved == 0, "ACCEPT reserved")
        return flags

    def wait_status(self, predicate, timeout: float = 5.0) -> dict:
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            try:
                kind, body = self.read_message(max(0.05, deadline - time.monotonic()))
            except Failure:
                break
            check(kind == K_STATUS, f"unexpected kind {kind} from VMHost")
            rejected, merged, dropped, depth, reserved = struct.unpack("<QQQII", body)
            check(reserved == 0, "STATUS reserved")
            last = {"rejected_downs": rejected, "merged_motions": merged, "dropped_unfocused": dropped, "queue_depth": depth}
            if predicate(last):
                return last
        raise Failure(f"no STATUS satisfied the condition within {timeout} s (last: {last})")

    def expect_input_closed(self, timeout: float = 3.0) -> bytes:
        """Reads until EOF; returns what arrived before it (for example a VERSION reply)."""
        got = self.buffer
        self.buffer = b""
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            check(remaining > 0, "the input channel was not closed")
            self.input.settimeout(remaining)
            try:
                data = self.input.recv(4096)
            except socket.timeout:
                raise Failure("the input channel was not closed (timeout)") from None
            except ConnectionResetError:
                return got
            if data == b"":
                return got
            got += data

    # lifecycle --------------------------------------------------------------------------------------------------
    def wait(self, timeout: float = TIMEOUT) -> int:
        deadline = time.monotonic() + timeout
        while True:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = status
                return status
            if time.monotonic() > deadline:
                os.kill(self.pid, 9)
                os.waitpid(self.pid, 0)
                raise Failure("VMHost did not exit after the frame socket was closed")
            time.sleep(0.01)

    def finish(self) -> None:
        """Closes both sockets (EOF is the stop signal) and checks the exit status 0."""
        self.input.close()
        self.frame.close()
        status = self.wait()
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, f"VMHost exit status {status:#x} after EOF; stderr: {self.stderr_text()!r}")

    def stderr_text(self) -> str:
        return Path(self.stderr_path).read_text(errors="replace")

    def kill(self) -> None:
        if self.status is None:
            try:
                os.kill(self.pid, 9)
                os.waitpid(self.pid, 0)
            except OSError:
                pass
        for sock in (self.frame, self.input):
            try:
                sock.close()
            except OSError:
                pass
        try:
            os.unlink(self.stderr_path)
        except OSError:
            pass


def start(binary: Path, extra: list[str] | None = None) -> Host:
    host = Host(binary, extra)
    host.frame_handshake()
    return host


# --- scenarios ---------------------------------------------------------------------------------------------------

def scenario_basic(binary: Path) -> None:
    host = start(binary)
    try:
        mode_count, width, height = host.mode
        flags = host.input_handshake()
        check(flags == 0, f"ACCEPT flags {flags:#x}: GUEST_DELIVERY must be 0 (no input device on the board)")
        host.send(
            focus(True), usage(0x07, 0x04, True),
            motion(dx=5, dy=-3, wheel=120), motion(dx=2, pan=-120),  # the second merges into the first
            usage(0x07, 0x04, False),
            motion(absolute=(mode_count, 3, 4, width, height)),  # click position, zero deltas, ABS_VALID, current mode
        )
        status = host.wait_status(lambda s: s["merged_motions"] >= 1)
        check(status["merged_motions"] == 1 and status["rejected_downs"] == 0 and status["dropped_unfocused"] == 0, f"counters {status}")
        # FOCUS GAINED, DOWN, MOTION (merged), UP, MOTION: 5 records, no consumer.
        check(status["queue_depth"] == 5, f"queue_depth {status['queue_depth']}, expected 5")
        host.send(focus(False), focus(True), usage(0x09, 0x01, True), motion(dx=1), motion(dx=1))
        status = host.wait_status(lambda s: s["merged_motions"] >= 2)
        check(status["queue_depth"] == 5 + 4, f"queue_depth {status['queue_depth']}, expected 9 (the queue does not drain without a consumer)")
        check(host.frame_alive(), "frame socket must stay open")
        # EOF on the input socket closes only the input channel; the frame socket is still served.
        host.input.close()
        check(host.frame_alive(0.4), "EOF on the input channel must not end VMHost")
        text = host.stderr_text()
        check("input channel closed: EOF" in text, f"stderr lacks the input close: {text!r}")
        host.frame.close()
        status_code = host.wait()
        check(os.WIFEXITED(status_code) and os.WEXITSTATUS(status_code) == 0, f"exit status {status_code:#x}")
        print("basic ok: ACCEPT GUEST_DELIVERY=0, STATUS merged_motions=1 then 2, queue_depth 5 then 9, input EOF closes only the input channel, exit 0")
    finally:
        host.kill()


def scenario_stale_empty_not_merged(binary: Path) -> None:
    """A stale-and-empty MOTION in FOCUSED is neither merged nor refused: merged_motions counts only real merges."""
    host = start(binary)
    try:
        mode_count, width, height = host.mode
        host.input_handshake()
        stale = motion(absolute=(mode_count + 5, 1, 1, width, height))
        host.send(focus(True), stale, stale, motion(dx=1), stale, motion(dx=2))
        status = host.wait_status(lambda s: s["merged_motions"] >= 1)
        check(status["merged_motions"] == 1, f"merged_motions {status['merged_motions']}, expected 1 (stale empty MOTIONs are not merges)")
        check(status["queue_depth"] == 2, f"queue_depth {status['queue_depth']}, expected 2 (GAINED and one MOTION)")
        check(status["dropped_unfocused"] == 0 and status["rejected_downs"] == 0, f"counters {status}")
        host.finish()
        print("stale empty MOTION ok: not queued, not counted as merged")
    finally:
        host.kill()


def scenario_full_queue(binary: Path) -> None:
    host = start(binary)
    try:
        host.input_handshake()
        # DOWN of 100 distinct keys with no consumer: the 85th is the first refusal (free 171 < 2*84+5 = 173).
        host.send(focus(True))
        for key in range(4, 104):
            host.send(usage(0x07, key, True))
        status = host.wait_status(lambda s: s["rejected_downs"] >= 16)
        check(status["rejected_downs"] == 16, f"rejected_downs {status['rejected_downs']}, expected 16")
        check(status["queue_depth"] == 1 + 84, f"queue_depth {status['queue_depth']}, expected 85")
        # UP for all 100: 84 are queued, 16 refused ones are silent (no protocol error, the channel stays open).
        for key in range(4, 104):
            host.send(usage(0x07, key, False))
        host.send(motion(dx=1))
        # Down/up cycles far beyond the capacity: depth stays <= 256, refusals grow, UP of a refused key is silent.
        for _ in range(1000):
            host.send(usage(0x07, 4, True), usage(0x07, 4, False))
        status = host.wait_status(lambda s: s["rejected_downs"] > 16 and s["queue_depth"] >= 240)
        check(status["queue_depth"] <= 256, f"queue_depth {status['queue_depth']} exceeds 256")
        before = status["rejected_downs"]
        # Fill with focus toggles until GAINED has no room (GAINED_PENDING); then MOTION is dropped and counted.
        host.send(focus(False))
        mode_count, width, height = host.mode
        # A MOTION whose only content is a stale absolute position: dropped as empty in FOCUSED (counted nowhere), but in
        # GAINED_PENDING it is dropped and counted in dropped_unfocused like any other MOTION (7.4).
        stale = motion(absolute=(mode_count + 5, 1, 1, width, height))
        for _ in range(400):
            host.send(focus(True), stale, focus(False))
        status = host.wait_status(lambda s: s["dropped_unfocused"] >= 1)
        check(status["queue_depth"] <= 256, f"queue_depth {status['queue_depth']} exceeds 256")
        check(status["rejected_downs"] >= before, "counters must not decrease")
        # The channel is still open and consistent: a new session of events after LOST/GAINED is accepted.
        check(host.frame_alive(0.2), "frame socket stays open")
        host.finish()
        print(f"full queue ok: rejected_downs=16 at depth 85, depth <= 256 after 2000 cycles, dropped_unfocused={status['dropped_unfocused']} in GAINED_PENDING")
    finally:
        host.kill()


def scenario_protocol_error(binary: Path, name: str, setup, bad, expect_reply: bytes | None = None,
                            handshake: bool = True, descriptor: bool = False) -> None:
    host = start(binary)
    try:
        if handshake:
            host.input_handshake()
        setup(host)
        if callable(bad):
            bad = bad(host)
        if descriptor:
            fd = os.open("/dev/null", os.O_RDONLY)
            try:
                socket.send_fds(host.input, [bad], [fd])
            finally:
                os.close(fd)
        else:
            host.send(bad)
        got = host.expect_input_closed()
        if expect_reply is not None:
            check(got == expect_reply, f"{name}: reply {got.hex()} expected {expect_reply.hex()}")
        else:
            check(got == b"", f"{name}: the channel must close without a reply, got {got.hex()}")
        check(host.frame_alive(0.3), f"{name}: the frame socket must stay open")
        check("input channel closed" in host.stderr_text(), f"{name}: stderr lacks the close line: {host.stderr_text()!r}")
        host.frame.close()
        status = host.wait()
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, f"{name}: VMHost exit status {status:#x}")
        print(f"{name} ok: input channel closed, frame socket still served, exit 0")
    finally:
        host.kill()


def scenario_protocol_errors(binary: Path) -> None:
    nothing = lambda host: None
    focused = lambda host: host.send(focus(True))

    scenario_protocol_error(binary, "unknown kind", nothing, odit(9))
    scenario_protocol_error(binary, "foreign version", nothing, odit(K_HELLO, version=2), expect_reply=odit(K_VERSION),
                            handshake=False)
    scenario_protocol_error(binary, "foreign magic", nothing, b"ODFT" + bytes(12), handshake=False)
    scenario_protocol_error(binary, "first message is not HELLO", nothing, focus(True), handshake=False)
    # Section 8.2 item 1: before HELLO the header alone decides; no payload follows and the channel still closes at once.
    scenario_protocol_error(binary, "FOCUS header without HELLO (no payload)", nothing, focus(True)[:16], handshake=False)
    scenario_protocol_error(binary, "USAGE header without HELLO (no payload)", nothing, usage(0x07, 4, True)[:16], handshake=False)
    scenario_protocol_error(binary, "wrong payload length", nothing, odit(K_FOCUS, b"\x01" * 4, length=4))
    scenario_protocol_error(binary, "reserved header field", nothing, odit(K_FOCUS, struct.pack("<II", 1, 0), reserved=1))
    scenario_protocol_error(binary, "ACCEPT sent to VMHost", nothing, odit(K_ACCEPT, bytes(8)))
    scenario_protocol_error(binary, "second HELLO", nothing, hello())
    scenario_protocol_error(binary, "USAGE before FOCUS", nothing, usage(0x07, 4, True))
    scenario_protocol_error(binary, "MOTION before FOCUS", nothing, motion(dx=1))
    scenario_protocol_error(binary, "repeated FOCUS GAINED", focused, focus(True))
    scenario_protocol_error(binary, "FOCUS LOST while unfocused", nothing, focus(False))
    scenario_protocol_error(binary, "repeated DOWN", lambda h: h.send(focus(True), usage(0x07, 4, True)), usage(0x07, 4, True))
    scenario_protocol_error(binary, "UP of a key that is not pressed", focused, usage(0x07, 4, False))
    scenario_protocol_error(binary, "reserved usage 0xA5", focused, usage(0x07, 0xA5, True))
    scenario_protocol_error(binary, "usage page 0x0C", focused, usage(0x0C, 0xE9, True))
    scenario_protocol_error(binary, "button 6", focused, usage(0x09, 6, True))
    scenario_protocol_error(binary, "empty MOTION", focused, motion())
    scenario_protocol_error(binary, "abs_x >= abs_width", focused, motion(absolute=(1, 64, 0, 64, 48)))
    scenario_protocol_error(binary, "abs size beyond 4096x2304", focused, motion(absolute=(1, 0, 0, 4097, 48)))
    scenario_protocol_error(binary, "abs fields without ABS_VALID", focused, motion(dx=1, flags=0)[:16] + struct.pack(
        "<QiiiiIIIIII", 0, 1, 0, 0, 0, 5, 0, 0, 0, 0, 0))
    scenario_protocol_error(binary, "MOTION undefined flag bit", focused, motion(dx=1, flags=2))
    scenario_protocol_error(binary, "descriptor in a message", nothing, focus(True), descriptor=True)
    # The absolute size must match the mode when the mode count is current (8.2 item 4).
    scenario_protocol_error(binary, "current mode count with another size", focused,
                            lambda host: motion(absolute=(host.mode[0], 0, 0, host.mode[1] * 2, host.mode[2])))


def scenario_trace(binary: Path) -> None:
    host = start(binary, ["--input-trace", "1"])
    try:
        mode_count, width, height = host.mode
        check(host.input_handshake() == 0, "GUEST_DELIVERY must be 0 with the trace consumer too")
        def step(*messages: bytes) -> None:
            host.send(*messages)
            time.sleep(0.06)  # one message per service step: no merging in the queue
        step(focus(True))
        step(usage(0x07, 0x04, True))
        step(motion(absolute=(mode_count, 3, 4, width, height)))  # click position
        step(usage(0x09, 0x01, True))
        step(motion(dx=2, absolute=(mode_count + 5, 7, 8, width, height)))  # stale count: absolute part reset, delta kept
        step(motion(absolute=(mode_count + 5, 9, 9, width, height)))  # stale and empty: dropped
        step(motion(dx=-1, dy=1, wheel=120, pan=-120))
        step(usage(0x07, 0x04, False))
        step(focus(False))
        deadline = time.monotonic() + 3
        lines: list[str] = []
        while time.monotonic() < deadline:
            lines = [line.split("input-trace ", 1)[1] for line in host.stderr_text().splitlines() if "input-trace " in line]
            if lines and lines[-1] == "FOCUS LOST":
                break
            time.sleep(0.05)
        expected = [
            "FOCUS GAINED",
            "USAGE page=7 id=4 state=DOWN",
            f"MOTION dx=0 dy=0 wheel=0 pan=0 abs=3,4 size={width}x{height} mode={mode_count}",
            "USAGE page=9 id=1 state=DOWN",
            "MOTION dx=2 dy=0 wheel=0 pan=0 abs=none",
            "MOTION dx=-1 dy=1 wheel=120 pan=-120 abs=none",
            "USAGE page=7 id=4 state=UP",
            "FOCUS LOST",
        ]
        check(lines == expected, f"trace {lines} != {expected}")
        # The page-9 button DOWN was never released: FOCUS LOST was the release (no UP in the trace for it).
        check("USAGE page=9 id=1 state=UP" not in lines, "FOCUS LOST must not produce an UP")
        host.finish()
        print("trace ok: records reach the test-only consumer with the same usages and coordinates; a stale absolute part is reset or dropped")
    finally:
        host.kill()


def scenario_close_queues_focus_lost(binary: Path) -> None:
    """EOF while FOCUSED queues FOCUS LOST (8.3): visible through the trace consumer."""
    host = start(binary, ["--input-trace", "1"])
    try:
        host.input_handshake()
        host.send(focus(True), usage(0x07, 0x04, True))
        time.sleep(0.2)
        host.input.close()
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and "input-trace FOCUS LOST" not in host.stderr_text():
            time.sleep(0.05)
        text = host.stderr_text()
        check("input-trace FOCUS LOST" in text, f"EOF while FOCUSED must queue FOCUS LOST: {text!r}")
        check("USAGE page=7 id=4 state=UP" not in text, "no UP for the key held at EOF")
        host.frame.close()
        status = host.wait()
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, f"exit status {status:#x}")
        print("EOF while focused ok: FOCUS LOST queued, no extra UP")
    finally:
        host.kill()


def scenario_without_input_fd(binary: Path) -> None:
    """Without --input-fd there is no input channel and the display smoke is unchanged."""
    frame_parent, frame_child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    src = fcntl.fcntl(frame_child.fileno(), fcntl.F_DUPFD_CLOEXEC, 10)
    pid = os.posix_spawn(str(binary), [str(binary), "--socket-fd", "3", "--display-smoke", "pattern"], os.environ,
                         file_actions=[(os.POSIX_SPAWN_DUP2, src, 3)])
    os.close(src)
    frame_child.close()
    try:
        frame_parent.sendall(b"ODFT" + struct.pack("<HHII", 2, 1, 0, 0))
        frame_parent.settimeout(TIMEOUT)
        data, _, _, _ = frame_parent.recvmsg(4096, socket.CMSG_SPACE(16))
        check(data[:4] == b"ODFT", "ANNOUNCE expected")
        time.sleep(0.3)  # let MODE and FRAME go out: closing earlier would make the writer fail with EPIPE
        frame_parent.close()
        _, status = os.waitpid(pid, 0)
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, f"exit status {status:#x}")
        print("without --input-fd ok: display smoke unchanged, exit 0")
    finally:
        try:
            os.kill(pid, 9)
        except OSError:
            pass


def main(argv: list[str]) -> int:
    binary = Path(argv[1]) if len(argv) > 1 else DEFAULT_VMHOST
    if not binary.exists():
        print(f"error: VMHost binary not found: {binary}", file=sys.stderr)
        return 2
    try:
        scenario_basic(binary)
        scenario_stale_empty_not_merged(binary)
        scenario_full_queue(binary)
        scenario_protocol_errors(binary)
        scenario_trace(binary)
        scenario_close_queues_focus_lost(binary)
        scenario_without_input_fd(binary)
    except (Failure, Closed, OSError, struct.error) as error:
        print(f"FAIL: {error!r}", file=sys.stderr)
        return 1
    print("input_channel_smoke: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
