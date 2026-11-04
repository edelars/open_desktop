#!/usr/bin/env python3
import json
import subprocess
import sys
import time
from pathlib import Path

root = Path(sys.argv[1]).resolve()
evidence = root / "evidence"
name = "multi_block_taken_register_only"
argv = [
    str(root / "open-desktop-launcher-smoke"),
    "--vmhost", str(root / "od-vmhost-smoke"),
    "--hex", "48 3b c1 74 03 48 03 c3 48 03 c2",
    "--rax", "5", "--rcx", "5", "--rdx", "2", "--budget", "8",
]
start = time.perf_counter_ns()
try:
    proc = subprocess.run(argv, capture_output=True, text=True, timeout=15, check=False)
    timeout = None
    returncode = proc.returncode
    stdout, stderr = proc.stdout, proc.stderr
except subprocess.TimeoutExpired as exc:
    timeout = 15
    returncode = None
    stdout = (exc.stdout or b"").decode(errors="replace") if isinstance(exc.stdout, bytes) else (exc.stdout or "")
    stderr = (exc.stderr or b"").decode(errors="replace") if isinstance(exc.stderr, bytes) else (exc.stderr or "")
duration = time.perf_counter_ns() - start
(evidence / f"{name}.stdout.txt").write_text(stdout)
(evidence / f"{name}.stderr.txt").write_text(stderr)
try:
    observed = json.loads(stdout.strip().splitlines()[-1])
except (IndexError, json.JSONDecodeError):
    observed = None
expected = {"gpr": [7, 5, 2, 0], "rip": 11, "reason": 0, "instructions_executed": 3}
passed = returncode == 0 and observed is not None and all(observed.get(k) == v for k, v in expected.items())
record = {
    "name": name,
    "argv": argv,
    "returncode": returncode,
    "timeout_seconds": timeout,
    "duration_ns": duration,
    "stdout_file": f"{name}.stdout.txt",
    "stderr_file": f"{name}.stderr.txt",
    "expected_fields": expected,
    "observed": observed,
    "passed": passed,
    "debugger_used": False,
}
(evidence / f"{name}.status.json").write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({"name": name, "returncode": returncode, "duration_ns": duration, "passed": passed}, sort_keys=True))
sys.exit(0 if passed else 1)
