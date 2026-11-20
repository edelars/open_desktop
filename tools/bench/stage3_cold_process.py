#!/usr/bin/env python3
"""Measure fresh release od-runner processes on the S3-14 branch/helper corpus."""

import argparse
import hashlib
import itertools
import json
import os
import platform
from datetime import datetime, timezone
from pathlib import Path
import subprocess
import time


CASE_LIST = Path("tests/fixtures/x86/stage3/s3_14_branch_helper_case_list.txt")
CASE_LIST_SHA256 = "dd590c6c8f8ae1a09f2192e47c1d019f7449c814212c87e06194bf7425dd0881"
TRACE = Path("docs/reports/data/stage3/s3-36b/trace.json")
TRACE_SHA256 = "272eb80c3283e71962f1bc2a2e6d847723164d48f56dd3970134a814df57d2c8"
BACKENDS = {
    "interpreter": "interpreter",
    "jit": "jit",
    "jit-dispatch": "jit_ir_dispatch",
}
SAMPLE_GROUPS = 24
MIN_SAMPLE_NS = 200_000_000
CALIBRATION_NS = 300_000_000


def digest(data):
    return hashlib.sha256(data).hexdigest()


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def read_corpus():
    case_list = CASE_LIST.read_bytes()
    trace = TRACE.read_bytes()
    if digest(case_list) != CASE_LIST_SHA256 or digest(trace) != TRACE_SHA256:
        raise ValueError("S3-14 case list or trace changed")
    cases = []
    recorded_inputs = json.loads(trace)["inputs"]
    for line, recorded in zip(case_list.decode().splitlines(), recorded_inputs, strict=True):
        case = Path(line)
        sha256 = digest(case.read_bytes())
        if sha256 != recorded["fixture_sha256"]:
            raise ValueError(f"fixture digest changed for {case}")
        cases.append({"path": line, "case_id": recorded["case_id"], "sha256": sha256})
    return cases


def launch(runner, case, engine, revision):
    command = [str(runner), "--fixture", case["path"], "--engine", engine, "--json"]
    start = time.perf_counter_ns()
    proc = subprocess.run(command, capture_output=True, timeout=15, check=False)
    elapsed = time.perf_counter_ns() - start
    if proc.returncode:
        raise RuntimeError(f"{case['case_id']} / {engine}: {proc.stderr.decode(errors='replace')}")
    result = json.loads(proc.stdout)
    if (
        result["case_id"] != case["case_id"]
        or result["input_sha256"] != case["sha256"]
        or result["status"] != "completed"
        or result["backend"] != BACKENDS[engine]
        or result["build"]["revision"] != revision
        or result["build"]["profile"] != "release"
        or result["build"]["target"] != "aarch64-apple-darwin"
    ):
        raise ValueError(f"unexpected result or build for {case['case_id']} / {engine}")
    return elapsed, result


def outcome(result):
    keys = (
        "schema_version", "case_id", "input_sha256", "profile", "status", "expected_exit",
        "exit_reason", "instructions_executed", "snapshot",
    )
    return {key: result[key] for key in keys}


def sample(runner, cases, engine, revision, sweeps):
    elapsed = 0
    for _ in range(sweeps):
        for case in cases:
            duration, result = launch(runner, case, engine, revision)
            if outcome(result) != case["interpreter"]:
                raise ValueError(f"engine disagrees with the interpreter for {case['case_id']} / {engine}")
            elapsed += duration
    return elapsed


def measure(runner, cases, revision):
    calibration = []
    for sweeps in (4, 8, 16, 32, 64, 128):
        elapsed = {engine: sample(runner, cases, engine, revision, sweeps) for engine in BACKENDS}
        calibration.append({"sweeps": sweeps, "elapsed_ns": elapsed})
        if min(elapsed.values()) >= CALIBRATION_NS:
            break
    else:
        raise RuntimeError("calibration failed to reach 300 ms")

    groups = []
    orders = list(itertools.permutations(BACKENDS))
    for i in range(SAMPLE_GROUPS):
        order = orders[i % len(orders)]
        elapsed = {engine: sample(runner, cases, engine, revision, sweeps) for engine in order}
        if min(elapsed.values()) < MIN_SAMPLE_NS:
            raise RuntimeError(f"sample {i + 1} under 200 ms: {elapsed}")
        groups.append({"order": order, "elapsed_ns": elapsed})
    return sweeps, calibration, groups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner", type=Path, default=Path("target/release/od-runner"))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("--output already exists")
    rust_sources = ("Cargo.toml", "Cargo.lock", "crates", "tools/runner")
    if output("git", "status", "--porcelain", "--", *rust_sources):
        raise ValueError("benchmark requires Rust sources committed at HEAD")
    revision = output("git", "rev-parse", "HEAD")
    runner = args.runner.resolve(strict=True)
    runner_sha256 = digest(runner.read_bytes())
    script = Path(__file__)
    script_sha256 = digest(script.read_bytes())
    host = {
        "model": output("sysctl", "-n", "hw.model"),
        "cpu": output("sysctl", "-n", "machdep.cpu.brand_string"),
        "os_version": output("sw_vers", "-productVersion"),
        "os_build": output("sw_vers", "-buildVersion"),
        "python_version": platform.python_version(),
    }
    cases = read_corpus()
    started = datetime.now(timezone.utc).isoformat()
    load_start = os.getloadavg()
    for case in cases:
        _, result = launch(runner, case, "interpreter", revision)
        case["interpreter"] = outcome(result)
    build = result["build"]
    for engine in BACKENDS:
        if engine != "interpreter":
            sample(runner, cases, engine, revision, 1)
    sweeps, calibration, groups = measure(runner, cases, revision)
    original_inputs = [{k: v for k, v in case.items() if k != "interpreter"} for case in cases]
    if (
        digest(runner.read_bytes()) != runner_sha256
        or digest(script.read_bytes()) != script_sha256
        or read_corpus() != original_inputs
        or output("git", "rev-parse", "HEAD") != revision
        or output("git", "status", "--porcelain", "--", *rust_sources)
    ):
        raise ValueError("runner, benchmark script, Rust source revision or corpus changed during measurement")

    report = {
        "schema": "s3-35-cold-process-v1",
        "started_utc": started,
        "finished_utc": datetime.now(timezone.utc).isoformat(),
        "runner": str(args.runner),
        "runner_sha256": runner_sha256,
        "build": build,
        "script_sha256": script_sha256,
        "host": {
            **host,
            "load_average_start": load_start,
            "load_average_end": os.getloadavg(),
        },
        "timing_scope": "Sum of spawn, execution, JSON transfer and wait for each new runner process",
        "case_list_sha256": CASE_LIST_SHA256,
        "trace_sha256": TRACE_SHA256,
        "cases": cases,
        "sweeps_per_sample": sweeps,
        "calibration": calibration,
        "groups": groups,
    }
    with args.output.open("x") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")


if __name__ == "__main__":
    main()
