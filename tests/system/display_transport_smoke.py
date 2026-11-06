#!/usr/bin/env python3
"""S10-06/S10-14 smoke: frame transport v2 reader against the test-only VMHost.

Starts `od-vmhost --socket-fd 3 --display-smoke pattern` over a socketpair,
acts as the UI reader of protocols/control/FRAME_TRANSPORT_V1.md (HELLO, ANNOUNCE
with SCM_RIGHTS, fstat before mmap, MODE, FRAME, seqlock read), saves the slot as a
raw file and compares it with the bars 64x48 seed 0 reference at tolerance 0.
Also checks the VERSION reply to a HELLO with a foreign version (v1 and v3), the v2 memory header
(cursor offset, cursor area header, zeroed cursor descriptors and slots) and that the test pattern
profile, which never writes a cursor register, sends no CURSOR message and publishes no cursor.

Usage: python3 tests/system/display_transport_smoke.py [VMHOST_BINARY]
Default: apps/.build/debug/od-vmhost (`cargo build -p od-host --offline`, then `cd apps && swift build`).
"""

from __future__ import annotations

import fcntl
import mmap
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
DEFAULT_VMHOST = ROOT / "apps" / ".build" / "debug" / "od-vmhost"
REFERENCE = ROOT / "tests" / "fixtures" / "guest" / "stage10" / "bars_64x48_seed0.raw"
COMPARE = ROOT / "tests" / "system" / "display_compare.py"

GRANULE = 16384
VERSION = 3
SLOTS_OFFSET = 65536
MIN_TOTAL = SLOTS_OFFSET + 2 * 16384
MAX_TOTAL = SLOTS_OFFSET + 4 * 37_748_736
KIND_VERSION, KIND_HELLO, KIND_ANNOUNCE, KIND_MODE, KIND_FRAME, KIND_CURSOR = range(6)
TIMEOUT = 10.0


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def header(kind: int, payload_length: int = 0, version: int = VERSION) -> bytes:
    return b"ODFT" + struct.pack("<HHII", version, kind, 0, payload_length)


class Host:
    """VMHost child with the control socket on fd 3 (posix_spawn, like the launcher)."""

    def __init__(self, binary: Path, extra: list[str]) -> None:
        self.stderr_path = tempfile.NamedTemporaryFile(prefix="vmhost_stderr_", delete=False).name
        parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
        # A high duplicate: dup2 onto 3 in the child must not be a no-op on a CLOEXEC fd.
        child_fd = fcntl.fcntl(child.fileno(), fcntl.F_DUPFD, 10)
        try:
            self.pid = os.posix_spawn(
                str(binary),
                [str(binary), "--socket-fd", "3", "--display-smoke", "pattern", *extra],
                os.environ,
                file_actions=[
                    (os.POSIX_SPAWN_DUP2, child_fd, 3),
                    (os.POSIX_SPAWN_OPEN, 2, self.stderr_path, os.O_WRONLY | os.O_TRUNC, 0o600),
                ],
            )
        finally:
            os.close(child_fd)
            child.close()
        self.sock = parent
        self.sock.settimeout(TIMEOUT)
        self.status: int | None = None

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
                raise Failure("VMHost did not exit after the socket was closed")
            time.sleep(0.01)

    def stderr_text(self) -> str:
        return Path(self.stderr_path).read_text(errors="replace")

    def kill(self) -> None:
        if self.status is None:
            try:
                os.kill(self.pid, 9)
                os.waitpid(self.pid, 0)
            except OSError:
                pass
        try:
            os.unlink(self.stderr_path)
        except OSError:
            pass


