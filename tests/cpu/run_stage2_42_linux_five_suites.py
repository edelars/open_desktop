#!/usr/bin/env python3
"""Run and aggregate the selected S2-42 Linux-native acceptance suites."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import signal
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any

import stage1_oracle

ROOT = Path(__file__).resolve().parents[2]
SUITE_IDS = ("S2-42c", "S2-42e", "S2-42h", "S2-42i", "S2-42r")
ALL_LETTERED_SUITES = tuple(f"S2-42{letter}" for letter in "abcdefghijklmnopqrstuvwxyz") + tuple(
    f"S2-42{letter}" for letter in ("aa", "ab", "ac", "ad", "ae", "af")
)
UNSELECTED_SUITES = tuple(suite for suite in ALL_LETTERED_SUITES if suite not in SUITE_IDS)
EXPECTED_CASES = {"S2-42c": 1030, "S2-42e": 1080, "S2-42h": 1056,
                  "S2-42i": 1060, "S2-42r": 1056}
SUMMARY_FILES = {
    "S2-42e": "s2-42e-summary.json",
    "S2-42h": "s2-42h-summary.json",
    "S2-42i": "s2-42i-summary.json",
    "S2-42r": "s2-42r-summary.json",
}


class AcceptanceError(Exception):
    pass


def json_bytes(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n").encode()


def read_json(path: Path) -> dict[str, Any]:
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise AcceptanceError(f"duplicate JSON key {key!r} in {path}")
            result[key] = value
        return result

    value = json.loads(path.read_bytes(), object_pairs_hook=unique_object)
    if not isinstance(value, dict):
        raise AcceptanceError(f"expected a JSON object in {path}")
    return value


def child_command(suite: str, child_output: Path, host: str, user: str,
                  remote_root: str) -> list[str]:
    common = ["--host", host, "--user", user, "--remote-root", remote_root]
    if suite == "S2-42c":
        return [sys.executable, str(ROOT / "tests/cpu/stage2_oracle.py"),
                "--suite", "xorpd-native-corpus", *common,
                "--output-root", str(child_output)]
    script = {
        "S2-42e": "generate_stage2_mov_memory_corpus.py",
        "S2-42h": "generate_stage2_stack_corpus.py",
        "S2-42i": "generate_stage2_rep_stosb_corpus.py",
        "S2-42r": "generate_stage2_branch_corpus.py",
    }[suite]
    return [sys.executable, str(ROOT / "tests/cpu" / script), "--run-oracle",
            *common, "--oracle-output-root", str(child_output)]


def _manifest_path(suite: str, child_output: Path, summary: dict[str, Any] | None) -> Path:
    if summary is not None and isinstance(summary.get("oracle_manifest"), str):
        path = Path(summary["oracle_manifest"])
        return path if path.is_absolute() else ROOT / path
    if suite == "S2-42c":
        manifests = sorted(child_output.glob("*/manifest.json"))
        if len(manifests) != 1:
            raise AcceptanceError(f"{suite}: expected exactly one saved run manifest, found {len(manifests)}")
        return manifests[0]
    raise AcceptanceError(f"{suite}: child summary has no oracle_manifest path")


def _read_child_summary(suite: str, child_output: Path) -> dict[str, Any] | None:
    name = SUMMARY_FILES.get(suite)
    if name is None:
        return None
    path = child_output / name
    if not path.is_file():
        raise AcceptanceError(f"{suite}: child summary is missing: {path}")
    return read_json(path)


def validate_child(suite: str, child_output: Path, process_status: int,
                   expected_fingerprint: str) -> dict[str, Any]:
    """Validate aggregate-level identity; child scripts own corpus/result semantics."""
    if process_status != 0:
        raise AcceptanceError(f"{suite}: child command exited {process_status}")
    summary = _read_child_summary(suite, child_output)
    manifest_path = _manifest_path(suite, child_output, summary)
    if not manifest_path.is_file():
        raise AcceptanceError(f"{suite}: saved evidence manifest is missing: {manifest_path}")
    manifest = read_json(manifest_path)
    if manifest.get("status") != "passed":
        raise AcceptanceError(f"{suite}: saved evidence status is not passed")
    if suite == "S2-42c" and manifest.get("suite") != "xorpd-native-corpus":
        raise AcceptanceError(f"{suite}: evidence is not the selected XORPD native corpus")
    fingerprint = manifest.get("source_fingerprint")
    if not isinstance(fingerprint, str) or not re.fullmatch(r"[0-9a-f]{64}", fingerprint):
        raise AcceptanceError(f"{suite}: saved evidence has no valid source fingerprint")
    if fingerprint != expected_fingerprint:
        raise AcceptanceError(f"{suite}: evidence source fingerprint is stale")
    if suite == "S2-42c":
        counts = manifest.get("case_counts")
        cases = counts.get("total") if isinstance(counts, dict) else None
        repeats = manifest.get("repeats")
    else:
        if not isinstance(summary, dict) or summary.get("status") != "passed":
            raise AcceptanceError(f"{suite}: child strict verifier did not report passed")
        if summary.get("physical_mac_acceptance") != "not_run":
            raise AcceptanceError(f"{suite}: physical Mac acceptance must remain not_run")
        cases = summary.get("total_count", summary.get("case_count"))
        repeats = summary.get("repeat_count")
    if cases != EXPECTED_CASES[suite]:
        raise AcceptanceError(f"{suite}: selected case count is {cases!r}, expected {EXPECTED_CASES[suite]}")
    if repeats != 2:
        raise AcceptanceError(f"{suite}: evidence does not contain exactly two repeats")
    return {
        "suite_id": suite,
        "status": "passed",
        "case_count": cases,
        "repeat_count": repeats,
        "source_fingerprint": fingerprint,
        "evidence_manifest": str(manifest_path.resolve()),
        "child_summary": summary,
        "physical_mac_acceptance": "not_run",
    }


def validate_aggregate(children: dict[str, dict[str, Any]],
                       expected_fingerprint: str) -> dict[str, Any]:
    if set(children) != set(SUITE_IDS):
        missing = sorted(set(SUITE_IDS) - set(children))
        extra = sorted(set(children) - set(SUITE_IDS))
        raise AcceptanceError(f"aggregate suite set differs; missing={missing}, extra={extra}")
    fingerprints = {child.get("source_fingerprint") for child in children.values()}
    if fingerprints != {expected_fingerprint}:
        raise AcceptanceError("child evidence is stale or does not share the current source fingerprint")
    for suite in SUITE_IDS:
        child = children[suite]
        if child.get("status") != "passed" or child.get("case_count") != EXPECTED_CASES[suite] \
                or child.get("repeat_count") != 2:
            raise AcceptanceError(f"{suite}: required aggregate check did not pass")
    return {
        "status": "passed",
        "selection": {
            "selected_suites": list(SUITE_IDS),
            "unselected_suites": list(UNSELECTED_SUITES),
            "meaning": "only selected suites are evaluated; unselected suites are not evidence",
        },
        "source_fingerprint": expected_fingerprint,
        "children": children,
        "physical_mac_acceptance": "not_run",
        "parent_s2_42_status": "open",
    }


def _inside_work(path: Path) -> Path:
    root = ROOT.resolve()
    work = (root / "work").resolve()
    if work != root and root not in work.parents:
        raise AcceptanceError("repository work/ resolves outside the repository")
    resolved = path.resolve()
    if resolved != work and work not in resolved.parents:
        raise AcceptanceError("output directory must remain inside repository work/")
    return resolved


def _terminate_process_group(process: subprocess.Popen[str]) -> None:
    """Stop the child command and descendants that inherited its output pipes."""

    def kill_group() -> PermissionError | None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            return None
        except PermissionError as error:
            return error
        return None

    term_error = None
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    except PermissionError as error:
        term_error = error
    # Do not infer group completion from the leader: descendants can outlive it,
    # ignore TERM, and close or redirect the pipes used by communicate().
    interrupted = None
    kill_error = None
    try:
        time.sleep(2)
    except KeyboardInterrupt as error:
        interrupted = error
    finally:
        kill_error = kill_group()
    try:
        process.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        # Repeat the group kill in case an unusual child retained the pipes.
        kill_error = kill_error or kill_group()
        _reap_leader_after_cleanup_error(process)
    except KeyboardInterrupt as error:
        interrupted = interrupted or error
        kill_error = kill_error or kill_group()
        _reap_leader_after_cleanup_error(process)
    if term_error is not None:
        raise term_error
    if kill_error is not None:
        raise kill_error
    if interrupted is not None:
        raise interrupted


def _reap_leader_after_cleanup_error(process: subprocess.Popen[str]) -> None:
    try:
        process.kill()
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        pass
    for stream in (process.stdout, process.stderr):
        if stream is not None:
            stream.close()


def _run_child_command(command: list[str], *, cwd: Path, timeout: int
                       ) -> tuple[subprocess.CompletedProcess[str], bool]:
    """Run one suite in its own process group so timeout/interruption cannot leak descendants."""
    process = subprocess.Popen(
        command, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            _terminate_process_group(process)
        except OSError:
            _reap_leader_after_cleanup_error(process)
            raise
        stdout, stderr = process.communicate()
        stderr = (stderr or "") + f"\nchild command exceeded timeout of {timeout} seconds"
        return subprocess.CompletedProcess(command, 124, stdout, stderr), True
    except KeyboardInterrupt:
        try:
            _terminate_process_group(process)
        except OSError:
            _reap_leader_after_cleanup_error(process)
            raise
        raise
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr), False


def run(args: argparse.Namespace) -> int:
    run_id = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    output = _inside_work(args.output_dir if args.output_dir else
                          ROOT / "work/stage2-42-linux-five-suites" / run_id)
    output.mkdir(parents=True, exist_ok=False)
    summary_path = output / "combined-summary.json"
    current_fingerprint = stage1_oracle.source_fingerprint(ROOT)
    aggregate: dict[str, Any] = {
        "status": "failed",
        "selection": {
            "selected_suites": list(SUITE_IDS),
            "unselected_suites": list(UNSELECTED_SUITES),
            "meaning": "only selected suites are evaluated; unselected suites are not evidence",
        },
        "run_id": run_id,
        "source_fingerprint": current_fingerprint,
        "children": {},
        "physical_mac_acceptance": "not_run",
        "parent_s2_42_status": "open",
    }
    summary_path.write_bytes(json_bytes(aggregate))
    try:
        for suite in SUITE_IDS:
            child_output = output / suite
            child_output.mkdir()
            before = stage1_oracle.source_fingerprint(ROOT)
            if before != current_fingerprint:
                raise AcceptanceError("local sources changed before the combined run completed")
            command = child_command(suite, child_output, args.host, args.user, args.remote_root)
            try:
                process, timed_out = _run_child_command(command, cwd=ROOT, timeout=args.timeout)
            except KeyboardInterrupt:
                aggregate["children"][suite] = {
                    "suite_id": suite, "status": "failed", "error": "interrupted",
                    "physical_mac_acceptance": "not_run",
                }
                aggregate["error"] = f"{suite} was interrupted; its process group was terminated"
                summary_path.write_bytes(json_bytes(aggregate))
                return 130
            try:
                child = validate_child(suite, child_output, process.returncode, current_fingerprint)
            except (AcceptanceError, OSError, UnicodeError, json.JSONDecodeError,
                    KeyError, TypeError, ValueError) as error:
                aggregate["children"][suite] = {
                    "suite_id": suite, "status": "failed", "error": str(error),
                    "process_status": process.returncode,
                    "timed_out": timed_out,
                    "stdout_tail": process.stdout[-3000:], "stderr_tail": process.stderr[-3000:],
                    "physical_mac_acceptance": "not_run",
                }
                summary_path.write_bytes(json_bytes(aggregate))
                raise AcceptanceError(f"{suite} required check failed: {error}") from error
            child["process_status"] = process.returncode
            aggregate["children"][suite] = child
            summary_path.write_bytes(json_bytes(aggregate))
        if stage1_oracle.source_fingerprint(ROOT) != current_fingerprint:
            raise AcceptanceError("local source fingerprint changed during the combined run")
        aggregate.update(validate_aggregate(aggregate["children"], current_fingerprint))
        aggregate["run_id"] = run_id
        summary_path.write_bytes(json_bytes(aggregate))
        print(f"combined machine-readable evidence: {summary_path}")
        return 0
    except (AcceptanceError, OSError, subprocess.SubprocessError, UnicodeError,
            json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        aggregate["status"] = "failed"
        aggregate["error"] = str(error)
        summary_path.write_bytes(json_bytes(aggregate))
        print(f"combined S2-42 acceptance failed: {error}", file=sys.stderr)
        return 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True)
    parser.add_argument("--user", required=True)
    parser.add_argument("--remote-root", required=True)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--timeout", type=int, default=6 * 60 * 60)
    args = parser.parse_args(argv)
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", args.host) or not re.fullmatch(
            r"[A-Za-z_][A-Za-z0-9_.-]*", args.user):
        parser.error("--host and --user must be plain SSH host/user names")
    if (not args.remote_root.startswith("/") or args.remote_root == "/"
            or os.path.normpath(args.remote_root) != args.remote_root
            or any(ord(ch) < 32 or ch == ":" for ch in args.remote_root)):
        parser.error("--remote-root must be a normalized absolute POSIX checkout path")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
