#!/usr/bin/env python3
"""S10-24 failures of the frame transport and of the VMHost process: observable, documented behaviour.

Each scenario makes one failure happen and checks (a) what the memory and the socket look like and (b) what the UI reader
shows. The UI state is read from the real viewer (`open-desktop-display-viewer --watch-ms`, which prints one `viewer-state:` line per
change of the reader state of screen 0 and a `viewer-end:` line), or from a Python reader that follows the state table of
protocols/control/FRAME_TRANSPORT_V1.md 7.2 (the real reader is covered by apps/vmhost/Tests/DisplayFailureTests.swift). The
failures are driven in a real od-vmhost through the test-only flag `--display-test-fault` (apps/vmhost/Sources/DisplayFaultHook.swift;
compiled in a debug build only, a release build refuses the flag) or by a fake VMHost (this file, `--fake-vmhost`) where the
failure is a foreign version. Pass/fail criteria and the UI text per scenario: docs/testing/DISPLAY.md section 6.

  1a  VMHost killed (SIGKILL) inside the write of the FIRST slot   UI: DISCONNECTED, never SHOWING
  1b  VMHost killed inside the write of the SECOND frame           memory: slot generation 0, half pixels; EOF; Python reader
  1c  same, viewed by the viewer                                    UI: SHOWING, then DISCONNECTED
  2a  reader sends HELLO 1.5 s late                                  VMHost waits, then serves the frame
  2b  reader reads its notifications late (3000 frames behind)      stale slots discarded, latest frame consistent
  3   mode change between two frames                                 MODE before FRAME, replaced-mode frame refused; UI: MODE_CHANGING
  4   incompatible version (VERSION, message v2, memory v2, format) UI: INCOMPATIBLE, no frame, no slot read
  5a  notification queue overflow, reader silent                     counters, latest state, VMHost alive, exit 0
  5b  the same flood read by the viewer                              UI: SHOWING at the last generation
  6   second ftruncate of the shared object (FRAME_TRANSPORT_V1.md 8.3)  EINVAL, no SIGBUS

Usage: python3 tests/system/display_failure_smoke.py [--repeat N] [--only NAME[,NAME]] [VIEWER_BINARY [VMHOST_BINARY]]
Defaults: apps/.build/debug/open-desktop-display-viewer and apps/.build/debug/od-vmhost (a DEBUG build:
`cargo build -p od-host --offline`, then `cd apps && swift build`).
Exit status: 0 = every scenario passed; 1 = a check failed; 3 = BLOCKED, the viewer reports no Metal device: the scenarios that
read the UI state through the viewer were NOT verified (3 is never a pass); 2 = usage.
"""

from __future__ import annotations

import argparse
import mmap
import os
import re
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import display_transport_smoke as dts  # noqa: E402  (Host, Reader, header and the layout constants of frame transport v3)
from display_transport_smoke import Failure, check  # noqa: E402

ROOT = dts.ROOT
DEFAULT_VIEWER = ROOT / "apps" / ".build" / "debug" / "open-desktop-display-viewer"
DEFAULT_VMHOST = dts.DEFAULT_VMHOST
REFERENCE = dts.REFERENCE
GRANULE, SLOTS_OFFSET = dts.GRANULE, dts.SLOTS_OFFSET
KIND_VERSION, KIND_HELLO, KIND_ANNOUNCE, KIND_MODE, KIND_FRAME = dts.KIND_VERSION, dts.KIND_HELLO, dts.KIND_ANNOUNCE, dts.KIND_MODE, dts.KIND_FRAME
EXIT_BLOCKED = 3
NO_METAL = "no Metal device"
SIGKILL_STATUS = 128 + signal.SIGKILL  # the viewer reports a signalled child as 128 + signal


class Blocked(Exception):
    pass


# --------------------------------------------------------------------------------------------------------------------
# Python reader: messages, slots (5.2) and the UI state table (7.2)
# --------------------------------------------------------------------------------------------------------------------

EOF = "EOF"


class UiReader(dts.Reader):
    """dts.Reader with a message call that tells a timeout (None) from the end of the stream (EOF)."""

    def try_message(self, timeout: float):
        self.sock.settimeout(timeout)
        try:
            while len(self.buffer) < 16:
                self._fill()
            magic, version, kind, reserved, length = struct.unpack("<4sHHII", self.buffer[:16])
            check(magic == b"ODFT" and version == dts.VERSION and reserved == 0, "bad message header")
            while len(self.buffer) < 16 + length:
                self._fill()
        except socket.timeout:
            return None
        except (EOFError, ConnectionResetError):
            check(not self.buffer, "EOF in the middle of a message")
            return EOF
        payload = self.buffer[16 : 16 + length]
        self.buffer = self.buffer[16 + length :]
        return kind, payload

    def _fill(self) -> None:
        data = self._recv(65536)
        if data == b"":
            raise EOFError
        self.buffer += data


