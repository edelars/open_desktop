#!/usr/bin/env python3
"""S11-14 smoke: the whole shell, end to end, without a guest (test-only, без гостя). Level 3 of docs/plans/STAGE_11.md section 5.

One pass of the scenario (repeated `--repeat` times, default 5, every pass in its own temporary library and its own temporary home;
the real `~/Library/Application Support/OpenDesktop` and `~/Documents/Open Desktop` are neither read nor written: `HOME` and
`CFFIXED_USER_HOME` of every child are the temporary directory, and the script fails if one of the two real directories appeared):

  a. (once, before the passes) `tools/build/build_app_bundle.py` -> `OpenDesktop.app` with `Contents/Helpers/od-vmhost`
     (`--skip-build` reuses a bundle that is already built: `--bundle PATH`, default `target/app-bundle/OpenDesktop.app`);
  b. `odctl create --name e2e --profile display-test-pattern --memory-mib 256 --library <tmp>` -> exit 0, `config.plist` and `lock` exist;
  c. `odctl run e2e --library <tmp> --script <tmp>/s.txt --json-events` with start, wait running, pause, wait paused, resume,
     wait running, stop -> exit 0, the events are stopped->running, running->paused, paused->running, running->stopped and `exited(0)`;
     no `od-vmhost` of the bundle outlives `odctl`;
  d. `OpenDesktop --open <tmp>/e2e.odvm --vmhost <bundle>/Contents/Helpers/od-vmhost` in the background for `--hold` seconds (default 4):
     the process is alive, stderr is empty, a window of the process exists when the window server answers (otherwise the pass says
     "window not checked": the script does not guess); then SIGTERM, the process ends, no `od-vmhost` of the bundle is left;
  e. `odctl delete e2e --library <tmp> --confirm e2e --files` -> exit 0, the directory is gone, the list is empty.

The script does not put the Mac to sleep and does not use `--simulate-host-event`: that is a handler test, not a proof of the OS behaviour.
The application is only opened here; it does not start the machine, so `logs/` of the bundle has no `vmhost.log` (no such file exists in the product).

Usage: python3 tests/system/desktop_lifecycle_smoke.py [--repeat N] [--skip-build] [--bundle PATH] [--odctl PATH] [--hold SECONDS]
Prints `ok` (and a line per pass) and exits 0 on success. Python 3.9+, macOS only.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ODCTL = ROOT / "apps" / ".build" / "debug" / "odctl"
DEFAULT_BUNDLE = ROOT / "target" / "app-bundle" / "OpenDesktop.app"
BUILD_SCRIPT = ROOT / "tools" / "build" / "build_app_bundle.py"
TIMEOUT = 60

REAL_DIRECTORIES = [Path.home() / "Library" / "Application Support" / "OpenDesktop", Path.home() / "Documents" / "Open Desktop"]

WINDOW_SCRIPT = """
ObjC.import('CoreGraphics');
// all windows (option 0): a window of an application that was not activated can be listed but not on screen
var list = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(0, 0)));
JSON.stringify(list.map(function (w) { return [w.kCGWindowOwnerPID, w.kCGWindowLayer, w.kCGWindowIsOnscreen]; }));
"""


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def vmhost_alive(pattern: str) -> bool:
    """Whether an `od-vmhost` whose command line matches `pattern` is running (the pattern holds the temporary bundle path)."""
    return subprocess.run(["pgrep", "-f", pattern], capture_output=True, text=True).returncode == 0


def windows_of(pid: int) -> tuple[bool, int, int]:
    """(window server answered, windows of `pid` at layer 0, of them on screen) through JXA + CGWindowListCopyWindowInfo; (False, 0, 0) when it cannot say."""
    try:
        done = subprocess.run(["osascript", "-l", "JavaScript", "-e", WINDOW_SCRIPT], capture_output=True, text=True, timeout=20)
        if done.returncode != 0:
            return False, 0, 0
        entries = json.loads(done.stdout)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return False, 0, 0
    if not entries:
        return False, 0, 0
    own = [onscreen for owner, layer, onscreen in entries if owner == pid and layer == 0]
    return True, len(own), sum(1 for onscreen in own if onscreen)


class Pass:
    def __init__(self, index: int, odctl: Path, bundle: Path, hold: float) -> None:
        self.index = index
        self.odctl = odctl
        self.bundle = bundle
        self.hold = hold
        self.work = Path(tempfile.mkdtemp(prefix="desktop-lifecycle-"))
        self.home = self.work / "home"
        self.home.mkdir()
        self.library = self.work / "library"
        self.machine = self.library / "e2e.odvm"
        self.helper = bundle / "Contents" / "Helpers" / "od-vmhost"
        self.executable = bundle / "Contents" / "MacOS" / "OpenDesktop"
        self.env = dict(os.environ, HOME=str(self.home), CFFIXED_USER_HOME=str(self.home), ODCTL_VMHOST=str(self.helper))
        self.window = "window not checked"

    def odctl_run(self, *args: str, expect: int = 0) -> subprocess.CompletedProcess:
        command = [str(self.odctl), *args, "--library", str(self.library)]
        done = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=TIMEOUT)
        check(done.returncode == expect,
              f"odctl {' '.join(args)}: exit {done.returncode}, expected {expect}\nstdout: {done.stdout}\nstderr: {done.stderr}")
        return done

    def no_vmhost(self, what: str) -> None:
        check(not vmhost_alive(f"od-vmhost.*--bundle {self.machine}"), f"{what}: od-vmhost of the bundle is still running")

    def create(self) -> None:
        done = self.odctl_run("create", "--name", "e2e", "--profile", "display-test-pattern", "--memory-mib", "256")
        check((self.machine / "config.plist").is_file(), "create: config.plist is missing")
        check((self.machine / "lock").is_file(), "create: lock is missing")
        check("test-only" in done.stdout, f"create: the output does not say test-only: {done.stdout!r}")
        self.no_vmhost("create")

    def run_script(self) -> None:
        script = self.work / "s.txt"
        script.write_text("start\nwait running\npause\nwait paused\nresume\nwait running\nstop\n", encoding="utf-8")
        done = self.odctl_run("run", "e2e", "--script", str(script), "--json-events")
        try:
            events = [json.loads(line) for line in done.stdout.splitlines() if line.strip()]
        except ValueError as error:
            raise Failure(f"run --json-events: not JSON ({error}): {done.stdout!r}") from error
        check(bool(events) and all(e.get("schema") == 1 for e in events), "run --json-events: no events or no schema 1")
        last = events[-1]
        check(last.get("event") == "result" and last.get("ok") is True and last.get("exit_code") == 0, f"run: last event {last}")
        transitions = [(e["from"], e["to"]) for e in events if e.get("event") == "state_changed"]
        check(transitions == [("stopped", "running"), ("running", "paused"), ("paused", "running"), ("running", "stopped")],
              f"run: transitions {transitions}")
        check(any(e.get("event") == "exited" and e.get("status") == 0 for e in events), "run: no exited(0)")
        self.no_vmhost("run")

    def open_app(self) -> None:
        stderr_path = self.work / "app.stderr"
        with open(stderr_path, "wb") as stderr:
            app = subprocess.Popen(
                [str(self.executable), "--open", str(self.machine), "--vmhost", str(self.helper)],
                env=self.env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=stderr)
        try:
            time.sleep(self.hold)
            check(app.poll() is None, f"the application ended by itself with status {app.returncode} before {self.hold} s")
            answered, count, on_screen = windows_of(app.pid)
            if answered:
                check(count >= 1, "the window server answered, but the application has no window")
                self.window = f"window of the process: {count} (on screen: {on_screen}; not activated apps may be off screen)"
            check(app.poll() is None, "the application ended while its windows were checked")
        finally:
            if app.poll() is None:
                app.send_signal(signal.SIGTERM)
                try:
                    app.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait()
                    raise Failure("the application did not end on SIGTERM within 15 s (killed)")
        check(app.returncode in (0, -signal.SIGTERM), f"the application ended with status {app.returncode} after SIGTERM")
        errors = stderr_path.read_text(encoding="utf-8", errors="replace").strip()
        check(errors == "", f"the application wrote to stderr: {errors[:500]!r}")
        check(not vmhost_alive(f"od-vmhost.*--bundle {self.machine}") and not vmhost_alive(f"{self.helper}"), "app: od-vmhost is left behind")
        check(self.machine.is_dir(), "app: the application removed the machine directory")

    def delete(self) -> None:
        self.odctl_run("delete", "e2e", "--confirm", "e2e", "--files")
        check(not self.machine.exists(), "delete --files: the directory is still there")
        done = self.odctl_run("list", "--json")
        check(json.loads(done.stdout)["machines"] == [], "delete: the machine is still listed")

    def execute(self) -> str:
        try:
            self.create()
            self.run_script()
            self.open_app()
            self.delete()
        finally:
            shutil.rmtree(self.work, ignore_errors=True)
        return self.window


def build_bundle() -> Path:
    done = subprocess.run([sys.executable, str(BUILD_SCRIPT)], capture_output=True, text=True, timeout=1800)
    check(done.returncode == 0, f"build_app_bundle.py: exit {done.returncode}\n{done.stdout}\n{done.stderr}")
    lines = [line for line in done.stdout.splitlines() if line.startswith("app_bundle_path=")]
    check(len(lines) == 1, f"build_app_bundle.py: no app_bundle_path= line: {done.stdout!r}")
    return Path(lines[0].split("=", 1)[1])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--skip-build", action="store_true", help="reuse the bundle given with --bundle")
    parser.add_argument("--bundle", type=Path, default=DEFAULT_BUNDLE)
    parser.add_argument("--odctl", type=Path, default=DEFAULT_ODCTL)
    parser.add_argument("--hold", type=float, default=4.0, help="seconds the application stays up before the checks")
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error("--repeat must be at least 1")
    existed = [path.exists() for path in REAL_DIRECTORIES]
    try:
        bundle = args.bundle if args.skip_build else build_bundle()
        check((bundle / "Contents" / "MacOS" / "OpenDesktop").is_file(), f"no application in {bundle} (run without --skip-build)")
        check((bundle / "Contents" / "Helpers" / "od-vmhost").is_file(), f"no od-vmhost in {bundle}")
        check(args.odctl.is_file(), f"no odctl at {args.odctl} (cd apps && swift build)")
        for index in range(1, args.repeat + 1):
            started = time.monotonic()
            window = Pass(index, args.odctl, bundle, args.hold).execute()
            print(f"ok   pass {index}/{args.repeat} ({time.monotonic() - started:.1f} s, {window})", flush=True)
        for path, before in zip(REAL_DIRECTORIES, existed):
            check(before or not path.exists(), f"{path} appeared: the scenario touched the real user directories")
    except Failure as failure:
        print(f"FAIL: {failure}", file=sys.stderr)
        return 1
    except subprocess.TimeoutExpired as timeout:
        print(f"FAIL: timeout: {timeout}", file=sys.stderr)
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
