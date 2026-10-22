#!/usr/bin/env python3
"""Run the explicit Stage 1 local, physical-x86, and VMHost suites."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import time
import uuid
from pathlib import Path

from stage1_oracle import DEFAULT_CASES

ROOT = Path(__file__).resolve().parents[2]
PACKAGES = ["od-isa", "od-testkit", "od-runner", "od-x86-runner"]


def run_step(name: str, command: list[str], run_dir: Path, timeout: int) -> dict:
    started = time.monotonic()
    try:
        process = subprocess.run(
            command,
            cwd=ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            check=False,
        )
        output = process.stdout
        status = "passed" if process.returncode == 0 else "failed"
        returncode = process.returncode
    except (OSError, subprocess.TimeoutExpired) as error:
        output = f"{type(error).__name__}: {error}\n"
        status = "failed"
        returncode = None
    log_path = run_dir / f"{name}.log"
    log_path.write_text(output)
    return {
        "name": name,
        "command": shlex.join(command),
        "status": status,
        "returncode": returncode,
        "duration_seconds": round(time.monotonic() - started, 3),
        "log": str(log_path.relative_to(ROOT)),
        "output_tail": output[-2000:],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=("local", "oracle", "vmhost", "all"), default="local")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--generated-count", type=int, default=0)
    parser.add_argument("--seed", default="0x5eedcafe")
    parser.add_argument("--launcher", default="target/open-desktop-launcher-smoke")
    parser.add_argument("--vmhost", default="target/od-vmhost-smoke")
    args = parser.parse_args()
    if args.repeats < 2:
        parser.error("--repeats must be at least 2")
    if args.generated_count < 0 or args.generated_count > 100_000:
        parser.error("--generated-count must be between 0 and 100000")

    run_id = time.strftime("stage1-harness-%Y%m%d-%H%M%S", time.gmtime()) + "-" + uuid.uuid4().hex[:8]
    run_dir = ROOT / "work" / "stage1-harness" / run_id
    run_dir.mkdir(parents=True, exist_ok=False)
    summary_path = run_dir / "summary.json"
    selected = {args.suite} if args.suite != "all" else {"local", "oracle", "vmhost"}
    summary = {
        "run_id": run_id,
        "status": "running",
        "suite_selection": args.suite,
        "suites": {name: {"status": "not_run", "steps": []} for name in ("local", "oracle", "vmhost")},
        "host": {"x86_host": args.host, "x86_user": args.user, "remote_root": args.remote_root},
        "oracle": {"repeats": args.repeats, "generated_count": args.generated_count, "seed": args.seed},
    }
    summary_path.write_text(json.dumps(summary, indent=2) + "\n")

    if "local" in selected:
        suite = summary["suites"]["local"]
        commands = [
            ("format", ["cargo", "fmt", "--all", "--", "--check"], 300),
            ("workspace_check", ["cargo", "check", "--workspace", "--all-targets", "--offline"], 1200),
            ("targeted_tests", ["cargo", "test", "--offline", *[part for package in PACKAGES for part in ("-p", package)]], 1200),
            ("orchestration_tests", [sys.executable, "tests/cpu/test_stage1_harness.py"], 60),
            ("fixture_digests", ["shasum", "-a", "256", "-c", "tests/fixtures/x86/SHA256SUMS"], 60),
        ]
        for name, command, timeout in commands:
            suite["steps"].append(run_step(name, command, run_dir, timeout))
            if suite["steps"][-1]["status"] != "passed":
                break
        suite["status"] = "passed" if all(step["status"] == "passed" for step in suite["steps"]) and len(suite["steps"]) == len(commands) else "failed"

    if "oracle" in selected:
        suite = summary["suites"]["oracle"]
        generated_dir_arg: str | None = None
        if args.generated_count:
            generated_dir = ROOT / "work" / f"stage1-harness-generated-{run_id}"
            generated = run_step(
                "generate_corpus",
                ["cargo", "run", "--offline", "-p", "od-runner", "--", "--generate-corpus", args.seed, str(args.generated_count), str(generated_dir.relative_to(ROOT))],
                run_dir,
                300,
            )
            suite["steps"].append(generated)
            if generated["status"] == "passed":
                generated_dir_arg = str(generated_dir.relative_to(ROOT))
                suite["generated_cases"] = len(list(generated_dir.glob("*.case.json")))
        if not suite["steps"] or suite["steps"][-1]["status"] == "passed":
            command = [sys.executable, "tests/cpu/stage1_oracle.py", "--host", args.host, "--user", args.user,
                       "--remote-root", args.remote_root, "--repeats", str(args.repeats)]
            if generated_dir_arg:
                command.extend(DEFAULT_CASES)
                command.append(generated_dir_arg)
            oracle_step = run_step("physical_oracle", command, run_dir, 7200)
            suite["steps"].append(oracle_step)
            if oracle_step["status"] == "passed":
                for line in oracle_step["output_tail"].splitlines():
                    if line.startswith("manifest: "):
                        suite["manifest"] = line.removeprefix("manifest: ")
        suite["status"] = "passed" if suite["steps"] and all(step["status"] == "passed" for step in suite["steps"]) else "failed"

    if "vmhost" in selected:
        suite = summary["suites"]["vmhost"]
        command = [sys.executable, "tests/system/vmhost_control_race_smoke.py", args.launcher, args.vmhost]
        suite["steps"].append(run_step("vmhost_control_scenarios", command, run_dir, 1800))
        suite["status"] = suite["steps"][0]["status"]

    for name in selected:
        if summary["suites"][name]["status"] != "passed":
            summary["status"] = "failed"
            break
    else:
        summary["status"] = "passed"
    summary["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    summary_path.write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))
    print(f"summary_path={summary_path.relative_to(ROOT)}")
    return 0 if summary["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