class Reader:
    """Receives messages with recvmsg; only the first recvmsg of the stream may carry a descriptor."""

    def __init__(self, sock: socket.socket) -> None:
        self.sock = sock
        self.buffer = b""
        self.fds: list[int] = []
        self.first = True

    def _recv(self, size: int) -> bytes:
        data, ancillary, flags, _ = self.sock.recvmsg(size, socket.CMSG_SPACE(4))
        check(not flags & socket.MSG_CTRUNC, "MSG_CTRUNC")
        received: list[int] = []
        for level, kind, payload in ancillary:
            check(level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS, "unexpected ancillary data")
            received += list(struct.unpack(f"={len(payload) // 4}i", payload[: len(payload) // 4 * 4]))
        for fd in received:
            fcntl.fcntl(fd, fcntl.F_SETFD, fcntl.FD_CLOEXEC)
        if received and not (self.first and data):
            for fd in received:
                os.close(fd)
            raise Failure("descriptor received in a recvmsg other than the first")
        if data:
            self.first = False
        self.fds += received
        return data

    def take(self, size: int) -> bytes:
        while len(self.buffer) < size:
            data = self._recv(size - len(self.buffer))
            check(data != b"", "EOF before a full message")
            self.buffer += data
        out, self.buffer = self.buffer[:size], self.buffer[size:]
        return out

    def message(self) -> tuple[int, bytes]:
        head = self.take(16)
        check(head[:4] == b"ODFT", "bad message magic")
        version, kind, reserved, length = struct.unpack("<HHII", head[4:])
        check(version == VERSION and reserved == 0, "bad message header")
        return kind, self.take(length)


def read_slot_seqlock(header_map: mmap.mmap, data_map: mmap.mmap, slot: int, slot_size: int, generation: int):
    # data_map starts at file offset GRANULE; frame slots start at SLOTS_OFFSET (after the cursor image slots).
    """Section 5.2 with plain reads: claim, g1, fields once, copy, g2, release claim."""
    descriptor = 128 + 64 * slot
    header_map[64:68] = struct.pack("<I", slot + 1)
    try:
        (g1,) = struct.unpack_from("<Q", header_map, descriptor)
        check(g1 == generation, f"slot generation {g1} differs from FRAME generation {generation}")
        mode_count, width, height, stride, fmt, state = struct.unpack_from("<QIIIII", header_map, descriptor + 8)
        reserved = header_map[descriptor + 36 : descriptor + 64]
        check(state == 1 and reserved == bytes(28), "descriptor state/reserved")
        check(stride == width * 4 and stride * height <= slot_size, "descriptor geometry")
        start = SLOTS_OFFSET - GRANULE + slot * slot_size
        pixels = bytes(data_map[start : start + stride * height])
        (g2,) = struct.unpack_from("<Q", header_map, descriptor)
        check(g2 == g1, "generation changed during the read (torn frame)")
        return mode_count, width, height, stride, fmt, pixels
    finally:
        header_map[64:68] = struct.pack("<I", 0)


def scenario_frame(binary: Path, workdir: Path) -> None:
    host = Host(binary, ["--display-hold-until-claim", "1"])
    try:
        host.sock.sendall(header(KIND_HELLO))
        reader = Reader(host.sock)

        head = reader.take(16)
        check(head[:4] == b"ODFT" and struct.unpack("<HHII", head[4:]) == (VERSION, KIND_ANNOUNCE, 0, 24), "ANNOUNCE header")
        check(len(reader.fds) == 1, "ANNOUNCE did not carry exactly one descriptor")
        fd = reader.fds[0]
        total_size, slot_count, announce_screen, announce_count, announce_reserved = struct.unpack("<QIIII", reader.take(24))
        check(announce_screen == 0 and announce_count == 1 and announce_reserved == 0, "ANNOUNCE screen fields")
        check(2 <= slot_count <= 4, "ANNOUNCE payload")
        check(MIN_TOTAL <= total_size <= MAX_TOTAL and total_size % GRANULE == 0, "total_size out of range")
        # Section 7.1 step 3: fstat before mmap.
        check(os.fstat(fd).st_size == total_size, f"fstat size {os.fstat(fd).st_size} != ANNOUNCE {total_size}")

        header_map = mmap.mmap(fd, GRANULE, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE, offset=0)
        data_map = mmap.mmap(fd, total_size - GRANULE, mmap.MAP_SHARED, mmap.PROT_READ, offset=GRANULE)
        os.close(fd)
        fields = struct.unpack_from("<4sHHIIIIIIIIIIQII", header_map, 0)
        (magic, version, header_size, reader_off, desc_off, desc_size, h_slots, slots_off, slot_size,
         pixel_format, max_w, max_h, flags, h_total, cursor_off, h_reserved) = fields
        check(magic == b"ODFS" and version == VERSION and header_size == 64, "memory header identity")
        # FRAME_TRANSPORT_V1.md 11.3: the object names its screen; a single-screen run has screen 0 at offset 60.
        check(struct.unpack_from("<I", header_map, 60)[0] == 0, "memory header screen")
        check((reader_off, desc_off, desc_size, slots_off) == (64, 128, 64, SLOTS_OFFSET), "memory header offsets")
        check(h_slots == slot_count and h_total == total_size and pixel_format == 1 and flags == 0, "memory header values")
        check(slot_size % GRANULE == 0 and max_w * 4 * max_h <= slot_size, "slot_size")
        check(total_size == slots_off + slot_count * slot_size, "total_size formula")
        check((cursor_off, h_reserved) == (384, 0), "cursor_offset / header reserved")
        # Cursor area header (10.3.2): slot count, descriptor size and offset, slots offset and size, max size, format, reserved.
        check(struct.unpack_from("<8I", header_map, 384) == (3, 64, 448, GRANULE, 16384, 64, 64, 2), "cursor area header")
        check(header_map[384 + 32 : 384 + 64] == bytes(32), "cursor area header reserved")
        # The test pattern profile never writes a cursor register: three zero descriptors, zero image slots, cursor_claim 0.
        check(header_map[448 : 448 + 3 * 64] == bytes(3 * 64), "cursor descriptors must be zero")
        check(header_map[68:72] == bytes(4), "cursor_claim must start at 0")
        check(bytes(data_map[0 : 3 * 16384]) == bytes(3 * 16384), "cursor image slots must be zero")

        kind, payload = reader.message()
        check(kind == KIND_MODE and len(payload) == 32, "second message must be MODE")
        mode_count, mode_flags, mode_w, mode_h, mode_stride, mode_fmt, mode_screen = struct.unpack("<QIIIIII", payload)
        check(mode_flags == 1 and mode_screen == 0, "MODE must be ACTIVE on screen 0")
        check((mode_w, mode_h, mode_stride, mode_fmt) == (64, 48, 256, 1), "MODE geometry")

        # Generation transition 0 -> N, deterministic: the test-only writer holds publication until the
        # reader sets reader_claim, so generation 0 is observed before the gate is opened. If the writer
        # published anyway (no gate), the check fails; if it never publishes, the FRAME wait times out.
        check(select.select([host.sock], [], [], 0)[0] == [], "FRAME arrived before the publication gate was opened")
        check(header_map[448 : 448 + 3 * 64] == bytes(3 * 64), "a cursor descriptor was published without a cursor")
        for slot in range(slot_count):
            (gen,) = struct.unpack_from("<Q", header_map, 128 + 64 * slot)
            check(gen == 0, f"slot {slot} generation {gen} before any FRAME")
        time.sleep(0.1)  # let several 60 Hz checks pass: a held writer must still not publish
        for slot in range(slot_count):
            (gen,) = struct.unpack_from("<Q", header_map, 128 + 64 * slot)
            check(gen == 0, f"slot {slot} published while held")
        header_map[64:68] = struct.pack("<I", 1)  # open the gate: claim slot 0

        kind, payload = reader.message()
        check(kind == KIND_FRAME and len(payload) == 24, "third message must be FRAME")
        generation, frame_mode_count, slot, frame_screen = struct.unpack("<QQII", payload)
        check(generation > 0 and slot < slot_count and frame_screen == 0, "FRAME fields")
        # The gate was opened with reader_claim = 1 (slot 0): the writer must pick another slot, here slot 1
        # (the oldest candidate; slots 1 and 2 are both empty, the lowest index wins).
        check(slot == 1, f"FRAME slot {slot}, expected 1 (slot 0 is claimed by the reader)")
        check(frame_mode_count == mode_count, "FRAME mode_change_count differs from the last MODE")

        got_mode, width, height, stride, fmt, pixels = read_slot_seqlock(header_map, data_map, slot, slot_size, generation)
        check(got_mode == mode_count and (width, height, stride, fmt) == (64, 48, 256, 1), "descriptor does not match MODE")
        check(len(pixels) == 12288, "frame length")
        raw = workdir / "frame_64x48.raw"
        raw.write_bytes(pixels)
        result = subprocess.run(
            [sys.executable, str(COMPARE), str(REFERENCE), str(raw), "--width", "64", "--height", "48", "--tolerance", "0"],
            capture_output=True, text=True,
        )
        print(result.stdout.strip())
        check(result.returncode == 0, f"display_compare.py exit {result.returncode}: {result.stderr.strip()}")
        check("differing_pixels=0" in result.stdout, "reference mismatch")

        # The generation stays published and no further notification arrives (nothing changed on the device).
        time.sleep(0.2)
        ready, _, _ = select.select([host.sock], [], [], 0)
        check(not ready, "unexpected extra message after FRAME (a CURSOR is not expected: the profile never writes a cursor)")
        check(header_map[448 : 448 + 3 * 64] == bytes(3 * 64), "a cursor descriptor was published without a cursor")
        (again,) = struct.unpack_from("<Q", header_map, 128 + 64 * slot)
        check(again == generation, "published generation changed without a new frame")
        print(f"frame ok: slots={slot_count} slot_size={slot_size} slot={slot} generation={generation} mode_change_count={mode_count}")

        header_map.close()
        data_map.close()
        host.sock.close()  # EOF is the shutdown signal (section 6.5)
        status = host.wait()
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, f"VMHost exit status {status:#x} after EOF")
    finally:
        host.kill()


def scenario_foreign_version(binary: Path, peer_version: int) -> None:
    host = Host(binary, [])
    try:
        host.sock.sendall(header(KIND_HELLO, version=peer_version))
        reader = Reader(host.sock)
        head = reader.take(16)
        check(head == header(KIND_VERSION), f"VERSION reply expected, got {head.hex()}")
        check(not reader.fds, "VERSION must not carry a descriptor")
        check(host.sock.recv(1) == b"", "connection must close after VERSION")
        status = host.wait()
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0, f"VMHost exit status {status:#x}")
        print(f"foreign version {peer_version} ok: VERSION reply (version {VERSION}), then EOF")
    finally:
        host.kill()


def scenario_protocol_error(binary: Path, name: str, payload: bytes) -> None:
    host = Host(binary, [])
    try:
        host.sock.sendall(payload)
        status = host.wait()
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) != 0, f"{name}: protocol error must exit non-zero, status {status:#x}")
        check("frame transport protocol" in host.stderr_text(), f"{name}: stderr lacks the protocol error text: {host.stderr_text()!r}")
        print(f"{name} ok: connection closed, exit status {os.WEXITSTATUS(status)}")
    finally:
        host.kill()


