#!/usr/bin/env bash
# Stage 4, S4-44a/S4-44b: documented reproducible LOCAL check set.
#
# Only calls existing commands (cargo fmt/check/test); no CPU semantics live here.
# It does NOT boot Linux, does NOT touch the Mac mini and does NOT build guest/firmware
# artifacts: those are printed and recorded as `not_run` / `blocked` (see NOT_RUN below).
#
# Output:
#   logs      work/stage4/s4-44b/<step>.log      (ignored by Git)
#   summary   docs/reports/data/stage4/s4-44b-local-set.json   (override: STAGE4_SUMMARY)
#
# Step statuses: passed | failed | known_divergence | timeout.
#   known_divergence: `cargo fmt --check` fails with exactly the diff listed in KNOWN_FMT_DIFFS.
# Every `cargo test` step runs with --no-fail-fast: a failing test target does not stop the
# other targets of the step, and the summary lists the result of EACH target (`targets`:
# binary, status, counts, names of failed tests) parsed from the cargo log.
# A failed/timeout step never stops the set silently: it is recorded, the remaining steps
# still run, and the script exits 1 at the end. Exit 2 = setup error.
#
# Environment:
#   STAGE4_STEP_TIMEOUT   per-step timeout in seconds (default 1200)
#   STAGE4_LOG_DIR        log directory (default work/stage4/s4-44b)
#   STAGE4_SUMMARY        summary path
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT" || exit 2

STEP_TIMEOUT="${STAGE4_STEP_TIMEOUT:-1200}"
LOG_DIR="${STAGE4_LOG_DIR:-$ROOT/work/stage4/s4-44b}"
SUMMARY="${STAGE4_SUMMARY:-$ROOT/docs/reports/data/stage4/s4-44b-local-set.json}"
# Known pre-existing rustfmt divergences (not new errors). Each entry is
# "<path relative to ROOT> <diff blocks> <changed +/- lines> <sha256[:16] of the changed lines>",
# as printed by tools/stage4/fmt_fingerprint.py. The fmt step is `known_divergence` only if its
# fingerprint equals this list exactly: a new diff block or any other change, even inside a
# known file, makes it `failed`. The hashes depend on the rustfmt version (recorded in the summary).
KNOWN_FMT_DIFFS=(
  "crates/od-host/src/storage_abi_v1.rs 1 9 a45ac43961401f05"
  "tools/image-tool/tests/cli.rs 1 5 98bc3ea4bc4e24e9"
)

mkdir -p "$LOG_DIR" "$(dirname "$SUMMARY")" || exit 2
TSV="$LOG_DIR/steps.tsv"
: > "$TSV" || exit 2

# Steps: id|command. Order is fixed; ids are the stable keys in the summary.
STEPS=(
  "fmt|cargo fmt --all -- --check --color never"
  "check-workspace|cargo check --workspace --all-targets --locked --offline"
  "test-od-isa|cargo test -p od-isa --no-fail-fast --locked --offline"
  "test-od-machine|cargo test -p od-machine --no-fail-fast --locked --offline"
  "test-od-jit|cargo test -p od-jit --no-fail-fast --locked --offline"
  "test-od-host|cargo test -p od-host --no-fail-fast --locked --offline"
  "test-od-runner|cargo test -p od-runner --no-fail-fast --locked --offline"
  "test-od-bench|cargo test -p od-bench --no-fail-fast --locked --offline"
  "test-rest-of-workspace|cargo test --workspace --exclude od-isa --exclude od-machine --exclude od-jit --exclude od-host --exclude od-runner --exclude od-bench --no-fail-fast --locked --offline"
)

