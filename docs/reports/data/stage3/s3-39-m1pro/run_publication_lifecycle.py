#!/usr/bin/env python3
import json
import subprocess
import sys
import time
from pathlib import Path

root = Path(sys.argv[1]).resolve()
evidence = root / "evidence"
name = "publication_lifecycle_same_process"
argv = [str(root / name)]
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
expected = {
    "create_status": 0,
    "first_status": 0,
    "first_rax": 0,
    "first_rip": 5,
    "first_reason": 0,
    "first_instructions": 6,
    "second_status": 0,
    "second_rax": 0,
    "second_rip": 5,
    "second_reason": 0,
    "second_instructions": 6,
    "destroy_status": 0,
}
passed = returncode == 0 and observed == expected
record = {
    "name": name,
    "argv": argv,
    "returncode": returncode,
    "timeout_seconds": timeout,
    "duration_ns": duration,
    "stdout_file": f"{name}.stdout.txt",
    "stderr_file": f"{name}.stderr.txt",
    "expected": expected,
    "observed": observed,
    "passed": passed,
    "debugger_used": False,
}
(evidence / f"{name}.status.json").write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({"name": name, "returncode": returncode, "duration_ns": duration, "passed": passed}, sort_keys=True))
sys.exit(0 if passed else 1)
