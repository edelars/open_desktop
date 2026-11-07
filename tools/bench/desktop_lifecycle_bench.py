#!/usr/bin/env python3
"""S11-14 measurements of the desktop shell (docs/plans/STAGE_11.md section 5, docs/testing/DESKTOP_APP.md section 3). test-only, без гостя.

What is measured (the machine is the test-only profile `display-test-pattern`, no guest, no disk):

  start-running   `START` -> `STATE_CHANGED running` (ms): from the moment the request is written to the control socket to the moment the
                  event frame is read by the controlling process (this script, the same role as the application); the ACK time is the second series.
  pause-paused    `PAUSE` -> `STATE_CHANGED paused` (ms), the same way, on a machine that has been `running` for 1 s.
  create-raw-64gib  `PREPARE_DISK disk0.img` of 64 GiB on a temporary bundle: request -> ACK (ms), the logical size (`st_size`) and the
                  really allocated size (`st_blocks * 512`); the second series is the whole `odctl create --disk-gib 64` command (ms, includes
                  the launch of od-vmhost). A refusal (`insufficient_space` and the others) is recorded as the result; it is never replaced by a
                  smaller disk. The image is removed after every run.
  rss             `ps -o rss= -p <pid>` (KiB) of od-vmhost after READY, 1 s after `running`, 0.5 s after `paused`; of the application
                  (`OpenDesktop --open <bundle>`, no machine started) 1 s and 4 s after the launch.

Rules (AGENTS.md "Решения в критичных по производительности участках"): `--repeat` runs (default 5) after one warm-up run; the warm-up is
recorded separately and is not part of min/median/max; every series has the unit, min, median, max and the raw values; the fingerprint
(Mac model, macOS build, build mode, Git revision and tree state, tool versions, VMHost profile) and the conditions (power, load, free space)
are written into every file. The output is `docs/reports/data/stage11/s11-14-<metric>-<YYYY-MM-DD>.json` (each at most 100 KiB).
Nothing is created outside a temporary directory except these JSON files. The Mac is not put to sleep.

Usage: python3 tools/bench/desktop_lifecycle_bench.py [--repeat N] [--bundle PATH] [--configuration debug|release]
                                                      [--odctl PATH] [--out-dir DIR] [--date YYYY-MM-DD]
Needs `python3 tools/build/build_app_bundle.py` first (the bundle is `target/app-bundle/OpenDesktop.app`; its default configuration is release)
and `cd apps && swift build` (odctl). Python 3.9+, macOS only.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
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

import vmhost_control_v2_lifecycle_smoke as v2  # noqa: E402  (frame builders, Host, Conn)
import vmhost_control_v2_prepare_disk_smoke as prepare  # noqa: E402  (make_bundle, prepare_payload)

DEFAULT_BUNDLE = ROOT / "target" / "app-bundle" / "OpenDesktop.app"
DEFAULT_ODCTL = ROOT / "apps" / ".build" / "debug" / "odctl"
DEFAULT_OUT = ROOT / "docs" / "reports" / "data" / "stage11"
GIB = 1 << 30
MAX_JSON = 100 * 1024
LABEL = "test-only, без гостя"
PROFILE = "display-test-pattern"
MEMORY_MIB = 256
ERROR_NAMES = {2: "invalid_config", 4: "unsupported_durability", 6: "unsupported_no_agent", 7: "busy", 15: "disk_exists",
               16: "insufficient_space", 20: "invalid_argument", 24: "io_error"}


class Failure(Exception):
    pass


def check(condition: bool, message: str) -> None:
    if not condition:
        raise Failure(message)


def command_output(*command: str, cwd: Path | None = None) -> str:
    try:
        done = subprocess.run(list(command), capture_output=True, text=True, timeout=60, cwd=cwd)
    except (OSError, subprocess.TimeoutExpired) as error:
        return f"unavailable: {error}"
    text = (done.stdout or done.stderr).strip()
    return text if done.returncode == 0 else f"unavailable: exit {done.returncode}: {text[:200]}"


def rss_kib(pid: int) -> int:
    value = command_output("ps", "-o", "rss=", "-p", str(pid))
    check(value.isdigit(), f"ps -o rss= -p {pid}: {value!r}")
    return int(value)


def series(unit: str, values: list, warmup) -> dict:
    """One series: warm-up apart, then raw values and min/median/max of the counted runs."""
    out = {"unit": unit, "warmup": {"value": warmup, "counted": False, "note": "warm-up run, discarded from statistics"},
           "samples": values}
    if values:
        out.update({"min": min(values), "median": statistics.median(values), "max": max(values)})
    return out


def fingerprint(bundle: Path, odctl: Path, configuration: str) -> dict:
    tracked = command_output("git", "status", "--porcelain", "--untracked-files=no", cwd=ROOT)
    untracked = [line for line in command_output("git", "status", "--porcelain", cwd=ROOT).splitlines() if line.startswith("??")]
    return {
        "mac_model": command_output("sysctl", "-n", "hw.model"),
        "macos_build": command_output("sw_vers", "-buildVersion"),
        "build_mode": {"bundle (od-vmhost, OpenDesktop)": f"{configuration} (declared with --configuration; build_app_bundle.py default is release)",
                       "odctl": f"debug ({odctl})"},
        "git_revision": command_output("git", "rev-parse", "--short", "HEAD", cwd=ROOT),
        "git_tracked_changes": 0 if tracked == "" else len(tracked.splitlines()),
        "git_tree_clean_tracked": tracked == "",
        "git_untracked_entries": len(untracked),
        "swift_version": command_output("swift", "--version").splitlines()[0] if command_output("swift", "--version") else "",
        "cargo_version": command_output("cargo", "--version"),
        "vmhost_profile": f"{PROFILE} (control v2 profile 1, memory {MEMORY_MIB} MiB, 1 vCPU, 1 screen), {LABEL}",
        "vmhost": str(bundle / "Contents" / "Helpers" / "od-vmhost"),
        "proposed_not_from_plan": {
            "macos_version": command_output("sw_vers", "-productVersion"),
            "chip": command_output("sysctl", "-n", "machdep.cpu.brand_string"),
            "host_page_size": command_output("sysctl", "-n", "hw.pagesize"),
            "note": "fields of this object are the executor's proposal (DESKTOP_APP.md section 3, rule 2), not required by the plan",
        },
    }


def conditions(path: Path) -> dict:
    usage = shutil.disk_usage(path)
    return {
        "power": command_output("pmset", "-g", "batt").splitlines()[0] if command_output("pmset", "-g", "batt") else "unknown",
        "load_average_1_5_15_min_before": [round(x, 2) for x in os.getloadavg()],
        "free_space_gib_before": round(usage.free / GIB, 1),
        "python": sys.version.split()[0],
        "other_load": "not controlled: an ordinary interactive session, other processes of the user were running",
    }


def write_result(out_dir: Path, name: str, date: str, payload: dict) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / f"s11-14-{name}-{date}.json"
    text = json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=False) + "\n"
    check(len(text.encode("utf-8")) <= MAX_JSON, f"{path.name}: {len(text.encode('utf-8'))} bytes exceed {MAX_JSON}")
    path.write_text(text, encoding="utf-8")
    return path


def timed_request(conn, kind: int, payload: bytes, target_state: int) -> tuple[float, float]:
    """Sends one request; returns (ms to STATE_CHANGED to `target_state`, ms to ACK) from the moment before the write."""
    rid = conn.next_id
    conn.next_id += 1
    frame_bytes = v2.frame(kind, rid, payload)
    event_ms = None
    started = time.perf_counter_ns()
    conn.send(frame_bytes)
    while True:
        item = conn.recv_event()
        arrived = time.perf_counter_ns()
        check(item is not None, f"EOF while waiting for the answer to kind {kind}")
        got_kind, got_rid, body = item
        check(got_rid == rid, f"answer carries request id {got_rid}, expected {rid}")
        if got_kind == v2.STATE_CHANGED:
            _, to, _ = struct.unpack("<III", body)
            if to == target_state and event_ms is None:
                event_ms = (arrived - started) / 1e6
            continue
        check(got_kind == v2.ACK, f"ACK expected for kind {kind}, got {got_kind}: {v2.parse_error(body) if got_kind == v2.ERROR else ''}")
        check(v2.parse_ack(body)["code"] == 0, f"ACK of kind {kind} is not ok")
        check(event_ms is not None, f"no STATE_CHANGED to {target_state} before the ACK of kind {kind}")
        return event_ms, (arrived - started) / 1e6


def lifecycle_run(helper: Path, work: Path) -> dict:
    """One run of START / PAUSE with the RSS of the VMHost; the machine is stopped and the process is waited for at the end."""
    bundle = tempfile.mkdtemp(prefix="bench-lifecycle-", suffix=".odvm", dir=work)
    host = v2.Host(helper, bundle)
    result = {}
    try:
        conn = host.conn
        conn.handshake()
        result["vmhost_rss_after_ready_kib"] = rss_kib(host.pid)
        conn.expect_ok(v2.CONFIGURE, v2.configure_payload(memory=MEMORY_MIB))
        result["start_to_running_ms"], result["start_to_ack_ms"] = timed_request(conn, v2.START, struct.pack("<Q", 0), v2.RUNNING)
        time.sleep(1.0)
        result["vmhost_rss_running_1s_kib"] = rss_kib(host.pid)
        result["pause_to_paused_ms"], result["pause_to_ack_ms"] = timed_request(conn, v2.PAUSE, struct.pack("<II", v2.USER, 0), v2.PAUSED)
        time.sleep(0.5)
        result["vmhost_rss_paused_0_5s_kib"] = rss_kib(host.pid)
        conn.expect_ok(v2.STOP, struct.pack("<Q", 0))
        conn.expect_closed()
        v2.expect_exit(host, 0)
    finally:
        host.cleanup()
        shutil.rmtree(bundle, ignore_errors=True)
    return result


def prepare_run(helper: Path, work: Path, capacity: int) -> dict:
    """One PREPARE_DISK of `capacity` bytes on a fresh temporary bundle; the outcome is recorded either way."""
    root = tempfile.mkdtemp(prefix="bench-raw-", dir=work)
    bundle = prepare.make_bundle(root)
    image = Path(bundle) / "disks" / "disk0.img"
    host = v2.Host(helper, bundle)
    try:
        conn = host.conn
        conn.handshake()
        rid = conn.next_id
        conn.next_id += 1
        frame_bytes = v2.frame(v2.PREPARE_DISK, rid, prepare.prepare_payload(b"disk0.img", capacity))
        progress = 0
        started = time.perf_counter_ns()
        conn.send(frame_bytes)
        while True:
            item = conn.recv_event(600)
            arrived = time.perf_counter_ns()
            check(item is not None, "EOF while waiting for the answer to PREPARE_DISK")
            kind, got_rid, body = item
            check(got_rid == rid, f"answer carries request id {got_rid}, expected {rid}")
            if kind == v2.PROGRESS:
                progress += 1
                continue
            break
        elapsed_ms = (arrived - started) / 1e6
        out: dict = {"progress_frames": progress}
        if kind == v2.ACK:
            ack = v2.parse_ack(body)
            check(ack["code"] == 0, f"ACK of PREPARE_DISK: {ack}")
            info = os.stat(image)
            out.update({"outcome": "ok", "prepare_disk_ms": elapsed_ms, "logical_bytes": info.st_size, "allocated_bytes": info.st_blocks * 512})
            check(info.st_size == capacity, f"image is {info.st_size} bytes, expected {capacity}")
        else:
            check(kind == v2.ERROR, f"unexpected kind {kind}")
            error = v2.parse_error(body)
            out.update({"outcome": ERROR_NAMES.get(error["code"], f"error_{error['code']}"), "error_code": error["code"],
                        "detail": error["detail"], "required_bytes": error["aux0"], "available_bytes": error["aux1"],
                        "refused_after_ms": elapsed_ms})
        conn.sock.shutdown(v2.socket.SHUT_WR)
        conn.expect_closed()
        v2.expect_exit(host, 0)
        return out
    finally:
        host.cleanup()
        shutil.rmtree(root, ignore_errors=True)


def odctl_create_run(odctl: Path, helper: Path, work: Path, capacity_gib: int) -> dict:
    """The whole `odctl create --disk-gib N` command in a temporary library (HOME is temporary too)."""
    library = Path(tempfile.mkdtemp(prefix="bench-odctl-", dir=work))
    home = library / "home"
    home.mkdir()
    env = dict(os.environ, HOME=str(home), CFFIXED_USER_HOME=str(home), ODCTL_VMHOST=str(helper))
    command = [str(odctl), "create", "--name", "bench", "--profile", PROFILE, "--memory-mib", str(MEMORY_MIB),
               "--disk-gib", str(capacity_gib), "--json", "--library", str(library / "lib")]
    try:
        started = time.perf_counter_ns()
        done = subprocess.run(command, env=env, capture_output=True, text=True, timeout=900)
        elapsed_ms = (time.perf_counter_ns() - started) / 1e6
        out: dict = {"exit_code": done.returncode}
        if done.returncode == 0:
            image = library / "lib" / "bench.odvm" / "disks" / "disk0.img"
            info = os.stat(image)
            out.update({"outcome": "ok", "odctl_create_ms": elapsed_ms, "logical_bytes": info.st_size, "allocated_bytes": info.st_blocks * 512})
        else:
            try:
                error = json.loads(done.stdout).get("error", {})
            except ValueError:
                error = {}
            out.update({"outcome": error.get("code", f"exit_{done.returncode}"), "refused_after_ms": elapsed_ms,
                        "message": (error.get("message") or done.stderr or done.stdout)[:300]})
        return out
    finally:
        shutil.rmtree(library, ignore_errors=True)


def app_run(executable: Path, helper: Path, odctl: Path, work: Path) -> dict:
    """RSS of the application 1 s and 4 s after the launch with `--open <bundle>` (no machine is started)."""
    library = Path(tempfile.mkdtemp(prefix="bench-app-", dir=work))
    home = library / "home"
    home.mkdir()
    env = dict(os.environ, HOME=str(home), CFFIXED_USER_HOME=str(home), ODCTL_VMHOST=str(helper))
    create = subprocess.run([str(odctl), "create", "--name", "bench", "--profile", PROFILE, "--memory-mib", str(MEMORY_MIB),
                             "--library", str(library / "lib")], env=env, capture_output=True, text=True, timeout=60)
    check(create.returncode == 0, f"odctl create for the application: {create.stdout} {create.stderr}")
    app = subprocess.Popen([str(executable), "--open", str(library / "lib" / "bench.odvm"), "--vmhost", str(helper)],
                           env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        time.sleep(1.0)
        check(app.poll() is None, "the application ended during the first second")
        first = rss_kib(app.pid)
        time.sleep(3.0)
        check(app.poll() is None, "the application ended before 4 s")
        return {"app_rss_1s_kib": first, "app_rss_4s_kib": rss_kib(app.pid)}
    finally:
        if app.poll() is None:
            app.send_signal(signal.SIGTERM)
            try:
                app.wait(timeout=15)
            except subprocess.TimeoutExpired:
                app.kill()
                app.wait()
        shutil.rmtree(library, ignore_errors=True)


def split(runs: list[dict], key: str) -> tuple:
    """(warm-up value, counted values) of `key` over runs[0] (warm-up) and runs[1:]."""
    return runs[0][key], [run[key] for run in runs[1:]]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--repeat", type=int, default=5, help="counted runs after one warm-up run (at least 5)")
    parser.add_argument("--bundle", type=Path, default=DEFAULT_BUNDLE)
    parser.add_argument("--configuration", choices=("debug", "release"), default="release", help="how the bundle was built (recorded, not detected)")
    parser.add_argument("--odctl", type=Path, default=DEFAULT_ODCTL)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--date", default=datetime.date.today().isoformat())
    parser.add_argument("--raw-gib", type=int, default=64)
    args = parser.parse_args()
    if args.repeat < 5:
        parser.error("--repeat must be at least 5 (plan section 5)")
    helper = args.bundle / "Contents" / "Helpers" / "od-vmhost"
    executable = args.bundle / "Contents" / "MacOS" / "OpenDesktop"
    work = Path(tempfile.mkdtemp(prefix="desktop-lifecycle-bench-"))
    capacity = args.raw_gib * GIB
    reproduce = (f"python3 tools/build/build_app_bundle.py && (cd apps && swift build) && "
                 f"python3 tools/bench/desktop_lifecycle_bench.py --repeat {args.repeat} --configuration {args.configuration}")
    try:
        check(helper.is_file() and executable.is_file(), f"no bundle at {args.bundle} (python3 tools/build/build_app_bundle.py)")
        check(args.odctl.is_file(), f"no odctl at {args.odctl} (cd apps && swift build)")
        fp = fingerprint(args.bundle, args.odctl, args.configuration)
        cond = conditions(work)
        runs_total = args.repeat + 1
        print(f"lifecycle: {runs_total} runs (1 warm-up)", flush=True)
        lifecycle = [lifecycle_run(helper, work) for _ in range(runs_total)]
        print(f"raw disk: {runs_total} runs of PREPARE_DISK {args.raw_gib} GiB", flush=True)
        prepares = [prepare_run(helper, work, capacity) for _ in range(runs_total)]
        print(f"raw disk: {runs_total} runs of odctl create --disk-gib {args.raw_gib}", flush=True)
        creates = [odctl_create_run(args.odctl, helper, work, args.raw_gib) for _ in range(runs_total)]
        print(f"application: {runs_total} runs", flush=True)
        apps = [app_run(executable, helper, args.odctl, work) for _ in range(runs_total)]

        def document(metric: str, title: str, primary: str, all_series: dict, notes: list[str], extra: dict | None = None) -> dict:
            unit, _ = primary
            main_series = all_series[primary[1]]
            out = {"task": "S11-14", "metric": metric, "title": title, "label": LABEL, "unit": unit, "repeats_counted": args.repeat,
                   "warmup_runs": 1, "command": reproduce, "fingerprint": fp, "conditions": cond,
                   "primary_series": primary[1], "warmup": main_series["warmup"], "samples": main_series["samples"]}
            for key in ("min", "median", "max"):
                if key in main_series:
                    out[key] = main_series[key]
            out["series"] = all_series
            out["notes"] = notes
            out.update(extra or {})
            return out

        written = []
        generic = ["Данные профиля test-only не переносятся на продуктовую VM (без гостя, без диска).",
                   "Первый прогон (прогрев) записан в warmup и не входит в min/median/max; входит ли прогрев в пять повторов, ждёт подтверждения владельца.",
                   "Разброс (min..max) нужно читать вместе с медианой; сравнения вариантов в этой задаче нет, поэтому выводов о скорости нет."]
        for metric, title, primary, names, notes in [
            ("start-running", "START -> STATE_CHANGED running", ("ms", "start_to_running_ms"), ["start_to_running_ms", "start_to_ack_ms"],
             ["Отсчёт: перед записью кадра START в control-сокет; конец: приём кадра STATE_CHANGED -> running этим же процессом. Вторая серия: до ACK.",
              "VMHost запущен заново на каждый прогон (posix_spawn, socketpair), CONFIGURE выполнен до замера."]),
            ("pause-paused", "PAUSE -> STATE_CHANGED paused", ("ms", "pause_to_paused_ms"), ["pause_to_paused_ms", "pause_to_ack_ms"],
             ["Отсчёт: перед записью кадра PAUSE; конец: приём STATE_CHANGED -> paused. Машина перед этим 1 с находилась в running."]),
        ]:
            all_series = {name: series("ms", *reversed(split(lifecycle, name))) for name in names}
            written.append(write_result(args.out_dir, metric, args.date, document(metric, title, primary, all_series, notes + generic)))

        # create-raw-64gib: PREPARE_DISK and the whole odctl command; a refusal is a result, not an error of the bench.
        def outcome_block(runs: list[dict]) -> dict:
            names = sorted({run["outcome"] for run in runs[1:]} | {runs[0]["outcome"]})
            return {"warmup": runs[0], "counted": runs[1:], "outcomes": names}

        prepare_ok = all(run["outcome"] == "ok" for run in prepares)
        create_ok = all(run["outcome"] == "ok" for run in creates)
        raw_series = {}
        if prepare_ok:
            raw_series["prepare_disk_ms"] = series("ms", *reversed(split(prepares, "prepare_disk_ms")))
            raw_series["logical_bytes"] = series("bytes", *reversed(split(prepares, "logical_bytes")))
            raw_series["allocated_bytes"] = series("bytes (st_blocks * 512)", *reversed(split(prepares, "allocated_bytes")))
        if create_ok:
            raw_series["odctl_create_ms"] = series("ms", *reversed(split(creates, "odctl_create_ms")))
        raw_notes = [f"Ёмкость: {args.raw_gib} GiB = {capacity} байт. Образ создаётся PREPARE_DISK внутри временного bundle; удаляется после каждого прогона.",
                     "Первая серия: от записи кадра PREPARE_DISK до ACK. Вторая: вся команда `odctl create --disk-gib N` (включая запуск od-vmhost и запись config.plist).",
                     "Отказ (например insufficient_space) записывается как результат в поле prepare_disk_runs / odctl_create_runs и не подменяется меньшим диском."]
        document_raw = document("create-raw-64gib", f"PREPARE_DISK {args.raw_gib} GiB (RAW) и odctl create --disk-gib {args.raw_gib}",
                                ("ms", "prepare_disk_ms" if prepare_ok else "none_refused"),
                                raw_series if raw_series else {"none_refused": series("ms", [], None)},
                                raw_notes + generic,
                                {"prepare_disk_runs": outcome_block(prepares), "odctl_create_runs": outcome_block(creates),
                                 "requested_capacity_bytes": capacity})
        written.append(write_result(args.out_dir, "create-raw-64gib", args.date, document_raw))

        rss_names = ["vmhost_rss_after_ready_kib", "vmhost_rss_running_1s_kib", "vmhost_rss_paused_0_5s_kib"]
        rss_series = {name: series("KiB", *reversed(split(lifecycle, name))) for name in rss_names}
        rss_series.update({name: series("KiB", *reversed(split(apps, name))) for name in ("app_rss_1s_kib", "app_rss_4s_kib")})
        rss_notes = ["`ps -o rss= -p <pid>`, KiB. VMHost: после READY (до CONFIGURE), через 1 с после running, через 0,5 с после paused; "
                     f"конфигурация CONFIGURE: профиль {PROFILE}, {MEMORY_MIB} MiB.",
                     "Приложение: `OpenDesktop --open <bundle>`, машина не запускается, через 1 с и 4 с после запуска; процессов od-vmhost у приложения нет.",
                     "Размер RSS приложения и VMHost с пустым профилем без гостя ничего не говорит о RSS реальной машины."]
        written.append(write_result(args.out_dir, "rss", args.date,
                                    document("rss", "RSS od-vmhost и приложения", ("KiB", "vmhost_rss_running_1s_kib"), rss_series, rss_notes + generic)))
    except Failure as failure:
        print(f"FAIL: {failure}", file=sys.stderr)
        return 1
    except subprocess.TimeoutExpired as timeout:
        print(f"FAIL: timeout: {timeout}", file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(work, ignore_errors=True)
    for path in written:
        print(f"{path.relative_to(ROOT) if path.is_relative_to(ROOT) else path} ({path.stat().st_size} bytes)")
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
