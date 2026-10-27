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
DISPATCH_PACKAGES = ["od-testkit", "od-runner", "od-x86-runner"]


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
    parser.add_argument("--engine", choices=("jit", "jit-block", "jit-dispatch"))
    parser.add_argument("--manifest", help="explicit jit-block fixture manifest")
    parser.add_argument("--local-only", action="store_true",
                        help="run local interpreter versus jit-dispatch comparisons only")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--generated-count", type=int,
                        help="generated fixture count (default: 1000 for jit-dispatch, 0 otherwise)")
    parser.add_argument("--seed", default="0x5eedcafe")
    parser.add_argument("--launcher", default="target/open-desktop-launcher-smoke")
    parser.add_argument("--vmhost", default="target/od-vmhost-smoke")
    args = parser.parse_args()
    engine = args.engine or ("jit-dispatch" if args.local_only else "jit")
    generated_count = args.generated_count
    if generated_count is None:
        generated_count = 1000 if engine == "jit-dispatch" else 0
    if args.repeats < 2:
        parser.error("--repeats must be at least 2")
    if generated_count < 0 or generated_count > 100_000:
        parser.error("--generated-count must be between 0 and 100000")
    if args.local_only and (args.suite != "oracle" or engine != "jit-dispatch"):
        parser.error("--local-only requires --suite oracle and --engine jit-dispatch")
    if args.manifest and engine != "jit-block":
        parser.error("--manifest requires --engine jit-block")
    if engine == "jit-block" and not args.manifest:
        parser.error("--engine jit-block requires --manifest")
    if args.manifest and generated_count:
        parser.error("--manifest cannot be combined with generated fixtures")

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
        "engine": engine,
        "evidence_scope": "local_only" if args.local_only else "physical_triple" if "oracle" in selected else "not_applicable",
        "oracle": {"repeats": args.repeats, "generated_count": generated_count, "seed": args.seed,
                   "local_only": args.local_only},
    }
    summary_path.write_text(json.dumps(summary, indent=2) + "\n")

    if "local" in selected:
        suite = summary["suites"]["local"]
        packages = DISPATCH_PACKAGES if engine in {"jit-block", "jit-dispatch"} else PACKAGES
        commands = [
            ("format", ["cargo", "fmt", "--all", "--", "--check"], 300),
            ("workspace_check", ["cargo", "check", "--workspace", "--all-targets", "--offline"], 1200),
            ("targeted_tests", ["cargo", "test", "--offline", *[part for package in packages for part in ("-p", package)]], 1200),
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
        if generated_count:
            generated_dir = ROOT / "work" / f"stage1-harness-generated-{run_id}"
            generated = run_step(
                "generate_corpus",
                ["cargo", "run", "--offline", "-p", "od-runner", "--", "--generate-corpus", args.seed, str(generated_count), str(generated_dir.relative_to(ROOT))],
                run_dir,
                300,
            )
            suite["steps"].append(generated)
            if generated["status"] == "passed":
                generated_dir_arg = str(generated_dir.relative_to(ROOT))
                suite["generated_cases"] = len(list(generated_dir.glob("*.case.json")))
        if not suite["steps"] or suite["steps"][-1]["status"] == "passed":
            command = [sys.executable, "tests/cpu/stage1_oracle.py", "--host", args.host, "--user", args.user,
                       "--remote-root", args.remote_root, "--repeats", str(args.repeats), "--engine", engine]
            if args.local_only:
                command.append("--local-only")
            if args.manifest:
                command.extend(["--manifest", args.manifest])
            elif generated_dir_arg or engine == "jit-dispatch":
                command.extend(DEFAULT_CASES)
            if generated_dir_arg:
                command.append(generated_dir_arg)
            oracle_step = run_step("local_oracle" if args.local_only else "physical_oracle", command, run_dir, 7200)
            suite["steps"].append(oracle_step)
            if oracle_step["status"] == "passed":
                if args.local_only:
                    oracle_step["status"] = "local_only"
                    suite["evidence_scope"] = "local_only"
                    suite["status"] = "local_only"
                for line in oracle_step["output_tail"].splitlines():
                    for prefix in ("manifest: ", "local-only manifest: "):
                        if line.startswith(prefix):
                            suite["manifest"] = line.removeprefix(prefix)
        if not args.local_only or suite.get("status") != "local_only":
            suite["status"] = "passed" if suite["steps"] and all(step["status"] == "passed" for step in suite["steps"]) else "failed"

    if "vmhost" in selected:
        suite = summary["suites"]["vmhost"]
        command = [sys.executable, "tests/system/vmhost_control_race_smoke.py", args.launcher, args.vmhost]
        suite["steps"].append(run_step("vmhost_control_scenarios", command, run_dir, 1800))
        suite["status"] = suite["steps"][0]["status"]

    for name in selected:
        if summary["suites"][name]["status"] not in {"passed", "local_only"}:
            summary["status"] = "failed"
            break
    else:
        summary["status"] = "local_only" if args.local_only else "passed"
    summary["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    summary_path.write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))
    print(f"summary_path={summary_path.relative_to(ROOT)}")
    return 0 if summary["status"] in {"passed", "local_only"} else 1


if __name__ == "__main__":
    raise SystemExit(main())
