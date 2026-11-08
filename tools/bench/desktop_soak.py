#!/usr/bin/env python3
"""S11-61 long run of the desktop shell (docs/plans/STAGE_11.md section 7 S11-61, section 5; docs/testing/DESKTOP_APP.md section 3). test-only, без гостя.

What runs (default one hour, `--duration-minutes`; every interval below is multiplied by min(1, duration / 60 min), so a short check run has the same shape):

  machines   Two machines of the profile `display-test-pattern` (256 MiB, no guest, no disk), created with `odctl create` in a temporary library.
             Each has its own `od-vmhost --control-v2` session that this script holds open for the whole run (the same role as the application:
             socketpair on fd 3, request IDs, STATE_CHANGED events). `odctl run` cannot be used: its script is fixed and the process ends with it.
  every 5 min  PAUSE, a short hold, RESUME on one of the machines (alternating).
  every 15 min  STOP of one machine (alternating; the process exits), then a new od-vmhost: CONFIGURE, START.
  application  One long-lived `OpenDesktop --open <third machine> --simulate-host-event memory` for the whole run (RSS at the beginning and at
             the end). `--simulate-host-event` runs ONE handler ONCE, 1 s after the launch, so the other events are given to short-lived
             instances of the application (a few seconds each) between the pauses: sleep, wake, screens, low-space, memory in turn.
             THIS IS A SIMULATION OF THE HANDLERS, NOT A SLEEP OF THE MAC and not real displays, space or memory pressure. The application has no
             way to start a machine without the window, so its machine is `stopped`: the handlers of sleep/wake/low-space find no running machine
             and issue no PAUSE/RESUME. The two soak machines belong to this script (flock), not to the application.
  RSS        `ps -o rss= -p <pid>` of the application and of both VMHost: at the beginning (after a settle time), every minute, at the end. A VMHost
             that was stopped and started again is a new process (a new "generation"), so the growth is judged per generation, and the begin/end
             values of each machine are also written as they are.
  latency    PAUSE -> STATE_CHANGED paused, RESUME -> running, STOP -> stopped (and START -> running), from the moment before the request is written
             to the arrival of the event frame in this script. Medians of the first 10 minutes and of the last 10 minutes. The scheduled commands
             alone give 2 or 3 samples per window, so inside both windows extra probe rounds are made (PAUSE/RESUME on both machines, STOP/START
             of one); every sample carries `source` (scheduled or probe), the extra commands are counted separately.
  hangs      Every command must be answered (ACK/STATE/ERROR) within 10 s; a QUERY of both machines every minute must report the state that the
             events imply; every STATE_CHANGED must be a transition of the table (stopped->running, running->paused, paused->running,
             running->stopped, paused->stopped) and must have been caused by a command of this script. A violation stops the run (exit 1).
  unrecognized  A frame the client cannot decode (bad header, unknown kind, wrong payload size, request ID that was not asked) is counted; stderr of
             every VMHost and of every instance of the application is counted too: any of them is "not passed".

The RSS growth threshold (10 % per hour) is a HYPOTHESIS written down before the run: it is neither from the plan nor measured. Growth above it
is "не пройдено". There is no threshold for the latency comparison (no plan or measurement gives one): the ratio is only recorded.

Rules (AGENTS.md, plan section 5): the fingerprint (Mac model, macOS build, build mode, Git revision, tool versions) and the conditions (power, load
average before and after, free space) are written; the Mac is not put to sleep; nothing is created outside the temporary directory
(HOME and CFFIXED_USER_HOME of every child are temporary) except the JSON; the script fails if one of the real user directories appeared.
Output: docs/reports/data/stage11/s11-61-soak-<YYYY-MM-DD>.json (at most 100 KiB). Exit 0 only if every criterion of the verdict is passed.

Usage: python3 tools/bench/desktop_soak.py [--duration-minutes N] [--bundle PATH] [--configuration debug|release] [--odctl PATH]
                                           [--out-dir DIR] [--date YYYY-MM-DD]
Needs `python3 tools/build/build_app_bundle.py` and `cd apps && swift build`. Python 3.9+, macOS only.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import select
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests" / "system"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import desktop_lifecycle_bench as bench  # noqa: E402  (fingerprint, conditions, command_output)
import vmhost_control_v2_lifecycle_smoke as v2  # noqa: E402  (frame builders, Host, Conn)

DEFAULT_BUNDLE = ROOT / "target" / "app-bundle" / "OpenDesktop.app"
DEFAULT_ODCTL = ROOT / "apps" / ".build" / "debug" / "odctl"
DEFAULT_OUT = ROOT / "docs" / "reports" / "data" / "stage11"
MAX_JSON = 100 * 1024
GIB = 1 << 30
LABEL = "test-only, без гостя"
PROFILE = "display-test-pattern"
MEMORY_MIB = 256
COMMAND_TIMEOUT = 10.0
RSS_GROWTH_LIMIT_PERCENT = 10.0
EVENT_KINDS = ["sleep", "wake", "screens", "low-space", "memory"]
STATE_NAMES = {1: "stopped", 2: "running", 3: "paused", 5: "failed"}
# The table of transitions that a STATE_CHANGED may carry (control v2; start/pause/resume/stop of the test-only profile).
TRANSITIONS = {(v2.STOPPED, v2.RUNNING), (v2.RUNNING, v2.PAUSED), (v2.PAUSED, v2.RUNNING), (v2.RUNNING, v2.STOPPED), (v2.PAUSED, v2.STOPPED)}
REAL_DIRECTORIES = [Path.home() / "Library" / "Application Support" / "OpenDesktop", Path.home() / "Documents" / "Open Desktop"]
FAILURES = (v2.Failure, bench.Failure)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise v2.Failure(message)


def rss_kib(pid: int) -> int:
    value = bench.command_output("ps", "-o", "rss=", "-p", str(pid))
    check(value.isdigit(), f"ps -o rss= -p {pid}: {value!r}")
    return int(value)


def median_block(values: list) -> dict:
    if not values:
        return {"n": 0}
    return {"n": len(values), "min": round(min(values), 3), "median": round(statistics.median(values), 3), "max": round(max(values), 3)}


class Generation:
    """One od-vmhost process of a machine: from `running` to STOP; its RSS samples are (t_s, KiB)."""

    def __init__(self, pid: int, running_t: float) -> None:
        self.pid = pid
        self.running_t = running_t
        self.samples: list[tuple[float, int]] = []
        self.ended_t: float | None = None


class Machine:
    def __init__(self, name: str, bundle: Path) -> None:
        self.name = name
        self.bundle = bundle
        self.host: v2.Host | None = None
        self.conn: v2.Conn | None = None
        self.state = v2.STOPPED  # the state that the events of this process imply
        self.generations: list[Generation] = []
        self.last_command = 0.0


class ShortApp:
    def __init__(self, process: subprocess.Popen, kind: str, started: float, stderr_path: Path, hold: float) -> None:
        self.process = process
        self.kind = kind
        self.started = started
        self.stderr_path = stderr_path
        self.hold = hold


class Soak:
    def __init__(self, args: argparse.Namespace, work: Path) -> None:
        self.args = args
        self.work = work
        self.duration = args.duration_minutes * 60.0
        self.scale = min(1.0, self.duration / 3600.0)
        self.helper = args.bundle / "Contents" / "Helpers" / "od-vmhost"
        self.executable = args.bundle / "Contents" / "MacOS" / "OpenDesktop"
        self.home = work / "home"
        self.home.mkdir()
        self.library = work / "library"
        self.env = dict(os.environ, HOME=str(self.home), CFFIXED_USER_HOME=str(self.home), ODCTL_VMHOST=str(self.helper))
        os.environ.update(HOME=str(self.home), CFFIXED_USER_HOME=str(self.home), ODCTL_VMHOST=str(self.helper))  # inherited by od-vmhost
        self.machines = [Machine("A", self.library / "soak-a.odvm"), Machine("B", self.library / "soak-b.odvm")]
        self.app_bundle = self.library / "soak-app.odvm"
        self.t0 = 0.0
        self.settle = max(5.0, 30.0 * self.scale)
        self.window = 600.0 * self.scale
        self.latencies: list[dict] = []
        self.timeline: list[list] = []
        self.app_samples: list[tuple[float, int]] = []
        self.counts = {"commands_sent": 0, "commands_answered": 0, "hangs": 0, "query_checks": 0, "state_mismatches": 0, "invalid_transitions": 0,
                       "unsolicited_state_changes": 0, "unrecognized_messages": 0, "stats_frames": 0, "vmhost_stderr_lines": 0,
                       "app_stderr_lines": 0, "vmhost_exit_status_not_zero": 0, "vmhost_forced_kill": 0, "shutdown_failures": 0,
                       "app_exit_unexpected": 0,
                       "pause_resume_cycles": {"scheduled": 0, "probe": 0}, "stop_start_cycles": {"scheduled": 0, "probe": 0},
                       "simulated_host_events": {kind: 0 for kind in EVENT_KINDS}, "simulated_host_event_launches": 0}
        self.events_log: list[dict] = []
        self.next_alternate = {"pause": 0, "restart": 0}
        self.short_apps: list[ShortApp] = []
        self.long_app: subprocess.Popen | None = None
        self.long_app_stderr = work / "app-long.stderr"
        self.long_app_started = 0.0
        self.error: str | None = None
        self.vmhost_stderr_text: list[str] = []
        self.app_stderr_text: list[str] = []

    # ------------------------------------------------------------------------------------------------------------------------------
    # time and logging

    def elapsed(self) -> float:
        return time.monotonic() - self.t0

    def log(self, text: str) -> None:
        print(f"[{self.elapsed():8.1f}s] {text}", flush=True)

    def sleep(self, seconds: float) -> None:
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.tick()
            time.sleep(min(0.05, max(0.0, end - time.monotonic())))

    # ------------------------------------------------------------------------------------------------------------------------------
    # control v2 session

    def account_change(self, machine: Machine, body: bytes, solicited: bool) -> tuple[int, int, int]:
        if len(body) != 12:
            self.counts["unrecognized_messages"] += 1
            raise v2.Failure(f"machine {machine.name}: STATE_CHANGED payload of {len(body)} bytes")
        before, after, reason = struct.unpack("<III", body)
        if not solicited:
            self.counts["unsolicited_state_changes"] += 1
            raise v2.Failure(f"machine {machine.name}: unsolicited STATE_CHANGED {STATE_NAMES.get(before, before)} -> {STATE_NAMES.get(after, after)} (reason {reason})")
        if (before, after) not in TRANSITIONS or before != machine.state:
            self.counts["invalid_transitions"] += 1
            raise v2.Failure(f"machine {machine.name}: STATE_CHANGED {STATE_NAMES.get(before, before)} -> {STATE_NAMES.get(after, after)} "
                             f"is not in the table or does not follow {STATE_NAMES.get(machine.state, machine.state)}")
        machine.state = after
        return before, after, reason

    def account_stats(self, machine: Machine, body: bytes) -> None:
        machine.conn.stats.clear()
        try:
            v2.parse_stats(body)
        except (v2.Failure, struct.error):
            self.counts["unrecognized_messages"] += 1
            raise v2.Failure(f"machine {machine.name}: STATS payload that cannot be decoded ({len(body)} bytes)")
        self.counts["stats_frames"] += 1

    def request(self, machine: Machine, kind: int, payload: bytes, target: int | None = None) -> dict:
        """One request with the next ID; returns the terminal frame and the milliseconds to the STATE_CHANGED to `target` and to the answer."""
        conn = machine.conn
        rid = conn.next_id
        conn.next_id += 1
        self.counts["commands_sent"] += 1
        deadline = time.monotonic() + COMMAND_TIMEOUT
        started = time.perf_counter_ns()
        conn.send(v2.frame(kind, rid, payload))
        machine.last_command = time.monotonic()
        event_ms = None
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                self.counts["hangs"] += 1
                raise v2.Failure(f"hang: machine {machine.name}, kind {kind}: no answer in {COMMAND_TIMEOUT:.0f} s")
            try:
                item = conn.recv(remaining)
            except v2.Failure as failure:
                if "timeout" in str(failure):
                    self.counts["hangs"] += 1
                    raise v2.Failure(f"hang: machine {machine.name}, kind {kind}: no answer in {COMMAND_TIMEOUT:.0f} s")
                self.counts["unrecognized_messages"] += 1
                raise
            arrived = time.perf_counter_ns()
            if item is None:
                raise v2.Failure(f"machine {machine.name}: EOF while waiting for the answer to kind {kind}")
            got_kind, got_rid, body = item
            if got_kind == v2.STATS:
                self.account_stats(machine, body)
                continue
            if got_rid != rid:
                self.counts["unrecognized_messages"] += 1
                raise v2.Failure(f"machine {machine.name}: answer to kind {kind} carries request id {got_rid}, expected {rid}")
            if got_kind == v2.STATE_CHANGED:
                _, after, _ = self.account_change(machine, body, solicited=True)
                if after == target and event_ms is None:
                    event_ms = (arrived - started) / 1e6
                continue
            if got_kind not in (v2.ACK, v2.STATE, v2.ERROR):
                self.counts["unrecognized_messages"] += 1
                raise v2.Failure(f"machine {machine.name}: frame of unknown kind {got_kind}")
            self.counts["commands_answered"] += 1
            if got_kind == v2.ERROR:
                error = v2.parse_error(body)
                raise v2.Failure(f"machine {machine.name}: kind {kind} refused: code {error['code']} ({error['detail']})")
            if got_kind == v2.ACK:
                ack = v2.parse_ack(body)
                if ack["code"] != 0 or ack["kind"] != kind:
                    self.counts["unrecognized_messages"] += 1
                    raise v2.Failure(f"machine {machine.name}: ACK {ack} for kind {kind}")
            if target is not None and event_ms is None:
                raise v2.Failure(f"machine {machine.name}: no STATE_CHANGED to {STATE_NAMES.get(target, target)} before the answer to kind {kind}")
            return {"kind": got_kind, "body": body, "event_ms": event_ms, "answer_ms": (arrived - started) / 1e6}

    def drain(self, machine: Machine) -> None:
        """Reads the frames that arrived while no command was in flight (STATS; anything else is a violation)."""
        conn = machine.conn
        if conn is None:
            return
        while True:
            complete = False
            if len(conn.buffer) >= 16:
                _, _, _, _, length = struct.unpack("<4sHHII", conn.buffer[:16])
                complete = len(conn.buffer) >= 16 + length
            if not complete and not select.select([conn.sock], [], [], 0)[0]:
                return
            try:
                item = conn.recv(2.0)
            except v2.Failure as failure:
                if "timeout" in str(failure):
                    return
                self.counts["unrecognized_messages"] += 1
                raise
            if item is None:
                raise v2.Failure(f"machine {machine.name}: the VMHost closed the connection while no command was in flight")
            kind, rid, body = item
            if kind == v2.STATS:
                self.account_stats(machine, body)
            elif kind == v2.STATE_CHANGED:
                self.account_change(machine, body, solicited=False)
            else:
                self.counts["unrecognized_messages"] += 1
                raise v2.Failure(f"machine {machine.name}: unexpected frame kind {kind} (request id {rid}) while idle")

    def tick(self) -> None:
        for machine in self.machines:
            self.drain(machine)
        self.reap_short_apps()
        for app, label in [(self.long_app, "long-lived application")]:
            if app is not None and app.poll() is not None:
                self.counts["app_exit_unexpected"] += 1
                raise v2.Failure(f"the {label} ended by itself with status {app.returncode}")

    # ------------------------------------------------------------------------------------------------------------------------------
    # machines

    def record(self, op: str, machine: Machine, ms: float, source: str) -> None:
        self.latencies.append({"t_s": round(self.elapsed(), 1), "op": op, "machine": machine.name, "ms": round(ms, 3), "source": source})

    def launch(self, machine: Machine, source: str) -> None:
        machine.host = v2.Host(self.helper, str(machine.bundle))
        machine.conn = machine.host.conn
        machine.state = v2.STOPPED
        try:
            machine.conn.handshake()
        except v2.Failure as failure:
            if "timeout" in str(failure):
                self.counts["hangs"] += 1
            raise
        machine.conn.expect_ok(v2.CONFIGURE, v2.configure_payload(memory=MEMORY_MIB))
        result = self.request(machine, v2.START, struct.pack("<Q", 0), v2.RUNNING)
        machine.generations.append(Generation(machine.host.pid, self.elapsed()))
        self.record("start", machine, result["event_ms"], source)
        self.log(f"machine {machine.name}: started, pid {machine.host.pid}, START -> running {result['event_ms']:.2f} ms ({source})")

    def stop(self, machine: Machine, source: str) -> None:
        sample_rss = rss_kib(machine.host.pid)
        generation = machine.generations[-1]
        generation.samples.append((round(self.elapsed(), 1), sample_rss))
        result = self.request(machine, v2.STOP, struct.pack("<Q", 0), v2.STOPPED)
        self.record("stop", machine, result["event_ms"], source)
        generation.ended_t = self.elapsed()
        machine.conn.expect_closed()
        status = machine.host.wait()
        if status != 0:
            self.counts["vmhost_exit_status_not_zero"] += 1
            raise v2.Failure(f"machine {machine.name}: od-vmhost exited with status {status}")
        self.collect_vmhost_stderr(machine)
        machine.host.cleanup()
        machine.host = None
        machine.conn = None
        self.log(f"machine {machine.name}: STOP -> stopped {result['event_ms']:.2f} ms ({source}), process ended with status 0")

    def collect_vmhost_stderr(self, machine: Machine) -> None:
        text = machine.host.stderr_text().strip()
        if text:
            lines = text.splitlines()
            self.counts["vmhost_stderr_lines"] += len(lines)
            self.vmhost_stderr_text.extend(f"{machine.name}: {line}" for line in lines[:5])

    def pause_resume(self, machine: Machine, source: str, hold: float | None = None) -> None:
        paused = self.request(machine, v2.PAUSE, struct.pack("<II", v2.USER, 0), v2.PAUSED)
        self.record("pause", machine, paused["event_ms"], source)
        self.sleep(hold if hold is not None else max(0.5, 2.0 * self.scale))
        resumed = self.request(machine, v2.RESUME, struct.pack("<II", v2.USER, 0), v2.RUNNING)
        self.record("resume", machine, resumed["event_ms"], source)
        self.counts["pause_resume_cycles"][source] += 1
        self.log(f"machine {machine.name}: PAUSE -> paused {paused['event_ms']:.2f} ms, RESUME -> running {resumed['event_ms']:.2f} ms ({source})")

    def restart(self, machine: Machine, source: str) -> None:
        self.stop(machine, source)
        self.sleep(max(0.5, 1.0 * self.scale))
        self.launch(machine, source)
        self.counts["stop_start_cycles"][source] += 1

    def alternate(self, key: str) -> Machine:
        machine = self.machines[self.next_alternate[key] % 2]
        self.next_alternate[key] += 1
        return machine

    def query_check(self) -> None:
        for machine in self.machines:
            result = self.request(machine, v2.QUERY, b"")
            check(result["kind"] == v2.STATE and len(result["body"]) == 12, f"machine {machine.name}: QUERY answer is not a STATE")
            state, _, _ = struct.unpack("<III", result["body"])
            self.counts["query_checks"] += 1
            if state != machine.state:
                self.counts["state_mismatches"] += 1
                raise v2.Failure(f"machine {machine.name}: QUERY reports {STATE_NAMES.get(state, state)}, the events imply {STATE_NAMES.get(machine.state, machine.state)}")

    # ------------------------------------------------------------------------------------------------------------------------------
    # applications

    def app_command(self, kind: str | None, stderr_path: Path) -> subprocess.Popen:
        command = [str(self.executable), "--open", str(self.app_bundle), "--vmhost", str(self.helper)]
        if kind:
            command += ["--simulate-host-event", kind]
        with open(stderr_path, "wb") as stderr:
            return subprocess.Popen(command, env=self.env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=stderr)

    def start_long_app(self) -> None:
        self.long_app = self.app_command("memory", self.long_app_stderr)
        self.long_app_started = time.monotonic()
        self.counts["simulated_host_events"]["memory"] += 1
        self.counts["simulated_host_event_launches"] += 1
        self.events_log.append({"t_s": 0.0, "kind": "memory", "target": "long-lived application (launch)", "simulation": "симуляция, не настоящее давление памяти"})
        time.sleep(1.5)
        check(self.long_app.poll() is None, "the long-lived application ended during the first 1.5 s")

    def launch_event_app(self, kind: str) -> None:
        path = self.work / f"app-{kind}-{len(self.events_log)}.stderr"
        process = self.app_command(kind, path)
        self.short_apps.append(ShortApp(process, kind, time.monotonic(), path, hold=3.0))
        self.counts["simulated_host_events"][kind] += 1
        self.counts["simulated_host_event_launches"] += 1
        self.events_log.append({"t_s": round(self.elapsed(), 1), "kind": kind, "target": "short-lived application",
                                "simulation": "симуляция обработчика, не настоящий сон/экраны/место/память"})
        self.log(f"application: --simulate-host-event {kind} (short-lived, simulation, not a sleep of the Mac)")

    def finish_short_app(self, app: ShortApp, force_wait: bool) -> None:
        if app.process.poll() is not None:
            self.counts["app_exit_unexpected"] += 1
            raise v2.Failure(f"the short-lived application ({app.kind}) ended by itself with status {app.process.returncode}")
        app.process.send_signal(signal.SIGTERM)
        try:
            app.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            app.process.kill()
            app.process.wait()
            self.counts["hangs"] += 1
            raise v2.Failure(f"hang: the application ({app.kind}) did not end on SIGTERM within 15 s (killed)")
        if app.process.returncode not in (0, -signal.SIGTERM):
            self.counts["app_exit_unexpected"] += 1
            raise v2.Failure(f"the application ({app.kind}) ended with status {app.process.returncode} after SIGTERM")
        self.collect_app_stderr(app.stderr_path, f"short-lived {app.kind}")

    def collect_app_stderr(self, path: Path, label: str) -> None:
        text = path.read_text(encoding="utf-8", errors="replace").strip()
        if text:
            lines = text.splitlines()
            self.counts["app_stderr_lines"] += len(lines)
            self.app_stderr_text.extend(f"{label}: {line}" for line in lines[:5])

    def reap_short_apps(self) -> None:
        now = time.monotonic()
        for app in list(self.short_apps):
            if now - app.started >= app.hold or app.process.poll() is not None:
                self.short_apps.remove(app)
                self.finish_short_app(app, force_wait=False)

    # ------------------------------------------------------------------------------------------------------------------------------
    # RSS

    def sample_rss(self, label: str) -> None:
        elapsed = round(self.elapsed(), 1)
        app = rss_kib(self.long_app.pid)
        self.app_samples.append((elapsed, app))
        row = [elapsed, app]
        for machine in self.machines:
            generation = machine.generations[-1]
            value = rss_kib(machine.host.pid)
            row.append(value)
            if elapsed - generation.running_t >= self.settle:
                generation.samples.append((elapsed, value))
        self.timeline.append(row)
        self.log(f"rss ({label}): application {app} KiB, A {row[2]} KiB, B {row[3]} KiB")

    def minute_tick(self) -> None:
        self.query_check()
        self.sample_rss("timeline")

    # ------------------------------------------------------------------------------------------------------------------------------
    # schedule

    def build_schedule(self) -> list[tuple[float, int, str, object]]:
        s = self.scale
        tasks: list[tuple[float, int, str, object]] = []
        latest = self.duration - 10.0

        def add(t: float, label: str, function) -> None:
            if t <= latest:
                tasks.append((t, len(tasks), label, function))

        tasks.append((self.settle, 0, "begin", lambda: self.sample_rss("begin")))
        step = 60.0 * s
        t = step
        while t <= latest:
            add(t, "timeline", self.minute_tick)
            t += step
        t = 300.0 * s
        while t <= latest:
            add(t, "pause/resume", lambda: self.pause_resume(self.alternate("pause"), "scheduled"))
            t += 300.0 * s
        t = 900.0 * s
        while t <= latest:
            add(t, "stop/start", lambda: self.restart(self.alternate("restart"), "scheduled"))
            t += 900.0 * s
        t = 150.0 * s
        index = 0
        while t <= latest:
            kind = EVENT_KINDS[index % len(EVENT_KINDS)]
            add(t, f"event {kind}", lambda kind=kind: self.launch_event_app(kind))
            index += 1
            t += 300.0 * s
        for base in (0.0, self.duration - self.window - 10.0):
            for offset in (90.0, 210.0, 330.0, 450.0, 570.0):
                add(base + offset * s, "probe pause/resume A", lambda: self.pause_resume(self.machines[0], "probe", hold=max(0.5, 1.0 * s)))
                add(base + offset * s + 20.0 * s, "probe pause/resume B", lambda: self.pause_resume(self.machines[1], "probe", hold=max(0.5, 1.0 * s)))
            for offset in (150.0, 360.0, 540.0):
                add(base + offset * s, "probe stop/start", lambda: self.restart(self.alternate("restart"), "probe"))
        tasks.sort(key=lambda task: (task[0], task[1]))
        return tasks

    def run(self) -> None:
        env = self.env
        for name in ("soak-a", "soak-b", "soak-app"):
            done = subprocess.run([str(self.args.odctl), "create", "--name", name, "--profile", PROFILE, "--memory-mib", str(MEMORY_MIB),
                                   "--library", str(self.library)], env=env, capture_output=True, text=True, timeout=60)
            check(done.returncode == 0, f"odctl create {name}: exit {done.returncode}: {done.stdout} {done.stderr}")
        self.t0 = time.monotonic()
        self.start_long_app()
        for machine in self.machines:
            self.launch(machine, "setup")
        self.t0 = time.monotonic()  # the clock of the run starts when both machines are running
        for machine in self.machines:
            machine.generations[-1].running_t = 0.0
        self.log(f"run: {self.duration:.0f} s, time scale {self.scale:.4f}, settle {self.settle:.1f} s, windows {self.window:.0f} s")
        tasks = self.build_schedule()
        self.log(f"schedule: {len(tasks)} tasks")
        for t, _, label, function in tasks:
            while self.elapsed() < t:
                self.tick()
                time.sleep(0.05)
            function()
        while self.elapsed() < self.duration:
            self.tick()
            time.sleep(0.05)
        self.query_check()
        self.sample_rss("end")
        self.log("end of the run: stopping the machines and the application")

    def shutdown(self) -> None:
        """Used after a normal end and after a failure. Never raises; every problem is recorded in counts or in self.error.

        A VMHost counts as shut down gracefully only if it was stopped by STOP and exited by itself with status 0.
        Anything else (an exception, a forced kill, a non-zero status) fails the run.
        """
        for machine in self.machines:
            if machine.host is None:
                continue
            try:
                if machine.host.status is None and machine.conn is not None and machine.state in (v2.RUNNING, v2.PAUSED):
                    self.request(machine, v2.STOP, struct.pack("<Q", 0), v2.STOPPED)
                    machine.generations[-1].ended_t = self.elapsed()
                    machine.conn.expect_closed()
                    status = machine.host.wait()
                    if status != 0:
                        self.counts["vmhost_exit_status_not_zero"] += 1
                self.collect_vmhost_stderr(machine)
            except Exception as error:  # noqa: BLE001  (recorded, the run fails)
                self.counts["shutdown_failures"] += 1
                self.error = self.error or f"shutdown of {machine.name}: {error}"
            if machine.host.status is None:
                # still alive or not reaped: cleanup kills it, which is not a graceful end
                self.counts["vmhost_forced_kill"] += 1
                if self.error is None:
                    self.error = f"VMHost of {machine.name} did not exit by itself"
            machine.host.cleanup()
            machine.host = None
        for app in self.short_apps:
            if app.process.poll() is None:
                app.process.kill()
                app.process.wait()
        self.short_apps = []
        if self.long_app is not None and self.long_app.poll() is None:
            self.long_app.send_signal(signal.SIGTERM)
            try:
                self.long_app.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.long_app.kill()
                self.long_app.wait()
                self.counts["hangs"] += 1
                self.error = self.error or "the long-lived application did not end on SIGTERM within 15 s (killed)"
        if self.long_app is not None and self.long_app.returncode not in (0, -signal.SIGTERM, None):
            self.counts["app_exit_unexpected"] += 1
        if self.long_app_stderr.exists():
            self.collect_app_stderr(self.long_app_stderr, "long-lived")


def percent_growth(first: int, last: int) -> float:
    return round((last - first) * 100.0 / first, 2) if first else 0.0


def build_result(soak: Soak, args: argparse.Namespace, fingerprint: dict, conditions_before: dict, conditions_after: dict, reproduce: str,
                 wall_s: float, orphans: list[str], real_dirs_ok: bool) -> dict:
    s = soak.scale
    # RSS of the application
    app_first, app_last = soak.app_samples[0], soak.app_samples[-1]
    app_growth = percent_growth(app_first[1], app_last[1])
    # RSS of the VMHost: per machine begin/end as they are, per generation growth
    vmhost: dict = {}
    judged: list[dict] = []
    min_judged = 300.0 * s
    for machine in soak.machines:
        generations = []
        for number, generation in enumerate(machine.generations, 1):
            entry = {"generation": number, "pid": generation.pid, "running_at_s": round(generation.running_t, 1),
                     "ended_at_s": None if generation.ended_t is None else round(generation.ended_t, 1), "samples": len(generation.samples)}
            if len(generation.samples) >= 2:
                first, last = generation.samples[0], generation.samples[-1]
                span = last[0] - first[0]
                entry.update({"first_kib": first[1], "last_kib": last[1], "span_s": round(span, 1), "growth_percent": percent_growth(first[1], last[1]),
                              "judged": span >= min_judged})
                if span >= min_judged:
                    judged.append({"machine": machine.name, **entry})
            else:
                entry["judged"] = False
            generations.append(entry)
        gens = [g for g in machine.generations if g.samples]
        vmhost[f"vmhost_{machine.name.lower()}"] = {
            "begin": {"pid": gens[0].pid, "t_s": gens[0].samples[0][0], "kib": gens[0].samples[0][1]},
            "end": {"pid": gens[-1].pid, "t_s": gens[-1].samples[-1][0], "kib": gens[-1].samples[-1][1],
                    "note": "the process at the end is not the one at the beginning when the machine was restarted: begin/end are of different processes then"},
            "generations": generations}
    worst = max(judged, key=lambda g: g["growth_percent"]) if judged else None
    rss = {
        "unit": "KiB (`ps -o rss= -p <pid>`)",
        "application": {"begin": {"pid_note": "one long-lived process", "t_s": app_first[0], "kib": app_first[1]},
                        "end": {"t_s": app_last[0], "kib": app_last[1]}, "growth_percent": app_growth,
                        "note": "the application got one simulated event (memory) at its launch; the machine of the application is stopped"},
        **vmhost,
        "settle_s_before_first_sample": round(soak.settle, 1),
        "timeline_columns": ["t_s", "application", "vmhost_a", "vmhost_b"],
        "timeline": soak.timeline,
    }
    # latency
    windows = {"first": (0.0, soak.window), "last": (soak.duration - soak.window, soak.duration + 1.0)}
    latency: dict = {"unit": "ms", "window_s": round(soak.window, 1),
                     "method": "from before the request is written to the arrival of the STATE_CHANGED frame in this script (the role of the application)"}
    ratios = {}
    for op, label in (("pause", "PAUSE -> paused"), ("resume", "RESUME -> running"), ("stop", "STOP -> stopped"), ("start", "START -> running (extra)")):
        entry: dict = {"command": label}
        medians = {}
        for name, (lo, hi) in windows.items():
            values = [x["ms"] for x in soak.latencies if x["op"] == op and lo <= x["t_s"] < hi]
            entry[f"{name}_window"] = median_block(values)
            if values:
                medians[name] = statistics.median(values)
        if "first" in medians and "last" in medians and medians["first"] > 0:
            entry["median_last_over_first"] = round(medians["last"] / medians["first"], 3)
            ratios[op] = entry["median_last_over_first"]
        latency[op] = entry
    latency["no_threshold"] = ("для задержек нет порога ни в плане, ни из замера: отношение медиан записано как наблюдение, критерием приёмки не является; "
                               "число образцов в окне мало (см. n)")
    latency["samples"] = soak.latencies
    # verdict
    c = soak.counts
    criteria = []

    def criterion(name: str, passed: bool | None, detail: str) -> None:
        criteria.append({"name": name, "result": "не оценено" if passed is None else ("выполнено" if passed else "не пройдено"), "detail": detail})

    criterion(f"рост RSS приложения не больше {RSS_GROWTH_LIMIT_PERCENT:g} % (порог — гипотеза)", app_growth <= RSS_GROWTH_LIMIT_PERCENT,
              f"{app_first[1]} -> {app_last[1]} KiB, {app_growth} %")
    if worst is None:
        criterion(f"рост RSS VMHost за жизнь процесса не больше {RSS_GROWTH_LIMIT_PERCENT:g} % (порог — гипотеза)", None,
                  "ни одно поколение процесса не прожило достаточно долго для оценки")
    else:
        criterion(f"рост RSS VMHost за жизнь процесса не больше {RSS_GROWTH_LIMIT_PERCENT:g} % (порог — гипотеза)",
                  all(g["growth_percent"] <= RSS_GROWTH_LIMIT_PERCENT for g in judged),
                  f"оценено поколений: {len(judged)} (каждое не короче {round(min_judged)} с между первым и последним замером); "
                  f"наибольший рост {worst['growth_percent']} % (машина {worst['machine']}, процесс {worst['pid']}, {worst['first_kib']} -> {worst['last_kib']} KiB)")
    criterion("ни одного зависшего состояния", c["hangs"] == 0 and c["state_mismatches"] == 0 and c["commands_sent"] == c["commands_answered"],
              f"команд {c['commands_sent']}, ответов {c['commands_answered']}, зависаний {c['hangs']}, расхождений QUERY с событиями {c['state_mismatches']} "
              f"из {c['query_checks']} проверок; таймаут ответа {COMMAND_TIMEOUT:.0f} с")
    criterion("все состояния из таблицы переходов", c["invalid_transitions"] == 0 and c["unsolicited_state_changes"] == 0,
              f"переходов вне таблицы {c['invalid_transitions']}, событий без команды {c['unsolicited_state_changes']}")
    criterion("ни одного нераспознанного сообщения", c["unrecognized_messages"] == 0 and c["vmhost_stderr_lines"] == 0 and c["app_stderr_lines"] == 0,
              f"нераспознанных кадров {c['unrecognized_messages']}, строк stderr VMHost {c['vmhost_stderr_lines']}, строк stderr приложения {c['app_stderr_lines']}"
              + (f"; примеры: {(soak.vmhost_stderr_text + soak.app_stderr_text)[:5]}" if (soak.vmhost_stderr_text or soak.app_stderr_text) else ""))
    criterion("процессы завершились штатно, сирот нет",
              c["vmhost_exit_status_not_zero"] == 0 and c["vmhost_forced_kill"] == 0 and c["shutdown_failures"] == 0
              and c["app_exit_unexpected"] == 0 and not orphans,
              f"VMHost с кодом не 0: {c['vmhost_exit_status_not_zero']}, принудительных завершений VMHost (SIGKILL): {c['vmhost_forced_kill']}, "
              f"ошибок при остановке: {c['shutdown_failures']}, неожиданных завершений приложения {c['app_exit_unexpected']}, "
              f"оставшиеся od-vmhost после очистки: {orphans or 'нет'}")
    criterion("реальные каталоги пользователя не появились", real_dirs_ok, "~/Library/Application Support/OpenDesktop и ~/Documents/Open Desktop")
    if soak.error:
        criterion("прогон дошёл до конца без ошибок", False, soak.error)
    else:
        criterion("прогон дошёл до конца без ошибок", True, "ошибок нет")
    # «не оценено» не проходит: вердикт «пройдено» требует, чтобы каждый критерий был выполнен
    passed = bool(criteria) and all(item["result"] == "выполнено" for item in criteria)
    return {
        "task": "S11-61",
        "metric": "desktop-soak",
        "title": "Длительный прогон приложения: RSS, задержки команд, зависания, нераспознанные сообщения",
        "unit": "KiB (rss), ms (latency), s (duration)",
        "label": LABEL,
        "command": reproduce,
        "fingerprint": fingerprint,
        "conditions": {**conditions_before, **conditions_after},
        "duration_s": round(wall_s, 1),
        "planned_duration_s": soak.duration,
        "time_scale": round(s, 6),
        "intervals_s": {"pause_resume": 300.0 * s, "stop_start": 900.0 * s, "simulated_event_every": 300.0 * s, "rss_and_query": 60.0 * s,
                        "latency_window": 600.0 * s},
        "setup": {"machines": ["soak-a", "soak-b", "soak-app (machine of the application, never started)"], "profile": PROFILE, "memory_mib": MEMORY_MIB,
                  "odctl_create": "odctl create --name N --profile display-test-pattern --memory-mib 256 --library <tmp>; HOME and CFFIXED_USER_HOME are temporary",
                  "session": "one od-vmhost --control-v2 per machine, held by this script (socketpair on fd 3); a stopped machine gets a new process"},
        "rss_kib": rss,
        "latency_ms": latency,
        "counts": {**c, "simulated_host_events_note": "симуляция обработчиков через --simulate-host-event, не сон Mac, не настоящие экраны/место/память",
                   "latency_samples": len(soak.latencies)},
        "simulated_host_events": soak.events_log,
        "verdict": {"passed": passed, "rss_growth_threshold_percent": RSS_GROWTH_LIMIT_PERCENT,
                    "threshold_status": "ГИПОТЕЗА: записана до прогона, не из плана и не из замера; рост сверх порога — «не пройдено»",
                    "criteria": criteria,
                    "not_checked": ["настоящий сон Mac и пробуждение", "реальная смена дисплеев", "настоящее давление памяти и нехватка места",
                                    "окно машины и окно приложения глазами", "обработчики sleep/wake/low-space на работающей машине приложения "
                                    "(приложение без окна не запускает машину; её нет)", "гость (ОС, агент, сеть)"]},
        "notes": ["Профиль test-only без гостя и без диска: RSS и задержки не переносятся на продуктовую VM.",
                  "Условия не контролировались: обычная интерактивная сессия, другие процессы пользователя работали; load average до и после записан.",
                  "Образцов задержек в окнах мало: в окна добавлены проверочные раунды (source = probe), они не относятся к расписанию «каждые 5/15 минут» и посчитаны отдельно."],
    }


class Terminated(BaseException):
    """Raised by the signal handler so that every finally block (shutdown, temporary directory) runs."""


def _on_terminate(signum: int, frame) -> None:  # noqa: ARG001
    raise Terminated(f"signal {signum}")


def find_processes(work: Path) -> tuple[list[str], str | None]:
    """od-vmhost processes whose command line contains the temporary directory.

    pgrep exit 1 means no match; exit 0 lists matches; anything else is an error and is returned as text.
    """
    try:
        done = subprocess.run(["pgrep", "-fl", f"od-vmhost.*{work}"], capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as error:
        return [], f"pgrep failed: {error}"
    if done.returncode == 1:
        return [], None
    if done.returncode == 0:
        return done.stdout.splitlines(), None
    return [], f"pgrep exit {done.returncode}: {(done.stderr or done.stdout).strip()[:200]}"


def main() -> int:
    signal.signal(signal.SIGTERM, _on_terminate)
    signal.signal(signal.SIGHUP, _on_terminate)
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--duration-minutes", type=float, default=60.0, help="length of the run (default 60); a shorter run scales every interval")
    parser.add_argument("--bundle", type=Path, default=DEFAULT_BUNDLE)
    parser.add_argument("--configuration", choices=("debug", "release"), default="release", help="how the bundle was built (recorded, not detected)")
    parser.add_argument("--odctl", type=Path, default=DEFAULT_ODCTL)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--date", default=datetime.date.today().isoformat())
    args = parser.parse_args()
    if args.duration_minutes < 1.0:
        parser.error("--duration-minutes must be at least 1")
    existed = [path.exists() for path in REAL_DIRECTORIES]
    work = Path(tempfile.mkdtemp(prefix="desktop-soak-"))
    reproduce = (f"python3 tools/build/build_app_bundle.py && (cd apps && swift build) && "
                 f"python3 tools/bench/desktop_soak.py --duration-minutes {args.duration_minutes:g} --configuration {args.configuration}")
    started = time.monotonic()
    soak: Soak | None = None
    code = 1
    try:
        check((args.bundle / "Contents" / "Helpers" / "od-vmhost").is_file() and (args.bundle / "Contents" / "MacOS" / "OpenDesktop").is_file(),
              f"no bundle at {args.bundle} (python3 tools/build/build_app_bundle.py)")
        check(args.odctl.is_file(), f"no odctl at {args.odctl} (cd apps && swift build)")
        fingerprint = bench.fingerprint(args.bundle, args.odctl, args.configuration)
        before = bench.conditions(work)
        soak = Soak(args, work)
        try:
            soak.run()
        except FAILURES as failure:
            soak.error = str(failure)
            if "timeout" in soak.error and not soak.error.startswith("hang") and soak.counts["hangs"] == 0:
                soak.counts["hangs"] += 1
            print(f"FAIL: {failure}", file=sys.stderr, flush=True)
        except subprocess.TimeoutExpired as timeout:
            soak.error = f"timeout: {timeout}"
            print(f"FAIL: {soak.error}", file=sys.stderr, flush=True)
        finally:
            soak.shutdown()
        wall = time.monotonic() - started
        orphans, pgrep_error = find_processes(work)
        if pgrep_error:
            orphans.append(pgrep_error)  # an unknown answer is not "no orphans"
        real_ok = all(before_exists or not path.exists() for path, before_exists in zip(REAL_DIRECTORIES, existed))
        after = {"load_average_1_5_15_min_after": [round(x, 2) for x in os.getloadavg()],
                 "power_after": bench.command_output("pmset", "-g", "batt").splitlines()[0] if bench.command_output("pmset", "-g", "batt") else "unknown",
                 "free_space_gib_after": round(shutil.disk_usage(work).free / GIB, 1)}
        if len(soak.app_samples) >= 2 and all(machine.generations and machine.generations[-1].samples for machine in soak.machines):
            result = build_result(soak, args, fingerprint, before, after, reproduce, wall, orphans, real_ok)
            args.out_dir.mkdir(parents=True, exist_ok=True)
            path = args.out_dir / f"s11-61-soak-{args.date}.json"
            text = json.dumps(result, indent=2, ensure_ascii=False) + "\n"
            check(len(text.encode("utf-8")) <= MAX_JSON, f"{path.name}: {len(text.encode('utf-8'))} bytes exceed {MAX_JSON}")
            path.write_text(text, encoding="utf-8")
            print(f"{path} ({path.stat().st_size} bytes)")
            for item in result["verdict"]["criteria"]:
                print(f"  {item['result']:<11} {item['name']}: {item['detail']}")
            print(f"duration {wall:.1f} s; verdict passed = {result['verdict']['passed']}")
            code = 0 if result["verdict"]["passed"] else 1
        else:
            print("FAIL: the run ended before the first RSS samples; no JSON written", file=sys.stderr)
    except FAILURES as failure:
        print(f"FAIL: {failure}", file=sys.stderr)
    except Terminated as stop:
        print(f"FAIL: the run was terminated ({stop}); shutdown ran, no JSON written", file=sys.stderr)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print("ok" if code == 0 else "not passed")
    return code


if __name__ == "__main__":
    sys.exit(main())
