#!/usr/bin/env python3
"""S11-13 smoke: the whole life of a machine through `odctl`, without a window (test-only, no guest).

What it runs, in a temporary library (`--library <tmp>`; the real `~/Library/Application Support/OpenDesktop` and
`~/Documents/Open Desktop` are not read or written: `HOME` of the child is a temporary directory too):

  1. `create --name smoke --profile display-test-pattern --memory-mib 256` (no disk: no VMHost) -> exit 0;
  2. `run smoke --script S` with start, wait running, pause, wait paused, resume, wait running, stop -> exit 0
     (`--json-events` variant: every line is a JSON object with `"schema": 1`, the last one is `result`, `ok: true`);
  3. `inspect smoke --json` and `list --json` parse with `json.loads`, `schema == 1`;
  4. `delete smoke --confirm smoke --files` -> exit 0, the directory is gone; a wrong `--confirm` is exit 2;
  5. `create --name disked --disk-gib 1` creates `disks/disk0.img` of exactly 1 GiB (`os.stat`); `run disked` with `start` -> exit 4 and a
     plain text without the raw code; `delete disked --files` -> exit 4 (the files of a machine with a disk stay), without `--files` it
     only leaves the index;
  6. a second `run` on a bundle that the first `run` holds (lock) -> exit 3 (`busy`), the first one is not disturbed (exit 0);
  7. a mistake in the script is exit 2 and starts no VMHost; the end of the commands leaves no `od-vmhost` of this bundle behind.

Usage: python3 tests/system/odctl_lifecycle_smoke.py [ODCTL_BINARY [VMHOST_BINARY]]
Default: apps/.build/debug/odctl and the `od-vmhost` next to it (`cd apps && swift build`; the staticlib first:
`cargo build -p od-host --offline`). Prints `ok` and exits 0 on success.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ODCTL = ROOT / "apps" / ".build" / "debug" / "odctl"
TIMEOUT = 60

EXIT_OK, EXIT_FAILURE, EXIT_USAGE, EXIT_BUSY, EXIT_DURABILITY = 0, 1, 2, 3, 4


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


class Odctl:
    def __init__(self, binary: Path, vmhost: Path | None, library: Path, home: Path) -> None:
        self.binary = binary
        self.library = library
        self.env = dict(os.environ, HOME=str(home))
        if vmhost is not None:
            self.env["ODCTL_VMHOST"] = str(vmhost)

    def argv(self, *args: str) -> list[str]:
        return [str(self.binary), *args, "--library", str(self.library)]

    def run(self, *args: str, expect: int, timeout: float = TIMEOUT) -> subprocess.CompletedProcess:
        done = subprocess.run(self.argv(*args), env=self.env, capture_output=True, text=True, timeout=timeout)
        check(done.returncode == expect,
              f"odctl {' '.join(args)}: exit {done.returncode}, expected {expect}\nstdout: {done.stdout}\nstderr: {done.stderr}")
        return done

    def popen(self, *args: str) -> subprocess.Popen:
        return subprocess.Popen(self.argv(*args), env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def parse_json(text: str, what: str) -> dict:
    try:
        value = json.loads(text)
    except ValueError as error:
        raise Failure(f"{what}: not JSON ({error}): {text!r}") from error
    check(isinstance(value, dict) and value.get("schema") == 1, f"{what}: no \"schema\": 1 in {text!r}")
    return value


def parse_events(text: str, what: str) -> list[dict]:
    events = [parse_json(line, what) for line in text.splitlines() if line.strip()]
    check(bool(events), f"{what}: no events")
    return events


def write_script(directory: Path, name: str, lines: list[str]) -> Path:
    path = directory / name
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def vmhost_alive(bundle: Path) -> bool:
    """Whether an `od-vmhost` of this bundle is still running (it must not outlive `odctl`)."""
    done = subprocess.run(["pgrep", "-f", f"od-vmhost.*--bundle {bundle}"], capture_output=True, text=True)
    return done.returncode == 0


def scenario(binary: Path, vmhost: Path | None, work: Path) -> None:
    library = work / "library"
    home = work / "home"
    home.mkdir()
    cli = Odctl(binary, vmhost, library, home)
    scripts = work / "scripts"
    scripts.mkdir()

    # 1. create without a disk (no VMHost).
    done = cli.run("create", "--name", "smoke", "--profile", "display-test-pattern", "--memory-mib", "256", expect=EXIT_OK)
    smoke = library / "smoke.odvm"
    check((smoke / "config.plist").is_file(), "create: config.plist is missing")
    check((smoke / "lock").is_file(), "create: lock is missing")
    check(not (smoke / "disks" / "disk0.img").exists(), "create without a disk made an image")
    check("test-only" in done.stdout, "create: the output does not say test-only")

    # 2. the lifecycle.
    lifecycle = write_script(scripts, "lifecycle.txt", [
        "# S11-13: start, pause, resume, stop",
        "start", "wait running", "pause", "wait paused 5000", "resume", "wait running", "query", "stop",
    ])
    cli.run("run", "smoke", "--script", str(lifecycle), expect=EXIT_OK)
    check(not vmhost_alive(smoke), "run: od-vmhost outlived odctl")
    done = cli.run("run", "smoke", "--script", str(lifecycle), "--json-events", expect=EXIT_OK)
    events = parse_events(done.stdout, "run --json-events")
    check(events[-1].get("event") == "result" and events[-1].get("ok") is True and events[-1].get("exit_code") == 0,
          f"run --json-events: last event is not a successful result: {events[-1]}")
    transitions = [(e["from"], e["to"]) for e in events if e.get("event") == "state_changed"]
    check(transitions == [("stopped", "running"), ("running", "paused"), ("paused", "running"), ("running", "stopped")],
          f"run --json-events: transitions {transitions}")
    check(any(e.get("event") == "exited" and e.get("status") == 0 for e in events), "run --json-events: no exited(0)")
    # A script without `stop` is ended by odctl itself (the process ends together with the command).
    cli.run("run", "smoke", "--script", str(write_script(scripts, "no_stop.txt", ["start", "wait running"])), expect=EXIT_OK)
    check(not vmhost_alive(smoke), "run without stop: od-vmhost outlived odctl")

    # 3. inspect and list.
    inspect = parse_json(cli.run("inspect", "smoke", "--json", expect=EXIT_OK).stdout, "inspect --json")
    machine = inspect["machine"]
    check(inspect["ok"] is True and machine["name"] == "smoke" and machine["status"] == "valid", f"inspect: {machine}")
    check(machine["profile"] == "display-test-pattern" and machine["memory_mib"] == 256 and machine["lock"] == "free", f"inspect: {machine}")
    listing = parse_json(cli.run("list", "--json", expect=EXIT_OK).stdout, "list --json")
    check([m["name"] for m in listing["machines"]] == ["smoke"], f"list: {listing['machines']}")
    check("smoke" in cli.run("list", expect=EXIT_OK).stdout, "list (text): no machine")
    by_path = parse_json(subprocess.run([str(binary), "inspect", str(smoke), "--json"], env=cli.env, capture_output=True, text=True,
                                        timeout=TIMEOUT).stdout, "inspect <path> --json")
    check(by_path["machine"]["name"] == "smoke", "inspect by path")

    # usage failures: exit 2, a message, no process.
    bad = write_script(scripts, "bad.txt", ["start", "wait flying", "stop"])
    done = cli.run("run", "smoke", "--script", str(bad), expect=EXIT_USAGE)
    check("строка 2" in done.stderr, f"a mistake in the script does not name the line: {done.stderr!r}")
    done = cli.run("run", "smoke", "--script", str(write_script(scripts, "after_stop.txt", ["stop", "start"])), expect=EXIT_USAGE)
    check("строка 2" in done.stderr, "a command after stop is accepted")
    cli.run("create", "--name", "x", "--profile", "no-such-profile", "--memory-mib", "256", expect=EXIT_USAGE)
    cli.run("create", "--name", "x", "--profile", "display-test-pattern", "--memory-mib", "16", expect=EXIT_USAGE)
    cli.run("delete", "smoke", "--confirm", "other", "--files", expect=EXIT_USAGE)
    check(smoke.is_dir(), "delete with a wrong --confirm removed the directory")
    failure = parse_json(cli.run("delete", "smoke", "--confirm", "other", "--json", expect=EXIT_USAGE).stdout, "delete --json (usage)")
    check(failure["ok"] is False and failure["error"]["exit_code"] == EXIT_USAGE and failure["error"]["code"] == "usage", f"{failure}")
    cli.run("create", "--name", "smoke", "--profile", "display-test-pattern", "--memory-mib", "256", expect=EXIT_FAILURE)  # name is taken
    check((smoke / "config.plist").is_file(), "create with a taken name touched the existing machine")

    # 4. delete with files.
    done = cli.run("delete", "smoke", "--confirm", "smoke", "--files", expect=EXIT_OK)
    check(not smoke.exists(), "delete --files: the directory is still there")
    check(parse_json(cli.run("list", "--json", expect=EXIT_OK).stdout, "list --json")["machines"] == [], "list after delete")

    # 5. a machine with a disk: the image exists, the machine does not start (ISSUE-055).
    done = cli.run("create", "--name", "disked", "--profile", "display-test-pattern", "--memory-mib", "256", "--disk-gib", "1", "--json",
                   expect=EXIT_OK, timeout=120)
    created = parse_json(done.stdout, "create --json")
    check(created["ok"] is True and created["vmhost_started"] is True and created["disk_bytes"] == 1 << 30, f"create --disk-gib: {created}")
    disked = library / "disked.odvm"
    image = disked / "disks" / "disk0.img"
    check(image.is_file() and os.stat(image).st_size == 1 << 30, "create --disk-gib 1: disks/disk0.img is not 1 GiB")
    check(not vmhost_alive(disked), "create: od-vmhost outlived odctl")
    start_only = write_script(scripts, "start_only.txt", ["start"])
    done = cli.run("run", "disked", "--script", str(start_only), expect=EXIT_DURABILITY)
    check("unsupported_durability" not in done.stderr and "Работа с дисками" in done.stderr, f"exit 4 text: {done.stderr!r}")
    done = cli.run("run", "disked", "--script", str(start_only), "--json-events", expect=EXIT_DURABILITY)
    result = parse_events(done.stdout, "run --json-events (exit 4)")[-1]
    check(result["event"] == "result" and result["ok"] is False and result["error"]["code"] == "unsupported_durability", f"{result}")
    check(not vmhost_alive(disked), "run with start on a machine with a disk started a VMHost")
    # `stop` of such a machine is `unsupported_durability` in every state: refused before the launch as well.
    cli.run("run", "disked", "--script", str(write_script(scripts, "stop_only.txt", ["stop"])), expect=EXIT_DURABILITY)
    # Without `start` the machine is not started and not stopped by a request: the script ends by closing the connection (status 0).
    query_only = write_script(scripts, "query_only.txt", ["query", "sleep 100", "query"])
    done = cli.run("run", "disked", "--script", str(query_only), expect=EXIT_OK)
    check(not vmhost_alive(disked), "run without start on a machine with a disk: od-vmhost outlived odctl")
    done = cli.run("run", "disked", "--script", str(query_only), "--json-events", expect=EXIT_OK)
    events = parse_events(done.stdout, "run --json-events (disk, no start)")
    check(events[-1].get("event") == "result" and events[-1].get("ok") is True and events[-1].get("exit_code") == 0, f"{events[-1]}")
    check(any(e.get("event") == "exited" and e.get("status") == 0 for e in events), "run (disk, no start): no exited(0)")
    check(any(e.get("event") == "command" and e.get("disks_non_empty") is True for e in events), "query: disks_non_empty is not set")
    check(not any(e.get("event") == "state_changed" for e in events), "run (disk, no start): the machine changed state")
    # A refusal of the VMHost on a command (`pause` in `stopped`) ends the script with exit 1 and the connection is closed, not terminated.
    cli.run("run", "disked", "--script", str(write_script(scripts, "pause_only.txt", ["pause"])), expect=EXIT_FAILURE)
    check(not vmhost_alive(disked), "run (disk, refused pause): od-vmhost outlived odctl")
    done = cli.run("inspect", "disked", "--json", expect=EXIT_OK)
    check(parse_json(done.stdout, "inspect disked")["machine"]["disks"][0]["size_bytes"] == 1 << 30, "inspect: size of the image")
    cli.run("delete", "disked", "--confirm", "disked", "--files", expect=EXIT_DURABILITY)
    check(image.is_file(), "delete --files removed the image of a machine with a disk")
    # The configuration cannot be read (not a plist): `disks/` is looked at all the same; nothing is deleted, exit 4.
    config_plist = disked / "config.plist"
    saved_config = config_plist.read_bytes()
    config_plist.write_bytes(b"not a plist")
    done = cli.run("delete", str(disked), "--confirm", "disked", "--files", expect=EXIT_DURABILITY)
    check("Файлы машины с дисками не удаляются" in done.stderr, f"exit 4 text of delete: {done.stderr!r}")
    check(image.is_file() and config_plist.is_file(), "delete --files removed files when config.plist could not be read")
    config_plist.write_bytes(saved_config)
    # A path that does not exist: the words speak about the path, not about the list.
    done = cli.run("delete", str(library / "elsewhere.odvm"), "--confirm", "elsewhere", expect=EXIT_FAILURE)
    check("По указанному пути" in done.stderr and "списк" not in done.stderr, f"missing path text: {done.stderr!r}")
    cli.run("delete", "disked", "--confirm", "disked", expect=EXIT_OK)  # the index only
    check(image.is_file(), "delete without --files removed files")
    check(parse_json(cli.run("list", "--json", expect=EXIT_OK).stdout, "list --json")["machines"] == [], "the machine is still listed")
    # The machine is out of the index now: by its path nothing changes, and the words say so (not "убрана из списка").
    done = cli.run("delete", str(disked), "--confirm", "disked", expect=EXIT_OK)
    check("ничего не изменено" in done.stdout and "убрана" not in done.stdout, f"delete of an unlisted machine: {done.stdout!r}")
    check(image.is_file(), "delete of an unlisted machine removed files")

    # 6. busy: a second run on a bundle that another run holds.
    cli.run("create", "--name", "busy", "--profile", "display-test-pattern", "--memory-mib", "256", expect=EXIT_OK)
    holder_script = write_script(scripts, "holder.txt", ["start", "wait running", "sleep 4000", "stop"])
    holder = cli.popen("run", "busy", "--script", str(holder_script), "--json-events")
    try:
        deadline = time.monotonic() + TIMEOUT
        running = False
        while not running:
            check(time.monotonic() < deadline, "the first run did not reach running")
            line = holder.stdout.readline()
            if line == "":  # the message reads stderr to its end: only when the run has really ended
                raise Failure(f"the first run ended early: {holder.stderr.read()}")
            event = parse_json(line, "first run event")
            running = event.get("event") == "state_changed" and event.get("to") == "running"
        check(vmhost_alive(library / "busy.odvm"), "od-vmhost of a running machine is not seen by pgrep (the absence checks would pass idly)")
        second = cli.run("run", "busy", "--script", str(write_script(scripts, "second.txt", ["start", "stop"])), expect=EXIT_BUSY)
        check("busy" not in second.stderr and "другим процессом" in second.stderr, f"exit 3 text: {second.stderr!r}")
        done = cli.run("run", "busy", "--script", str(holder_script), "--json-events", expect=EXIT_BUSY)
        result = parse_events(done.stdout, "run --json-events (busy)")[-1]
        check(result["error"]["code"] == "busy" and result["error"]["exit_code"] == EXIT_BUSY, f"{result}")
        done = cli.run("delete", "busy", "--confirm", "busy", "--files", expect=EXIT_BUSY)  # only a stopped machine is deleted
        check((library / "busy.odvm").is_dir(), "delete removed a machine that runs")
        rest, errors = holder.communicate(timeout=TIMEOUT)
        check(holder.returncode == EXIT_OK, f"the first run was disturbed: exit {holder.returncode}\n{rest}\n{errors}")
    finally:
        if holder.poll() is None:
            holder.kill()
            holder.wait()
    check(not vmhost_alive(library / "busy.odvm"), "od-vmhost outlived odctl (busy scenario)")

    # 7. SIGTERM to a run that holds a running machine: exit 1, the last line is `result`, no VMHost stays.
    long_script = write_script(scripts, "long.txt", ["start", "wait running", "sleep 30000", "stop"])
    victim = cli.popen("run", "busy", "--script", str(long_script), "--json-events")
    try:
        deadline = time.monotonic() + TIMEOUT
        running = False
        while not running:
            check(time.monotonic() < deadline, "the run for SIGTERM did not reach running")
            line = victim.stdout.readline()
            if line == "":
                raise Failure(f"the run for SIGTERM ended early: {victim.stderr.read()}")
            event = parse_json(line, "run event")
            running = event.get("event") == "state_changed" and event.get("to") == "running"
        victim.terminate()
        rest, errors = victim.communicate(timeout=TIMEOUT)
        check(victim.returncode == EXIT_FAILURE, f"SIGTERM: exit {victim.returncode}\n{rest}\n{errors}")
        last = parse_events(rest, "run --json-events (SIGTERM)")[-1]
        check(last.get("event") == "result" and last.get("ok") is False and last.get("exit_code") == EXIT_FAILURE, f"SIGTERM: last line {last}")
    finally:
        if victim.poll() is None:
            victim.kill()
            victim.wait()
    check(not vmhost_alive(library / "busy.odvm"), "od-vmhost outlived odctl (SIGTERM scenario)")
    cli.run("delete", "busy", "--confirm", "busy", "--files", expect=EXIT_OK)
    check(not (library / "busy.odvm").exists(), "the stopped machine was not deleted")


def main() -> int:
    binary = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_ODCTL
    vmhost = Path(sys.argv[2]) if len(sys.argv) > 2 else binary.parent / "od-vmhost"
    if not binary.is_file() or not vmhost.is_file():
        print(f"build first: cd apps && swift build ({binary}, {vmhost})", file=sys.stderr)
        return 1
    work = Path(tempfile.mkdtemp(prefix="odctl-smoke-"))
    try:
        scenario(binary, vmhost, work)
    except Failure as failure:
        print(f"FAIL: {failure}", file=sys.stderr)
        return 1
    except subprocess.TimeoutExpired as timeout:
        print(f"FAIL: timeout: {timeout}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