# NOT RUN: id|status|what|requires
NOT_RUN=(
  "linux-boot-positive-negative|not_run|Linux positive/negative boot|vmlinuz, initramfs and ACPI manifest/blob (guest artifacts, not in the repository)"
  "linux-direct-gate-real-kernel|not_run|od-runner linux-direct-gate on a real kernel|real vmlinuz + initramfs + ACPI manifest passed to linux-direct-gate"
  "mac-mini-hardware-cases|blocked|Mac mini hardware cases (physical Hypervisor.framework / KVM-method cases)|the Mac mini stand and its owner; stage 3 hardware gate stays in its own scope"
  "guest-firmware-artifacts-build|not_run|guest/firmware artifacts build (tools/build/*.py, firmware/uefi, tools/build/test_*.py)|x86_64-unknown-none/uefi toolchain run as a separate build job; the outputs are inputs of the boot cases above"
  "vmhost-system-smokes|not_run|tests/system Python/Swift/C smokes of the VMHost app (macOS app bundle)|built VMHost app bundle and a desktop session; stage 3 scope"
)

KNOWN_NOTE="rustfmt divergences are the known ones (KNOWN_FMT_DIFFS), not new errors"

now() { date +%s; }

run_step() {
  local id="$1" cmd="$2" log="$LOG_DIR/$1.log" t0 t1 rc status note=""
  echo "=== STEP $id: $cmd"
  t0=$(now)
  { echo "# $cmd"; echo "# started_utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"; } > "$log"
  perl -e 'alarm shift; exec @ARGV' "$STEP_TIMEOUT" bash -c "$cmd" >> "$log" 2>&1
  rc=$?
  t1=$(now)
  if [ "$rc" -eq 0 ]; then
    status=passed
  elif [ "$rc" -eq 142 ]; then
    status=timeout; note="killed by SIGALRM after ${STEP_TIMEOUT}s"
  else
    status=failed
  fi
  if [ "$id" = fmt ] && [ "$status" = failed ]; then
    # Compare the diff fingerprint (files, block counts, changed lines) with KNOWN_FMT_DIFFS.
    local got want
    got="$(python3 -I "$ROOT/tools/stage4/fmt_fingerprint.py" "$ROOT" "$log" 2>&1)"
    want="$(printf '%s\n' "${KNOWN_FMT_DIFFS[@]}" | sort)"
    if [ -n "$got" ] && [ "$got" = "$want" ]; then
      status=known_divergence
      note="$KNOWN_NOTE; diff fingerprint equals KNOWN_FMT_DIFFS: $(echo "$got" | awk '{print $1}' | tr '\n' ' ')"
    else
      note="fmt diff differs from KNOWN_FMT_DIFFS; got: $(echo "$got" | tr '\n' ';')"
    fi
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$id" "$cmd" "$status" "$rc" "$((t1 - t0))" "$log" "$note" >> "$TSV"
  echo "=== RESULT $id: $status rc=$rc ${note:+($note)} duration=$((t1 - t0))s"
  [ "$status" = passed ] || [ "$status" = known_divergence ]
}

SET_START=$(now)
FAILED=0
for s in "${STEPS[@]}"; do
  run_step "${s%%|*}" "${s#*|}" || FAILED=$((FAILED + 1))
done
SET_END=$(now)

for n in "${NOT_RUN[@]}"; do
  IFS='|' read -r nid nstatus nwhat nreq <<< "$n"
  echo "NOT RUN: $nwhat — требует $nreq"
done

# --- Summary -----------------------------------------------------------------
export KNOWN_FMT_DIFFS_LINES="$(printf '%s\n' "${KNOWN_FMT_DIFFS[@]}")"
export ROOT SUMMARY TSV SET_START SET_END STEP_TIMEOUT
export NOT_RUN_LINES="$(printf '%s\n' "${NOT_RUN[@]}")"
export GIT_HEAD="$(git rev-parse HEAD 2>&1)"
export GIT_BRANCH="$(git rev-parse --abbrev-ref HEAD 2>&1)"
export GIT_PORCELAIN="$(git status --porcelain 2>&1)"
export SCRIPT_SHA="$(shasum -a 256 "$ROOT/tools/stage4/run_local_set.sh" | awk '{print $1}')"
export RUSTC_V="$(rustc --version 2>&1)" CARGO_V="$(cargo --version 2>&1)"
export MACOS_V="$(sw_vers -productVersion 2>&1)" UNAME_V="$(uname -m 2>&1)"
python3 -I - <<'PY' || exit 2
import hashlib, json, os
from datetime import datetime, timezone

def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()

