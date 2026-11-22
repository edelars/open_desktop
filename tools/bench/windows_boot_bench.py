#!/usr/bin/env python3
"""Measure interpreter throughput on the longest available firmware boot path (S8-04).

The Windows media is not on the boot path yet (S7-32 is not accepted), so the
path is the longest existing firmware boot of the stage 7 bench fixtures: the
firmware-volume route (FV) and the NVMe ESP route (ESP) of
tools/bench/firmware_boot_bench.py, each run to the first firmware stop.

For every route the script does one discovery run (RIP addresses), one
discarded warmup and at least five measured runs. Every run is a separate
od-runner process. With --require-nice the script requires an effective nice
value of -5 and exits with an error otherwise; without it nice is left as it is
and the result is marked satisfied=false. The effective value is read back with
ps and written to the JSON. The script never invents numbers: any failed run
aborts it.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import math
import os
import re
import resource
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/bench"))
sys.path.insert(0, str(ROOT / "tools/build"))

import firmware_boot_bench as fb  # fixtures of S7-27: ROM, EFI app, FAT32 ESP disk

MAX_JSON = 100 * 1024
TARGET_NICE = -5
DEFAULT_BUDGET = 1_200_000_000
DAY_SECONDS = 24 * 3600
ENGINE_SOURCES = (
    "crates/od-machine/src/firmware_board.rs",
    "crates/od-machine/src/paged_probe.rs",
    "tools/runner/src/firmware_boot.rs",
    "crates/od-isa/Cargo.toml",
)
# od-jit is linked into the runner here and used by these files (not by the firmware board).
JIT_LINK_MANIFESTS = ("crates/od-machine/Cargo.toml", "tools/runner/Cargo.toml")
JIT_USERS = ("crates/od-machine/src/diagnostic_vcpu.rs", "tools/runner/src/main.rs")
BENCH_SOURCES = ENGINE_SOURCES + ("tools/bench/windows_boot_bench.py", "tools/bench/firmware_boot_bench.py")
# Hypothesis, not a measurement: see the basis in the JSON and in the report.
SCALE_HYPOTHESIS = (1e12, 1e13)


def command(argv, **kwargs):
    return subprocess.check_output(argv, cwd=ROOT, text=True, **kwargs).strip()


def sha256_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def summarize(values):
    ordered = sorted(values)
    return {
        "n": len(ordered),
        "mean": round(statistics.fmean(ordered), 3),
        "p50": statistics.median(ordered),
        "p95": ordered[math.ceil(0.95 * len(ordered)) - 1],  # nearest rank
        "min": ordered[0],
        "max": ordered[-1],
        "spread": ordered[-1] - ordered[0],
        "stdev": round(statistics.stdev(ordered), 3) if len(ordered) > 1 else 0.0,
    }


def nice_wrapper():
    """Argv prefix that gives the child an effective nice of TARGET_NICE, or []."""
    current = os.nice(0)
    if current == TARGET_NICE:
        return [], current
    return ["nice", "-n", str(TARGET_NICE - current)], current


def read_nice(pid):
    out = subprocess.run(["ps", "-o", "ni=", "-p", str(pid)], capture_output=True, text=True)
    text = out.stdout.strip()
    return int(text) if re.fullmatch(r"-?\d+", text) else None


def engine_facts():
    """Which execution engine the firmware boot path calls, by a text search of the sources.

    This is a text search, not proof: it cannot show that no call to od-jit exists.
    """
    facts = {"jit_mentions": {}, "interpreter_call": None, "firmware_path_mentions_jit": None,
             "od_jit_linked_in_runner": None}
    for rel in ENGINE_SOURCES:
        text = (ROOT / rel).read_text(encoding="utf-8")
        facts["jit_mentions"][rel] = len(re.findall("jit", text, re.IGNORECASE))
    board_dir = ROOT / "crates/od-machine/src/firmware_board"
    facts["jit_mentions"]["crates/od-machine/src/firmware_board/*.rs"] = sum(
        len(re.findall("jit", path.read_text(encoding="utf-8"), re.IGNORECASE))
        for path in sorted(board_dir.glob("*.rs")))
    facts["firmware_path_mentions_jit"] = any(facts["jit_mentions"].values())
    probe = (ROOT / "crates/od-machine/src/paged_probe.rs").read_text(encoding="utf-8").splitlines()
    for number, line in enumerate(probe, 1):
        if "step_paged_firmware_probe_with_guest_fp(" in line and "use" not in line:
            facts["interpreter_call"] = f"crates/od-machine/src/paged_probe.rs:{number}"
            break
    linked = {rel: "od-jit" in (ROOT / rel).read_text(encoding="utf-8") for rel in JIT_LINK_MANIFESTS}
    used = {rel: bool(re.search("od_jit", (ROOT / rel).read_text(encoding="utf-8"))) for rel in JIT_USERS}
    facts["od_jit_linked_in_runner"] = all(linked.values())
    if facts["firmware_path_mentions_jit"]:
        path_note = "a jit mention WAS found in the firmware board paths (see jit_mentions); the engine is undetermined"
    else:
        path_note = "no od-jit mention was found in the firmware board paths (text search; not a proof of no call)"
    facts["od_jit_evidence"] = {"cargo_manifests": linked, "used_in_sources": used,
                                "note": "od-jit is a dependency of od-machine and od-runner and is used by the "
                                        "diagnostic vCPU and the runner main; " + path_note}
    facts["engine"] = ("interpreter (od_isa::execute::step_paged_firmware_probe_with_guest_fp)"
                       if facts["interpreter_call"] and not facts["firmware_path_mentions_jit"]
                       else "UNDETERMINED: inspect the sources")
    facts["engine_basis"] = ("text search of the firmware board execution paths for 'jit' "
                             "(firmware_board.rs, firmware_board/*.rs, paged_probe.rs, firmware_boot.rs, "
                             "od-isa Cargo.toml); not a proof that no call to od-jit exists")
    return facts


def vcpu_support():
    """boot-firmware takes no vCPU option; the board default is one vCPU."""
    text = (ROOT / "tools/runner/src/firmware_boot.rs").read_text(encoding="utf-8")
    has_option = bool(re.search(r'"--(vcpus?|smp|cpus)"', text))
    return {"runner_has_vcpu_option": has_option, "supported_counts": [1],
            "two_vcpu": "not measured: boot-firmware has no vCPU option, the board default is vcpu_count = 1 "
                        "and S6-51 (two vCPUs for stages 7, 8, 11) is not accepted; no code was changed"}


def host_fingerprint():
    def sysctl(name):
        return command(["sysctl", "-n", name])

    return {
        "model": sysctl("hw.model"),
        "cpu": sysctl("machdep.cpu.brand_string"),
        "logical_cpus": int(sysctl("hw.ncpu")),
        "memory_bytes": int(sysctl("hw.memsize")),
        "page_size": int(sysctl("hw.pagesize")),
        "macos_product_version": command(["sw_vers", "-productVersion"]),
        "macos_build": command(["sw_vers", "-buildVersion"]),
        "architecture": command(["uname", "-m"]),
        "python": sys.version.split()[0],
        "rustc": command(["rustc", "-V"]),
        "loadavg_start": [round(v, 2) for v in os.getloadavg()],
    }


def run_once(runner, work, budget, route, tag, targets, timeout, wrapper):
    com1 = work / f"{route}-{tag}.com1"
    argv = [*wrapper, str(runner), "boot-firmware", "--image", str(work / "firmware.bin"),
            "--budget", str(budget), "--com1-out", str(com1), "--nvme-image", str(work / "disk.raw"),
            "--tpm", "--nvram-out", str(work / f"{route}.nvram")]
    if route == "FV":
        argv += ["--fv-app", "hello"]
    for address in targets:
        argv += ["--sample-rip", hex(address)]
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.perf_counter_ns()
    proc = subprocess.Popen(argv, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    nice_seen = []
    deadline = time.monotonic() + timeout
    while True:
        try:
            out, err = proc.communicate(timeout=2)
            break
        except subprocess.TimeoutExpired:
            if time.monotonic() > deadline:
                proc.kill()
                proc.communicate()
                raise RuntimeError(f"{route} {tag}: killed after {timeout} s")
            value = read_nice(proc.pid)
            if value is not None:
                nice_seen.append(value)
    wall_ns = time.perf_counter_ns() - started
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    (work / f"{route}-{tag}.json").write_text(out, encoding="utf-8")
    if proc.returncode != 6:
        raise RuntimeError(f"{route} {tag}: runner exit {proc.returncode}: {err[-500:]}")
    report = json.loads(out)
    detail = 0 if route == "FV" else 1
    if ((report["status"], report["fw_stop"]) != ("firmware_stop", {"code": "0x21", "detail": f"0x{detail:016x}"})
            or report["exit"] != "halted" or report["page_fault"] or report["memory_error"]):
        raise RuntimeError(f"{route} {tag}: unexpected stop {report['status']} {report['exit']} {report['fw_stop']}")
    transcript = com1.read_bytes()
    expected = [b"OD-S7-27 route " + route.encode() + b"\r\n", b"OD-S7-27 verified\r\n"]
    if route == "FV":
        expected.append(b"OD-FW image hello: EFI_SUCCESS\r\n")
    if any(transcript.count(line) != 1 for line in expected):
        raise RuntimeError(f"{route} {tag}: missing or duplicate success markers in {com1}")
    return report, {
        "wall_ns": wall_ns,
        "cpu_user_s": round(after.ru_utime - before.ru_utime, 3),
        "cpu_sys_s": round(after.ru_stime - before.ru_stime, 3),
        "nice_seen": sorted(set(nice_seen)),
    }


def measure_route(runner, work, fixtures, route, budget, repeats, timeout, wrapper):
    # Discovery: the final RIP (the HLT where the firmware stops) is sampled in measured runs so
    # that the guest-only interval handoff -> stop has its own host timestamps.
    report, _ = run_once(runner, work, budget, route, "discover", [], timeout, wrapper)
    # The report RIP is the one after the executed HLT (0xF4, one byte); samples are taken before fetch.
    if report["activity"] != "Halted":
        raise RuntimeError(f"{route}: expected a halted machine, got {report['activity']}")
    stop_rip = int(report["rip"], 16) - 1
    targets = [fixtures["firmware_rip"], stop_rip]
    reference = {key: report[key] for key in ("retired", "quanta", "handoff_sha256", "budget", "tsc_hz")}
    rows = []
    for index in range(repeats + 1):  # index 0 is the discarded warmup
        report, meta = run_once(runner, work, budget, route, str(index), targets, timeout, wrapper)
        samples = report["rip_samples"]
        if [int(s["rip"], 16) for s in samples] != targets or samples[0]["retired"] != 0:
            raise RuntimeError(f"{route} {index}: unexpected RIP samples {samples}")
        if {key: report[key] for key in reference} != reference or report["image_sha256"] != fixtures["sha256"]["firmware"]:
            raise RuntimeError(f"{route} {index}: counts, handoff or ROM changed between runs")
        guest_ns = samples[1]["host_ns"] - samples[0]["host_ns"]
        guest_retired = samples[1]["retired"] - samples[0]["retired"]
        row = {"retired": report["retired"], "quanta": report["quanta"], "wall_ns": meta["wall_ns"],
               "guest_window_ns": guest_ns, "guest_window_retired": guest_retired,
               "guest_window_quanta": samples[1]["quanta"] - samples[0]["quanta"],
               "ips_wall": round(report["retired"] / (meta["wall_ns"] / 1e9), 1),
               "ips_guest_window": round(guest_retired / (guest_ns / 1e9), 1),
               "cpu_user_s": meta["cpu_user_s"], "cpu_sys_s": meta["cpu_sys_s"],
               "nice_seen": meta["nice_seen"]}
        rows.append(row)
        print(f"{route} {index}/{repeats}{' (warmup)' if index == 0 else ''}: retired {row['retired']} quanta {row['quanta']} "
              f"wall {meta['wall_ns'] / 1e9:.2f} s, {row['ips_wall'] / 1e6:.3f} M instr/s wall, "
              f"{row['ips_guest_window'] / 1e6:.3f} M instr/s in window, nice {meta['nice_seen']}", flush=True)
    measured = rows[1:]
    return {
        "route": route, "vcpus": 1, "reference": reference, "stop_rip": hex(stop_rip),
        "sample_rips": [hex(v) for v in targets],
        "warmup": rows[0], "runs": measured,
        "summary": {key: summarize([row[key] for row in measured])
                    for key in ("wall_ns", "guest_window_ns", "ips_wall", "ips_guest_window", "retired", "quanta")},
    }


def install_budget(ips):
    """Required speed for a 24 h install at hypothetical instruction counts."""
    rows = []
    for scale in SCALE_HYPOTHESIS:
        need = scale / DAY_SECONDS
        rows.append({"instructions": scale, "required_ips": round(need), "factor_over_measured": round(need / ips, 1),
                     "hours_at_measured": round(scale / ips / 3600, 1)})
    return {
        "status": "hypothesis",
        "basis": "No measured install instruction count exists before S8-25. 1e12..1e13 is an order of magnitude "
                 "for a 10..60 minute native Windows 11 setup on a ~3 GHz core at 1..3 instructions per ns "
                 "(unsourced hypothesis, to be replaced by the S8-25 count).",
        "measured_ips_used": round(ips), "limit_hours": 24, "rows": rows,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--work-dir", type=Path, required=True, help="work/stage8/bench-<run-id>/ (ignored by git)")
    parser.add_argument("--out-dir", type=Path, default=ROOT / "docs/reports/data/stage8")
    parser.add_argument("--run-id", default=datetime.datetime.now().strftime("s8-04-%Y%m%dT%H%M"))
    parser.add_argument("--runner", type=Path, help="prebuilt release od-runner (default: build in the work dir)")
    parser.add_argument("--budget", type=int, default=DEFAULT_BUDGET, help="global instruction budget of one boot")
    parser.add_argument("--repeats", type=int, default=5, help="measured runs after one warmup, at least 5")
    parser.add_argument("--routes", default="FV,ESP", help="comma list of FV, ESP")
    parser.add_argument("--timeout", type=int, default=900, help="seconds per runner process")
    parser.add_argument("--require-nice", action="store_true", help="abort when the effective nice is not -5")
    args = parser.parse_args()
    routes = args.routes.split(",")
    if args.repeats < 5 or args.budget <= 0 or any(route not in ("FV", "ESP") for route in routes):
        parser.error("need --repeats >= 5, a positive --budget and routes FV and/or ESP")
    work = args.work_dir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    wrapper, nice_before = nice_wrapper()
    probe = subprocess.run([*wrapper, "sh", "-c", "ps -o ni= -p $$"], capture_output=True, text=True)
    probe_value = int(probe.stdout.strip()) if re.fullmatch(r"-?\d+", probe.stdout.strip()) else None
    nice = {"requested": TARGET_NICE, "script_nice": nice_before, "wrapper": wrapper, "probe_effective": probe_value,
            "probe_stderr": probe.stderr.strip(), "satisfied": probe_value == TARGET_NICE}
    if not nice["satisfied"]:
        print(f"WARNING: effective nice is {probe_value}, not {TARGET_NICE}: {probe.stderr.strip()}", file=sys.stderr)
        if args.require_nice:
            sys.exit("nice -5 is unavailable (setpriority needs privileges); aborting by --require-nice")
    host = host_fingerprint()
    runner = args.runner
    if runner is None:
        subprocess.run(["cargo", "build", "-p", "od-runner", "--release", "--locked", "--offline",
                        "--target-dir", str(work / "runner-target")], cwd=ROOT, check=True)
        runner = work / "runner-target/release/od-runner"
    runner = runner.resolve()
    fixtures = fb.build_fixtures(work)
    results = [measure_route(runner, work, fixtures, route, args.budget, args.repeats, args.timeout, wrapper)
               for route in routes]
    nice["seen_in_runs"] = sorted({v for r in results for run in r["runs"] for v in run["nice_seen"]})
    nice["satisfied_in_runs"] = nice["seen_in_runs"] == [TARGET_NICE]
    host["loadavg_end"] = [round(v, 2) for v in os.getloadavg()]
    status = subprocess.run(["git", "status", "--short"], cwd=ROOT, capture_output=True, text=True).stdout
    result = {
        "schema_version": 1, "run_id": args.run_id, "task": "S8-04",
        "date_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "host": host, "nice": nice,
        "build": {
            "profile": "release", "git_head": command(["git", "rev-parse", "HEAD"]),
            "source_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=ROOT)).hexdigest(),
            "dirty_files": len(status.splitlines()),
            "runner_sha256": sha256_file(runner), "rom_sha256": fixtures["sha256"]["firmware"],
            "asset_sha256": fixtures["sha256"], "tools": fixtures["tool_versions"],
            "sources_sha256": {p: sha256_file(ROOT / p) for p in BENCH_SOURCES},
        },
        "engine": engine_facts(),
        "board": {"vcpu_count": 1, "ram_bytes": 128 << 20, "tpm": "CRB, SHA-256 and SHA-384 banks (--tpm)",
                  "nvme": "fixed controller, in-memory copy of disk.raw", "budget_instructions": args.budget,
                  "quantum": "remaining global budget, shortened by device events and deadlines "
                             "(crates/od-machine/src/firmware_board.rs run_boot_with, quantum_budget)"},
        "vcpu": vcpu_support(),
        "routes": results,
        "install_budget": install_budget(statistics.fmean(
            row["ips_wall"] for route in results for row in route["runs"])),
    }
    encoded = (json.dumps(result, indent=2) + "\n").encode()
    if len(encoded) > MAX_JSON:
        raise ValueError(f"benchmark JSON is {len(encoded)} bytes, limit is {MAX_JSON}")
    args.out_dir.mkdir(parents=True, exist_ok=True)
    out = args.out_dir / f"{datetime.date.today().isoformat()}-{args.run_id}.json"
    out.write_bytes(encoded)
    print(f"Wrote {len(encoded)} bytes to {out}")


if __name__ == "__main__":
    main()