def scenario_bytes_after_hello(binary: Path) -> None:
    host = Host(binary, [])
    try:
        host.sock.sendall(header(KIND_HELLO))
        reader = Reader(host.sock)
        check(reader.take(16)[4:8] == struct.pack("<HH", VERSION, KIND_ANNOUNCE), "ANNOUNCE expected")
        host.sock.sendall(b"x")
        status = host.wait()
        check(os.WIFEXITED(status) and os.WEXITSTATUS(status) != 0, f"bytes after HELLO must exit non-zero, status {status:#x}")
        check("frame transport protocol: bytes from the reader after HELLO" in host.stderr_text(),
              f"stderr lacks the protocol error text: {host.stderr_text()!r}")
        print(f"bytes after HELLO ok: exit status {os.WEXITSTATUS(status)}")
    finally:
        host.kill()


def main(argv: list[str]) -> int:
    binary = Path(argv[1]) if len(argv) > 1 else DEFAULT_VMHOST
    if not binary.exists():
        print(f"error: VMHost binary not found: {binary}", file=sys.stderr)
        return 2
    try:
        with tempfile.TemporaryDirectory(prefix="display_transport_smoke_") as tmp:
            scenario_frame(binary, Path(tmp))
        scenario_foreign_version(binary, 2)  # a v2 UI against this v3 VMHost: VERSION reply, then EOF (8.1)
        scenario_foreign_version(binary, 4)
        scenario_protocol_error(binary, "foreign magic", b"ODCP" + bytes(12))
        scenario_protocol_error(binary, "malformed HELLO", header(KIND_FRAME))
        scenario_bytes_after_hello(binary)
    except (Failure, OSError, struct.error) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print("display_transport_smoke: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