def targets_of(log):
    """Per-target results of a `cargo test` log: one entry per Running/Doc-tests block."""
    import re
    out, cur = [], None
    run = re.compile(r"^\s+Running (.+?)(?: \((.+)\))?$")
    doc = re.compile(r"^\s+Doc-tests (\S+)")
    res = re.compile(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored")
    bad = re.compile(r"^test (.+) \.\.\. FAILED$")
    for line in open(log, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        m = run.match(line)
        if m:
            binary = os.path.basename(m.group(2) or "").rsplit("-", 1)[0]
            cur = {"target": m.group(1), "binary": binary, "status": "no_result",
                   "passed": 0, "failed": 0, "ignored": 0, "failed_tests": []}
            out.append(cur)
            continue
        m = doc.match(line)
        if m:
            cur = {"target": "Doc-tests " + m.group(1), "binary": m.group(1), "status": "no_result",
                   "passed": 0, "failed": 0, "ignored": 0, "failed_tests": []}
            out.append(cur)
            continue
        if cur is None:
            continue
        m = bad.match(line)
        if m and m.group(1) not in cur["failed_tests"]:
            cur["failed_tests"].append(m.group(1))
        m = res.match(line)
        if m:
            cur["status"] = "passed" if m.group(1) == "ok" else "failed"
            cur["passed"], cur["failed"], cur["ignored"] = (int(m.group(i)) for i in (2, 3, 4))
            cur = None
    return out

steps = []
for line in open(os.environ["TSV"], encoding="utf-8"):
    sid, cmd, status, rc, dur, log, note = line.rstrip("\n").split("\t")
    targets = targets_of(log) if cmd.startswith("cargo test") else None
    steps.append({
        "id": sid, "command": cmd, "status": status, "returncode": int(rc),
        "duration_seconds": int(dur),
        "log": os.path.relpath(log, os.environ["ROOT"]), "log_sha256": sha(log),
        "note": note,
        **({"targets": targets, "targets_total": len(targets),
            "targets_failed": sum(1 for t in targets if t["status"] != "passed")}
           if targets is not None else {}),
    })
not_run = []
for line in os.environ["NOT_RUN_LINES"].splitlines():
    nid, status, what, req = line.split("|")
    not_run.append({"id": nid, "status": status, "what": what, "requires": req})
count = lambda s: sum(1 for x in steps if x["status"] == s)
doc = {
    "schema": "od.stage4.s4-44b.local-set.v2",
    "generated_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "environment": {
        "git_head": os.environ["GIT_HEAD"], "git_branch": os.environ["GIT_BRANCH"],
        "git_status_porcelain": os.environ["GIT_PORCELAIN"].splitlines(),
        "script_sha256": os.environ["SCRIPT_SHA"],
        "rustc": os.environ["RUSTC_V"], "cargo": os.environ["CARGO_V"],
        "macos": os.environ["MACOS_V"], "arch": os.environ["UNAME_V"],
        "step_timeout_seconds": int(os.environ["STEP_TIMEOUT"]),
        "known_fmt_diffs": os.environ["KNOWN_FMT_DIFFS_LINES"].splitlines(),
    },
    "total_duration_seconds": int(os.environ["SET_END"]) - int(os.environ["SET_START"]),
    "steps": steps,
    "totals": {
        "passed": count("passed"), "known_divergence": count("known_divergence"),
        "failed": count("failed"), "timeout": count("timeout"),
        "excluded_steps": 0,
        "not_run": sum(1 for x in not_run if x["status"] == "not_run"),
        "blocked": sum(1 for x in not_run if x["status"] == "blocked"),
    },
    "not_run": not_run,
}
doc["overall_local"] = (
    "passed" if doc["totals"]["failed"] == 0 and doc["totals"]["timeout"] == 0 else "failed")
with open(os.environ["SUMMARY"], "w", encoding="utf-8") as f:
    json.dump(doc, f, ensure_ascii=False, indent=2)
    f.write("\n")
print("summary:", os.environ["SUMMARY"], "overall_local:", doc["overall_local"])
PY

[ "$FAILED" -eq 0 ] || { echo "FAILED steps: $FAILED" >&2; exit 1; }
exit 0