def parse_mode(payload: bytes) -> dict:
    count, flags, width, height, stride, fmt, screen = struct.unpack("<QIIIIII", payload)
    return {"count": count, "active": bool(flags & 1), "width": width, "height": height, "screen": screen}


def parse_frame(payload: bytes) -> dict:
    generation, count, slot, screen = struct.unpack("<QQII", payload)
    return {"generation": generation, "count": count, "slot": slot, "screen": screen}


def tag_pixels(generation: int, size: int) -> bytes:
    """The pixel of the synthetic frames of the fault hook: [g, g >> 8, g >> 16, 0]."""
    return bytes([generation & 255, (generation >> 8) & 255, (generation >> 16) & 255, 0]) * (size // 4)


class UiModel:
    """The reader states of 7.2 that these scenarios need, with shown_once."""

    LABEL = {
        "NO_SIGNAL": "Нет сигнала",
        "MODE_CHANGING": "Смена режима экрана",
        "DISCONNECTED": "Нет сигнала: VMHost отключился",
        "INCOMPATIBLE": "Несовместимая версия",
    }

    def __init__(self) -> None:
        self.history = ["CONNECTING"]
        self.shown_once = False

    @property
    def state(self) -> str:
        return self.history[-1]

    def _set(self, state: str) -> None:
        if state != self.history[-1]:
            self.history.append(state)

    def announced(self) -> None:
        self._set("NO_SIGNAL")

    def mode(self, active: bool) -> None:
        self._set("MODE_CHANGING" if active and self.shown_once else "NO_SIGNAL")

    def frame_shown(self) -> None:
        self.shown_once = True
        self._set("SHOWING")

    def eof(self) -> None:
        self._set("DISCONNECTED")


class Link:
    """HELLO, ANNOUNCE with its descriptor, both mappings, then the notifications (7.1)."""

    def __init__(self, host: dts.Host) -> None:
        self.host = host
        host.sock.sendall(dts.header(KIND_HELLO))
        self.reader = UiReader(host.sock)
        head = self.reader.take(16)
        check(head[:4] == b"ODFT" and struct.unpack("<HHII", head[4:]) == (dts.VERSION, KIND_ANNOUNCE, 0, 24), "ANNOUNCE header")
        check(len(self.reader.fds) == 1, "ANNOUNCE did not carry exactly one descriptor")
        self.fd = self.reader.fds[0]
        self.total_size, slot_count, screen, count, reserved = struct.unpack("<QIIII", self.reader.take(24))
        check(screen == 0 and count == 1 and reserved == 0 and 2 <= slot_count <= 4, "ANNOUNCE payload")
        check(os.fstat(self.fd).st_size == self.total_size, "fstat size differs from ANNOUNCE")
        self.header_map = mmap.mmap(self.fd, GRANULE, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE, offset=0)
        self.data_map = mmap.mmap(self.fd, self.total_size - GRANULE, mmap.MAP_SHARED, mmap.PROT_READ, offset=GRANULE)
        self.slot_count = slot_count
        self.slot_size = struct.unpack_from("<I", self.header_map, 28)[0]
        self.ui = UiModel()
        self.ui.announced()

    def generation(self, slot: int) -> int:
        return struct.unpack_from("<Q", self.header_map, 128 + 64 * slot)[0]

    def slot_pixels(self, slot: int, length: int) -> bytes:
        start = SLOTS_OFFSET - GRANULE + slot * self.slot_size
        return bytes(self.data_map[start : start + length])

    def read_slot(self, slot: int, generation: int, mode_count: int):
        """Section 5.2 with plain reads. ("ok", width, height, pixels) or ("discard", reason)."""
        descriptor = 128 + 64 * slot
        self.header_map[64:68] = struct.pack("<I", slot + 1)
        try:
            (g1,) = struct.unpack_from("<Q", self.header_map, descriptor)
            if g1 != generation:
                return ("discard", f"g1 {g1} != announced {generation}")
            mcc, width, height, stride, fmt, state = struct.unpack_from("<QIIIII", self.header_map, descriptor + 8)
            check(state == 1 and mcc == mode_count and stride == width * 4 and stride * height <= self.slot_size,
                  "descriptor of a stable generation violates the protocol")
            pixels = self.slot_pixels(slot, stride * height)
            (g2,) = struct.unpack_from("<Q", self.header_map, descriptor)
            if g2 != g1:
                return ("discard", f"g2 {g2} != g1 {g1}")
            return ("ok", width, height, pixels)
        finally:
            self.header_map[64:68] = struct.pack("<I", 0)

    def close(self) -> None:
        for mapped in (self.header_map, self.data_map):
            try:
                mapped.close()
            except (BufferError, ValueError):
                pass
        try:
            os.close(self.fd)
        except OSError:
            pass


def alive(host: dts.Host) -> bool:
    """True while the VMHost process has not ended; an ended process is reaped and its raw status kept."""
    if host.status is not None:
        return False
    pid, status = os.waitpid(host.pid, os.WNOHANG)
    if pid:
        host.status = status
        return False
    return True


def wait_text(read, text: str, timeout: float = 15.0) -> None:
    deadline = time.monotonic() + timeout
    while text not in read():
        check(time.monotonic() < deadline, f"'{text}' did not appear in the log within {timeout} s")
        time.sleep(0.01)


def exit_code(host: dts.Host) -> int:
    check(host.status is not None or not alive(host), "VMHost still runs")
    return os.waitstatus_to_exitcode(host.status)


def messages_until_quiet(link: Link, quiet: float = 0.4, limit: float = 30.0) -> tuple[list, bool]:
    """Every notification up to the first silence of `quiet` seconds; True when the stream ended."""
    out = []
    deadline = time.monotonic() + limit
    while time.monotonic() < deadline:
        message = link.reader.try_message(quiet)
        if message is None:
            return out, False
        if message == EOF:
            return out, True
        out.append((time.monotonic(), message))
    raise Failure("the stream never went quiet")


# --------------------------------------------------------------------------------------------------------------------
# The viewer as the UI
# --------------------------------------------------------------------------------------------------------------------


@dataclass
class ViewerRun:
    states: list = field(default_factory=list)  # (name, text after "viewer-state: ")
    end: dict = field(default_factory=dict)
    stdout: str = ""
    stderr: str = ""
    returncode: int = 0
    killed_at: float = 0.0
    ended_at: float = 0.0

    @property
    def names(self) -> list[str]:
        return [name for name, _ in self.states]


class Context:
    def __init__(self, viewer: Path, vmhost: Path, workdir: Path) -> None:
        self.viewer, self.vmhost, self.workdir = viewer, vmhost, workdir
        self.counter = 0

    def wrapper(self, script_tail: str) -> tuple[Path, Path]:
        """A one-line shell wrapper the viewer starts as its VMHost: it records its pid, then becomes the real thing."""
        self.counter += 1
        pidfile = self.workdir / f"pid{self.counter}"
        path = self.workdir / f"vmhost{self.counter}.sh"
        path.write_text(f"#!/bin/sh\necho $$ > '{pidfile}'\nexec {script_tail}\n")
        path.chmod(0o755)
        return path, pidfile

    def watch(self, vmhost_tail: str, watch_ms: int, kill_after: str | None = None) -> ViewerRun:
        """Runs `viewer --watch-ms` with the wrapper; with `kill_after` the VMHost gets SIGKILL when that text is in the log."""
        wrapper, pidfile = self.wrapper(vmhost_tail)
        out_path, err_path = self.workdir / f"viewer_out{self.counter}", self.workdir / f"viewer_err{self.counter}"
        with open(out_path, "w") as out, open(err_path, "w") as err:
            process = subprocess.Popen(
                [str(self.viewer), "--vmhost", str(wrapper), "--watch-ms", str(watch_ms)],
                stdout=out, stderr=err, start_new_session=True,
            )
            run = ViewerRun()
            try:
                if kill_after is not None:
                    wait_text(lambda: err_path.read_text(errors="replace"), kill_after, 15)
                    pid = int(pidfile.read_text())
                    os.kill(pid, signal.SIGKILL)
                    run.killed_at = time.monotonic()
                process.wait(timeout=watch_ms / 1000 + 20)
                run.ended_at = time.monotonic()
            finally:
                try:
                    os.killpg(process.pid, signal.SIGKILL)  # whatever the viewer left (a VMHost) must not outlive it
                except ProcessLookupError:
                    pass
        run.returncode = process.returncode
        run.stdout, run.stderr = out_path.read_text(errors="replace"), err_path.read_text(errors="replace")
        if run.returncode != 0 and NO_METAL in run.stderr:
            raise Blocked(run.stderr.strip())
        check(run.returncode == 0, f"viewer exit {run.returncode}: {run.stderr.strip()}")
        for line in run.stdout.splitlines():
            if line.startswith("viewer-state: "):
                text = line[len("viewer-state: ") :]
                run.states.append((text.split(":")[0].split(" ")[0], text))
            elif line.startswith("viewer-end: "):
                run.end = dict(item.split("=", 1) for item in line[len("viewer-end: ") :].split())
        check(run.end, f"no viewer-end line: {run.stdout.strip()} / {run.stderr.strip()}")
        return run

    def vmhost_tail(self, fault: str | None) -> str:
        return f"'{self.vmhost}' \"$@\"" + (f" --display-test-fault {fault}" if fault else "")

    def host(self, fault: str | None) -> dts.Host:
        return dts.Host(self.vmhost, ["--display-test-fault", fault] if fault else [])


# --------------------------------------------------------------------------------------------------------------------
# Scenario 1: VMHost killed during the write of a slot
# --------------------------------------------------------------------------------------------------------------------


def s1a_kill_inside_the_first_write(ctx: Context) -> None:
    run = ctx.watch(ctx.vmhost_tail("mid-write:1"), watch_ms=15000, kill_after="fault-hook mid-write")
    check("SHOWING" not in run.names, f"a frame was shown although the only slot was half written: {run.names}")
    check(run.names[-1] == "DISCONNECTED", f"final UI state {run.names[-1]}, expected DISCONNECTED: {run.states}")
    check(run.end["state"] == "DISCONNECTED" and run.end["accepted"] == "0", f"viewer-end {run.end}")
    check(int(run.end["vmhost_status"]) == SIGKILL_STATUS, f"VMHost status {run.end['vmhost_status']}, expected {SIGKILL_STATUS} (SIGKILL)")
    check(run.ended_at - run.killed_at < 5.0, f"the UI needed {run.ended_at - run.killed_at:.1f} s after the kill to notice")


def s1b_kill_inside_the_second_write(ctx: Context) -> None:
    host = ctx.host("mid-write:2")
    link = None
    try:
        link = Link(host)
        modes, frames = [], []
        while not frames:
            message = link.reader.try_message(5.0)
            check(message not in (None, EOF), "no first FRAME")
            kind, payload = message
            if kind == KIND_MODE:
                modes.append(parse_mode(payload))
                link.ui.mode(modes[-1]["active"])
            elif kind == KIND_FRAME:
                frames.append(parse_frame(payload))
        first = frames[0]
        check(first["generation"] == 1 and first["count"] == modes[-1]["count"], f"first FRAME {first}")
        outcome = link.read_slot(first["slot"], first["generation"], first["count"])
        check(outcome[0] == "ok", f"the first frame was not readable: {outcome}")
        check(outcome[3] == REFERENCE.read_bytes(), "the first frame differs from the reference")
        link.ui.frame_shown()
        # The second publication stops inside the pixel copy.
        wait_text(host.stderr_text, "fault-hook mid-write")
        gens = [link.generation(slot) for slot in range(link.slot_count)]
        torn = [slot for slot in range(link.slot_count) if gens[slot] == 0 and link.slot_pixels(slot, 1) == b"\xa5"]
        check(len(torn) == 1 and gens.count(1) == 1, f"slot generations {gens}: expected one slot of generation 1 and one half-written slot of generation 0")
        torn_slot = torn[0]
        half = len(REFERENCE.read_bytes()) // 2
        check(link.slot_pixels(torn_slot, half) == b"\xa5" * half, "the first half of the torn slot is not the half-written content")
        check(link.slot_pixels(torn_slot, 2 * half)[half:] == bytes(half), "the second half of the torn slot is not untouched")
        # A reader that was (wrongly) told about this slot must refuse it: generation 0 is never a frame.
        refused = link.read_slot(torn_slot, 2, first["count"])
        check(refused[0] == "discard" and "g1 0" in refused[1], f"the torn slot was not refused: {refused}")
        check(not [m for _, m in messages_until_quiet(link, 0.3)[0] if m[0] == KIND_FRAME], "a FRAME was announced for the unfinished frame")
        os.kill(host.pid, signal.SIGKILL)
        _, ended = messages_until_quiet(link, 2.0)
        check(ended, "no EOF after SIGKILL")
        link.ui.eof()
        pid, raw = os.waitpid(host.pid, 0)
        host.status = raw
        check(os.WIFSIGNALED(raw) and os.WTERMSIG(raw) == signal.SIGKILL, f"status {raw}")
        check(link.ui.history == ["CONNECTING", "NO_SIGNAL", "SHOWING", "DISCONNECTED"], f"UI states {link.ui.history}")
        # The first frame is intact in memory, but the UI does not show it any more: the state is DISCONNECTED.
        check(link.read_slot(first["slot"], first["generation"], first["count"])[0] == "ok", "the first frame was damaged by the kill")
    finally:
        if link:
            link.close()
        host.kill()


def s1c_kill_after_a_shown_frame_in_the_viewer(ctx: Context) -> None:
    run = ctx.watch(ctx.vmhost_tail("mid-write:2"), watch_ms=15000, kill_after="fault-hook mid-write")
    names = run.names
    check("SHOWING" in names and names[-1] == "DISCONNECTED" and names.index("SHOWING") < names.index("DISCONNECTED"),
          f"expected SHOWING then DISCONNECTED, got {names}")
    check(run.states[names.index("SHOWING")][1] == "SHOWING width=64 height=48 mode_change_count=1", f"shown frame {run.states}")
    check(run.end["state"] == "DISCONNECTED" and run.end["generation"] == "1" and run.end["accepted"] == "1", f"viewer-end {run.end}")
    check(int(run.end["vmhost_status"]) == SIGKILL_STATUS, f"VMHost status {run.end['vmhost_status']}")


# --------------------------------------------------------------------------------------------------------------------
# Scenario 2: the reader is late
# --------------------------------------------------------------------------------------------------------------------


def s2a_hello_after_a_delay(ctx: Context) -> None:
    host = ctx.host(None)
    link = None
    try:
        time.sleep(1.5)
        check(alive(host), "VMHost ended while no reader had said HELLO")
        link = Link(host)
        modes, frames = [], []
        while not frames:
            message = link.reader.try_message(5.0)
            check(message not in (None, EOF), "no FRAME after a late HELLO")
            kind, payload = message
            if kind == KIND_MODE:
                modes.append(parse_mode(payload))
            elif kind == KIND_FRAME:
                frames.append(parse_frame(payload))
        outcome = link.read_slot(frames[0]["slot"], frames[0]["generation"], frames[0]["count"])
        check(outcome[0] == "ok" and outcome[3] == REFERENCE.read_bytes(), f"frame after a late HELLO: {outcome[0]}")
    finally:
        if link:
            link.close()
        host.kill()


def s2b_notifications_read_late(ctx: Context) -> None:
    host = ctx.host("flood:3000")
    link = None
    try:
        link = Link(host)  # ANNOUNCE and the mappings, then no notification is read while the writer publishes
        wait_text(host.stderr_text, "flood: 3000 frames published")
        check(alive(host), "VMHost ended while its reader was not reading")
        match = re.search(r"flood: 3000 frames published, last generation (\d+), mode_change_count (\d+)", host.stderr_text())
        last_generation, last_count = int(match.group(1)), int(match.group(2))
        messages, ended = messages_until_quiet(link)
        check(not ended, "VMHost closed the connection")
        modes = [parse_mode(p) for _, (k, p) in messages if k == KIND_MODE]
        frames = [parse_frame(p) for _, (k, p) in messages if k == KIND_FRAME]
        check(modes and frames, "no notifications")
        check([m["count"] for m in modes] == sorted({m["count"] for m in modes}), "MODE counts do not strictly grow")
        check([f["generation"] for f in frames] == sorted({f["generation"] for f in frames}), "FRAME generations do not strictly grow")
        check(modes[-1]["count"] == last_count and frames[-1]["generation"] == last_generation,
              f"the latest state was lost: last MODE {modes[-1]['count']} / FRAME {frames[-1]['generation']}, writer {last_count} / {last_generation}")
        # Replay the stream as the reader would: it takes the first FRAME of the first generation later than it was announced.
        current_mode = None
        accepted, discarded = [], 0
        for _, (kind, payload) in messages:
            if kind == KIND_MODE:
                current_mode = parse_mode(payload)
                link.ui.mode(current_mode["active"])
            else:
                frame = parse_frame(payload)
                check(frame["count"] == current_mode["count"], "FRAME of a mode that is not the last MODE")
                outcome = link.read_slot(frame["slot"], frame["generation"], frame["count"])
                if outcome[0] == "ok":
                    _, width, height, pixels = outcome
                    check((width, height) == (current_mode["width"], current_mode["height"]), "frame size differs from the MODE")
                    check(pixels == tag_pixels(frame["generation"], len(pixels)) or frame["generation"] == 1,
                          f"a frame of generation {frame['generation']} mixes pixels of two generations")
                    accepted.append(frame["generation"])
                    link.ui.frame_shown()
                else:
                    discarded += 1
        check(accepted and accepted[-1] == last_generation, f"the latest frame was not accepted: {accepted[-3:]}")
        check(discarded >= 1, "no stale notification: the scenario did not make the reader late")
        check(link.ui.state == "SHOWING", f"UI state {link.ui.state}")
        print(f"      late reader: {len(frames)} FRAME messages for {last_generation} published frames, {discarded} discarded as stale, last accepted generation {accepted[-1]}")
    finally:
        if link:
            link.close()
        host.kill()


# --------------------------------------------------------------------------------------------------------------------
# Scenario 3: a mode change
# --------------------------------------------------------------------------------------------------------------------


def s3_mode_change_python(ctx: Context) -> None:
    host = ctx.host("mode-change")
    link = None
    try:
        link = Link(host)
        events = []  # (time, "MODE"/"FRAME", parsed, outcome)
        deadline = time.monotonic() + 15
        current_mode = None
        while time.monotonic() < deadline and len([e for e in events if e[1] == "FRAME"]) < 2:
            message = link.reader.try_message(5.0)
            check(message not in (None, EOF), f"the stream stopped: {events}")
            kind, payload = message
            now = time.monotonic()
            if kind == KIND_MODE:
                current_mode = parse_mode(payload)
                link.ui.mode(current_mode["active"])
                events.append((now, "MODE", current_mode, None))
            elif kind == KIND_FRAME:
                frame = parse_frame(payload)
                check(frame["count"] == current_mode["count"], f"FRAME {frame} of an older mode than the last MODE {current_mode}")
                outcome = link.read_slot(frame["slot"], frame["generation"], frame["count"])
                check(outcome[0] == "ok", f"frame {frame}: {outcome}")
                check((outcome[1], outcome[2]) == (current_mode["width"], current_mode["height"]), "frame size differs from the MODE")
                link.ui.frame_shown()
                events.append((now, "FRAME", frame, outcome))
        kinds = [(e[1], e[2]["count"]) for e in events]
        first_count = events[0][2]["count"]
        check(kinds == [("MODE", first_count), ("FRAME", first_count), ("MODE", first_count + 1), ("FRAME", first_count + 1)],
              f"order of notifications {kinds}")
        gap = events[3][0] - events[2][0]
        check(gap >= 0.4, f"MODE and the first frame of the new mode were {gap:.2f} s apart: no observable MODE_CHANGING window")
        check(events[3][3][3] == tag_pixels(events[3][2]["generation"], len(events[3][3][3])), "the first frame of the new mode is not the published one")
        check(link.ui.history == ["CONNECTING", "NO_SIGNAL", "SHOWING", "MODE_CHANGING", "SHOWING"], f"UI states {link.ui.history}")
        text = host.stderr_text()
        check(re.search(r"frame of the replaced mode \d+ published=false", text), "the frame of the replaced mode was not refused by the writer")
    finally:
        if link:
            link.close()
        host.kill()


def s3_mode_change_viewer(ctx: Context) -> None:
    run = ctx.watch(ctx.vmhost_tail("mode-change"), watch_ms=3500)
    sequence = [text for _, text in run.states if text.startswith(("SHOWING", "MODE_CHANGING"))]
    check(sequence == ["SHOWING width=64 height=48 mode_change_count=1", "MODE_CHANGING", "SHOWING width=32 height=24 mode_change_count=2"],
          f"UI states {run.states}")
    check(run.end["state"] == "SHOWING" and run.end["generation"] == "2" and run.end["accepted"] == "2", f"viewer-end {run.end}")


# --------------------------------------------------------------------------------------------------------------------
# Scenario 4: an incompatible version
# --------------------------------------------------------------------------------------------------------------------


def s4a_the_writer_answers_a_foreign_hello(ctx: Context) -> None:
    for version in (1, 2, 9):
        host = ctx.host(None)
        try:
            host.sock.sendall(b"ODFT" + struct.pack("<HHII", version, KIND_HELLO, 0, 0))
            reply = b""
            while len(reply) < 16:
                data = host.sock.recv(16 - len(reply))
                check(data != b"", f"HELLO v{version}: closed without VERSION")
                reply += data
            check(reply == dts.header(KIND_VERSION, 0, version=dts.VERSION), f"HELLO v{version}: reply {reply.hex()}")
            check(host.sock.recv(1) == b"", f"HELLO v{version}: bytes after VERSION (memory must not be sent)")
            host.wait()
            check(os.waitstatus_to_exitcode(host.status) == 0, f"HELLO v{version}: VMHost status {host.status}")
        finally:
            host.kill()


def s4b_the_ui_refuses(ctx: Context) -> None:
    script = Path(__file__).resolve()
    for kind, expected in (
        ("version", "VMHost speaks frame transport v9"),
        ("announce-v2", "VMHost speaks frame transport v2"),
        ("memory-v2", "memory header version 2"),
        ("memory-format", "pixel_format 9"),
    ):
        run = ctx.watch(f"{sys.executable} '{script}' --fake-vmhost {kind}", watch_ms=8000)
        check("SHOWING" not in run.names and run.names[-1] == "INCOMPATIBLE", f"{kind}: UI states {run.states}")
        check(expected in run.states[-1][1], f"{kind}: reason {run.states[-1][1]!r} does not name '{expected}'")
        check(run.end["state"] == "INCOMPATIBLE" and run.end["accepted"] == "0" and run.end["generation"] == "0", f"{kind}: viewer-end {run.end}")
        check(run.end["vmhost_status"] == "0", f"{kind}: the fake VMHost ended with {run.end['vmhost_status']}")


def fake_vmhost(kind: str) -> int:
    """A VMHost that speaks a foreign version (fd 3, like the real one). Memory kinds also send a valid-looking MODE and FRAME."""
    import _posixshmem

    sock = socket.socket(fileno=3)
    sock.settimeout(10)
    hello = b""
    while len(hello) < 16:
        data = sock.recv(16 - len(hello))
        if not data:
            return 1
        hello += data
    if kind == "version":
        sock.sendall(dts.header(KIND_VERSION, 0, version=9))
    else:
        slots, slot_size, width, height = 3, 16384, 64, 48
        total = SLOTS_OFFSET + slots * slot_size
        name = f"/odft.f{os.getpid():x}{int(time.time()) & 0xFFFF:x}"
        fd = _posixshmem.shm_open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
        _posixshmem.shm_unlink(name)
        os.ftruncate(fd, total)
        mapped = mmap.mmap(fd, total)
        version = 2 if kind == "memory-v2" else 3
        pixel_format = 9 if kind == "memory-format" else 1
        struct.pack_into("<IHHIIIIIIIIIIQII", mapped, 0, 0x5346444F, version, 64, 64, 128, 64, slots, SLOTS_OFFSET, slot_size,
                         pixel_format, width, height, 0, total, 384, 0)
        struct.pack_into("<8I", mapped, 384, 3, 64, 448, 16384, 16384, 64, 64, 2)
        message_version = 2 if kind == "announce-v2" else 3
        announce = b"ODFT" + struct.pack("<HHII", message_version, KIND_ANNOUNCE, 0, 24) + struct.pack("<QIIII", total, slots, 0, 1, 0)
        socket.send_fds(sock, [announce], [fd])
        mode = dts.header(KIND_MODE, 32) + struct.pack("<QIIIIII", 1, 1, width, height, width * 4, 1, 0)
        frame = dts.header(KIND_FRAME, 24) + struct.pack("<QQII", 1, 1, 0, 0)
        struct.pack_into("<Q", mapped, 128, 1)  # slot 0 looks published: a reader that read it would show something
        struct.pack_into("<QIIIII", mapped, 128 + 8, 1, width, height, width * 4, 1, 1)
        sock.sendall(mode + frame)
    try:
        while sock.recv(4096):
            pass
    except OSError:
        pass
    return 0


# --------------------------------------------------------------------------------------------------------------------
# Scenario 5: notification queue overflow
# --------------------------------------------------------------------------------------------------------------------

FLOOD = 30000


def s5a_flood_with_a_silent_reader(ctx: Context) -> None:
    host = ctx.host(f"flood:{FLOOD}")
    link = None
    try:
        link = Link(host)
        wait_text(host.stderr_text, f"flood: {FLOOD} frames published")
        match = re.search(r"last generation (\d+), mode_change_count (\d+)", host.stderr_text())
        last_generation, last_count = int(match.group(1)), int(match.group(2))
        check(alive(host), "VMHost died while its reader was not reading")
        rss_first = rss_kib(host.pid)
        time.sleep(0.5)
        check(alive(host), "VMHost died after the flood")
        rss_second = rss_kib(host.pid)
        check(rss_second - rss_first < 4096, f"VMHost RSS grew from {rss_first} to {rss_second} KiB after the flood")
        messages, ended = messages_until_quiet(link, quiet=0.6)
        check(not ended, "VMHost closed the connection")
        modes = [parse_mode(p) for _, (k, p) in messages if k == KIND_MODE]
        frames = [parse_frame(p) for _, (k, p) in messages if k == KIND_FRAME]
        check(modes[-1]["count"] == last_count and frames[-1]["generation"] == last_generation,
              f"state lost: last MODE {modes[-1]['count']} / FRAME {frames[-1]['generation']}, writer {last_count} / {last_generation}")
        outcome = link.read_slot(frames[-1]["slot"], frames[-1]["generation"], frames[-1]["count"])
        check(outcome[0] == "ok" and outcome[3] == tag_pixels(last_generation, len(outcome[3])), f"the latest frame is not readable and whole: {outcome[0]}")
        # Closing the socket is the normal end: VMHost exits 0 and prints the counters.
        link.reader.sock.close()
        host.wait()
        check(os.waitstatus_to_exitcode(host.status) == 0, f"VMHost status {host.status} after EOF")
        counters = parse_counters(host.stderr_text())
        published = last_generation  # generations 1..last_generation, one FRAME each
        check(counters["superseded_frames"] > 0 and counters["superseded_modes"] >= 0, f"counters {counters}")
        check(len(frames) + counters["superseded_frames"] == published,
              f"{len(frames)} FRAME messages received + {counters['superseded_frames']} superseded != {published} published")
        check(0 < counters["high_water_bytes"] <= 256, f"queue high water {counters['high_water_bytes']} bytes (bound: 256)")
        print(f"      overflow: {published} frames published, {len(frames)} FRAME and {len(modes)} MODE messages reached the reader, counters {counters}")
    finally:
        if link:
            link.close()
        host.kill()


def s5b_flood_read_by_the_viewer(ctx: Context) -> None:
    run = ctx.watch(ctx.vmhost_tail(f"flood:{FLOOD}"), watch_ms=6000)
    check(run.names[-1] == "SHOWING" and not {"DISCONNECTED", "INCOMPATIBLE"} & set(run.names), f"UI states {run.names[-5:]}")
    check(run.end["state"] == "SHOWING" and int(run.end["generation"]) == FLOOD + 1, f"viewer-end {run.end}: expected the last generation {FLOOD + 1}")
    check(run.end["vmhost_status"] == "0", f"VMHost status {run.end['vmhost_status']}")
    counters = parse_counters(run.stderr)
    check(counters["superseded_frames"] > 0 and 0 < counters["high_water_bytes"] <= 256, f"counters {counters}")


def parse_counters(text: str) -> dict:
    match = re.search(r"notification queue: superseded_frames=(\d+) superseded_modes=(\d+) superseded_cursors=(\d+) high_water_bytes=(\d+)", text)
    check(match is not None, f"VMHost printed no queue counters: {text.strip()[-300:]}")
    return dict(zip(("superseded_frames", "superseded_modes", "superseded_cursors", "high_water_bytes"), map(int, match.groups())))


def rss_kib(pid: int) -> int:
    return int(subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip())


# --------------------------------------------------------------------------------------------------------------------
# Scenario 6: the size of the object cannot be changed again (FRAME_TRANSPORT_V1.md 8.3)
# --------------------------------------------------------------------------------------------------------------------


def s6_second_ftruncate_is_refused(ctx: Context) -> None:
    host = ctx.host(None)
    link = None
    try:
        link = Link(host)
        total = link.total_size
        for size in (total, total // 2, GRANULE, 0, total * 2):
            try:
                os.ftruncate(link.fd, size)
            except OSError as error:
                check(error.errno == 22, f"ftruncate({size}) failed with errno {error.errno}, expected EINVAL")
            else:
                raise Failure(f"ftruncate({size}) of the received descriptor succeeded: the object can be cut under a mapped reader")
        check(os.fstat(link.fd).st_size == total, "the size of the object changed")
        check(len(link.data_map[-GRANULE:]) == GRANULE and link.data_map[-1] is not None, "the last page is not readable")  # SIGBUS would end this process
    finally:
        if link:
            link.close()
        host.kill()


SCENARIOS = [
    ("1a", s1a_kill_inside_the_first_write, True),
    ("1b", s1b_kill_inside_the_second_write, False),
    ("1c", s1c_kill_after_a_shown_frame_in_the_viewer, True),
    ("2a", s2a_hello_after_a_delay, False),
    ("2b", s2b_notifications_read_late, False),
    ("3a", s3_mode_change_python, False),
    ("3b", s3_mode_change_viewer, True),
    ("4a", s4a_the_writer_answers_a_foreign_hello, False),
    ("4b", s4b_the_ui_refuses, True),
    ("5a", s5a_flood_with_a_silent_reader, False),
    ("5b", s5b_flood_read_by_the_viewer, True),
    ("6", s6_second_ftruncate_is_refused, False),
]


def main() -> int:
    if len(sys.argv) >= 3 and sys.argv[1] == "--fake-vmhost":
        return fake_vmhost(sys.argv[2])
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--only", default="")
    parser.add_argument("viewer", nargs="?", type=Path, default=DEFAULT_VIEWER)
    parser.add_argument("vmhost", nargs="?", type=Path, default=DEFAULT_VMHOST)
    args = parser.parse_args()
    for binary in (args.viewer, args.vmhost):
        if not binary.exists():
            print(f"missing {binary} (cargo build -p od-host --offline; cd apps && swift build)", file=sys.stderr)
            return 2
    selected = {item for item in args.only.split(",") if item}
    blocked: list[str] = []
    failed = False
    for run_number in range(1, args.repeat + 1):
        with tempfile.TemporaryDirectory(prefix="display_failure_") as workdir:
            ctx = Context(args.viewer, args.vmhost, Path(workdir))
            for name, scenario, needs_viewer in SCENARIOS:
                if selected and name not in selected:
                    continue
                started = time.monotonic()
                try:
                    scenario(ctx)
                except Blocked as reason:
                    blocked.append(f"{name} {scenario.__name__}: {reason}")
                    print(f"[{run_number}] {name} {scenario.__name__}: BLOCKED ({reason})")
                except Failure as failure:
                    failed = True
                    print(f"[{run_number}] {name} {scenario.__name__}: FAIL: {failure}")
                except Exception as error:  # noqa: BLE001 - a crash of the test itself is a failure, with its type
                    failed = True
                    print(f"[{run_number}] {name} {scenario.__name__}: ERROR {type(error).__name__}: {error}")
                else:
                    print(f"[{run_number}] {name} {scenario.__name__}: ok ({time.monotonic() - started:.1f} s)")
    if failed:
        print("display_failure_smoke: FAILED")
        return 1
    if blocked:
        print("display_failure_smoke: BLOCKED: the viewer has no Metal device, so the UI-state scenarios were NOT verified; this is not a pass.")
        return EXIT_BLOCKED
    print(f"display_failure_smoke: ok ({args.repeat} run(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
