#!/usr/bin/env python3
import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path

root = Path(sys.argv[1]).resolve()
launcher = root / "open-desktop-launcher-smoke"
vmhost = root / "od-vmhost-smoke"
evidence = root / "evidence"
evidence.mkdir(mode=0o700)


def output(argv):
    p = subprocess.run(argv, capture_output=True, text=True, timeout=15, check=False)
    return {"argv": argv, "returncode": p.returncode, "stdout": p.stdout, "stderr": p.stderr}

machine = {
    "uname": output(["uname", "-a"]),
    "macos": output(["sw_vers"]),
    "model": output(["sysctl", "-n", "hw.model"]),
    "cpu": output(["sysctl", "-n", "machdep.cpu.brand_string"]),
    "memory_bytes": output(["sysctl", "-n", "hw.memsize"]),
    "page_size": output(["pagesize"]),
    "python": platform.python_version(),
    "outside_debugger": True,
}
(evidence / "machine.json").write_text(json.dumps(machine, indent=2) + "\n")

cases = [
    {
        "name": "arithmetic",
        "args": ["--hex", "48 2b c1 75 fb", "--rax", "3", "--rcx", "1", "--budget", "32"],
        "expected": {"gpr": [0, 1, 0, 0], "rip": 5, "reason": 0, "instructions_executed": 6},
    },
    {
        "name": "multi_block_taken_branch",
        "args": ["--hex", "48 3b c1 74 04 48 83 c0 01 48 83 c0 02", "--rax", "5", "--rcx", "5", "--budget", "8"],
        "expected": {"gpr": [7, 5, 0, 0], "rip": 13, "reason": 0, "instructions_executed": 3},
    },
    {
        "name": "s3_14_live_through_helpers",
        "args": ["--hex", "48 03 c0 48 2b d2 48 03 c3 48 2b c9 48 3b c3 74 00", "--rax", "0x8000000000000000", "--rcx", "7", "--rdx", "9", "--rbx", "1", "--budget", "16"],
        "expected": {"gpr": [1, 0, 0, 1], "rip": 17, "reason": 0, "instructions_executed": 6},
    },
    {
        "name": "cooperative_stop",
        "args": ["--hex", "75 fe", "--budget", "18446744073709551615", "--stop-after-ms", "25"],
        "expected": {"reason": 2},
    },
    {
        "name": "publication_lifecycle_repeat",
        "args": ["--hex", "48 2b c1 75 fb", "--rax", "3", "--rcx", "1", "--budget", "32"],
        "expected": {"gpr": [0, 1, 0, 0], "rip": 5, "reason": 0, "instructions_executed": 6},
    },
]
records = []
for case in cases:
    argv = [str(launcher), "--vmhost", str(vmhost), *case["args"]]
    start = time.perf_counter_ns()
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=15, check=False)
        elapsed = time.perf_counter_ns() - start
        stdout = p.stdout
        stderr = p.stderr
        status = p.returncode
        timeout = None
    except subprocess.TimeoutExpired as exc:
        elapsed = time.perf_counter_ns() - start
        stdout = (exc.stdout or b"").decode(errors="replace") if isinstance(exc.stdout, bytes) else (exc.stdout or "")
        stderr = (exc.stderr or b"").decode(errors="replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
        status = None
        timeout = 15
    (evidence / f"{case['name']}.stdout.txt").write_text(stdout)
    (evidence / f"{case['name']}.stderr.txt").write_text(stderr)
    record = {
        "name": case["name"],
        "argv": argv,
        "returncode": status,
        "timeout_seconds": timeout,
        "duration_ns": elapsed,
        "stdout_file": f"{case['name']}.stdout.txt",
        "stderr_file": f"{case['name']}.stderr.txt",
        "expected_fields": case["expected"],
    }
    try:
        observed = json.loads(stdout.strip().splitlines()[-1])
    except (IndexError, json.JSONDecodeError):
        observed = None
    record["observed"] = observed
    if status != 0 or observed is None:
        record["passed"] = False
        record["failure"] = "nonzero process status or missing JSON result"
    else:
        record["passed"] = all(observed.get(k) == v for k, v in case["expected"].items())
        if not record["passed"]:
            record["failure"] = "result fields differ from expected bounded outcome"
    (evidence / f"{case['name']}.status.json").write_text(json.dumps(record, indent=2) + "\n")
    records.append(record)
    print(json.dumps({"name": case["name"], "returncode": status, "duration_ns": elapsed, "passed": record["passed"]}, sort_keys=True))

summary = {
    "machine": machine,
    "cases": records,
    "all_passed": all(record["passed"] for record in records),
    "debugger_used": False,
}
(evidence / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
sys.exit(0 if summary["all_passed"] else 1)
