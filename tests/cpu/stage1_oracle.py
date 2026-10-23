#!/usr/bin/env python3
"""Run fixture engines locally and compare them with the physical x86 oracle."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import posixpath
import re
import shlex
import subprocess
import sys
import tarfile
import time
import uuid
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CASES = (
    "tests/fixtures/x86/s0_09_mov_imm64.case.json",
    "tests/fixtures/x86/s0_09_add_overflow.case.json",
    "tests/fixtures/x86/s0_09_cmp_jz.case.json",
    "tests/fixtures/x86/s0_09_sub_jnz_loop.case.json",
    "tests/fixtures/x86/s1_06_add_signed_overflow.case.json",
    "tests/fixtures/x86/s1_06_sub_borrow.case.json",
    "tests/fixtures/x86/s1_06_cmp_signed_boundary.case.json",
    "tests/fixtures/x86/s1_06_jz_taken.case.json",
    "tests/fixtures/x86/s1_06_jz_not_taken.case.json",
    "tests/fixtures/x86/s1_06_mov_preserves_flags.case.json",
)
SOURCE_ROOTS = (
    "crates/od-isa",
    "crates/od-testkit",
    "crates/od-host",
    "crates/od-jit",
    "tools/runner",
    "tools/x86-runner",
)
SOURCE_SUFFIXES = {".rs", ".toml", ".h", ".c", ".S"}
REMOTE_FINGERPRINT_CODE = """import hashlib,pathlib
r=pathlib.Path('.')
roots=['Cargo.toml','Cargo.lock']+list(%r)
files=[]
for name in roots:
 p=r/name
 if p.is_file(): files.append(p)
 elif p.is_dir(): files.extend(x for x in p.rglob('*') if x.is_file() and not x.name.startswith('._') and x.suffix in %r)
h=hashlib.sha256()
for p in sorted(set(files),key=lambda x:x.as_posix()):
 h.update(p.as_posix().encode()); h.update(b'\\0'); h.update(p.read_bytes()); h.update(b'\\0')
print(h.hexdigest())
""" % (SOURCE_ROOTS, tuple(SOURCE_SUFFIXES))


class Stage1Error(RuntimeError):
    pass


def prepare_cases(case_names: list[str], output_dir: Path) -> list[tuple[Path, dict, str]]:
    """Freeze inputs before building or executing, and reject ambiguous identities."""
    fixture_paths: list[Path] = []
    for name in case_names:
        path = (ROOT / name).resolve()
        if ROOT not in path.parents:
            raise Stage1Error(f"fixture must be inside the repository: {name}")
        fixture_paths.extend(sorted(path.glob("*.case.json")) if path.is_dir() else [path])
    if not fixture_paths:
        raise Stage1Error("no fixture JSON files were selected")
    cases = []
    names: set[str] = set()
    ids: set[str] = set()
    frozen_dir = output_dir / "fixtures"
    frozen_dir.mkdir()
    for path in fixture_paths:
        if ROOT not in path.resolve().parents:
            raise Stage1Error(f"fixture resolves outside the repository: {path}")
        with path.open("rb") as stream:
            raw = stream.read(64 * 1024 + 1)
        if len(raw) > 64 * 1024:
            raise Stage1Error(f"fixture exceeds the v1 JSON size limit: {path}")
        fixture = json.loads(raw)
        if not isinstance(fixture, dict):
            raise Stage1Error(f"fixture must be a JSON object: {path}")
        case_id = fixture.get("case_id")
        if not isinstance(case_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", case_id):
            raise Stage1Error(f"invalid fixture case_id: {path}")
        if path.name in names or case_id in ids:
            raise Stage1Error(f"duplicate fixture filename or case_id: {path}")
        if not path.name.endswith(".case.json"):
            raise Stage1Error(f"fixture filename must end in .case.json: {path}")
        program = fixture.get("program")
        if not isinstance(program, dict) or not isinstance(program.get("bytes_hex"), str):
            raise Stage1Error(f"fixture program must contain hexadecimal bytes: {path}")
        code = bytes.fromhex(program["bytes_hex"])
        if hashlib.sha256(code).hexdigest() != program.get("sha256"):
            raise Stage1Error(f"{path} program digest does not match its bytes")
        names.add(path.name)
        ids.add(case_id)
        frozen = frozen_dir / path.name
        frozen.write_bytes(raw)
        cases.append((frozen, fixture, hashlib.sha256(raw).hexdigest()))
    return cases


def run(argv: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None,
        timeout: float = 60.0) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            argv,
            cwd=cwd,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise Stage1Error(f"could not run {argv[0]}: {error}") from error


def require_success(process: subprocess.CompletedProcess[str], label: str) -> str:
    if process.returncode != 0:
        detail = (process.stderr or process.stdout).strip()[-2000:]
        raise Stage1Error(f"{label} failed with exit {process.returncode}: {detail}")
    return process.stdout


def source_fingerprint(root: Path) -> str:
    files = [root / "Cargo.toml", root / "Cargo.lock"]
    for name in SOURCE_ROOTS:
        directory = root / name
        files.extend(
            path
            for path in directory.rglob("*")
            if path.is_file() and not path.name.startswith("._") and path.suffix in SOURCE_SUFFIXES
        )
    digest = hashlib.sha256()
    for path in sorted(set(files), key=lambda item: item.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix()
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def ssh_argv(target: str, remote_command: str) -> list[str]:
    return [
        "ssh",
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        target,
        remote_command,
    ]


def ssh_run(target: str, command: str, *, timeout: float = 60.0) -> str:
    return require_success(run(ssh_argv(target, command), timeout=timeout), "SSH command")


def parse_result(output: str, path: Path) -> dict:
    try:
        result = json.loads(output)
    except json.JSONDecodeError as error:
        raise Stage1Error(f"{path} did not contain one JSON result: {error}") from error
    if not isinstance(result, dict):
        raise Stage1Error(f"{path} result is not a JSON object")
    return result


def validate_result(result: dict, fixture: dict, input_digest: str, backend: str, label: str) -> None:
    required = {
        "schema_version": 1,
        "case_id": fixture["case_id"],
        "input_sha256": input_digest,
        "profile": fixture["profile"],
        "backend": backend,
        "status": "completed",
        "expected_exit": fixture["expected_exit"],
    }
    for key, value in required.items():
        if result.get(key) != value:
            raise Stage1Error(f"{label} returned {key}={result.get(key)!r}, expected {value!r}")
    build = result.get("build", {})
    if not build.get("revision") or not build.get("compiler") or not build.get("target"):
        raise Stage1Error(f"{label} omitted required build metadata")
    snapshot = result.get("snapshot", {})
    for field in ("gpr", "rip", "rflags"):
        if snapshot.get(field, {}).get("status") != "available":
            raise Stage1Error(f"{label} did not provide snapshot.{field}")


def compare(
    runner: Path,
    left: Path,
    right: Path,
    fixture_path: Path,
    output_path: Path,
    env: dict[str, str],
    *,
    process_runner=None,
) -> dict:
    process = (process_runner or run)(
        [
            str(runner),
            "--compare-results",
            str(left),
            str(right),
            "--case",
            str(fixture_path),
            "--json",
        ],
        env=env,
    )
    output_path.write_text(process.stdout)
    if process.returncode != 0:
        raise Stage1Error(
            f"comparison failed with exit {process.returncode}: "
            f"{(process.stderr or process.stdout).strip()[-2000:]}"
        )
    report = parse_result(process.stdout, output_path)
    if report.get("differences"):
        raise Stage1Error(f"{fixture_path.name} has {len(report['differences'])} differences")
    return report


def run_remote_batch(
    target: str,
    remote_root: str,
    runner: str,
    fixture_dir: str,
    cases: list[tuple[Path, dict, str]],
    output_dir: Path,
    *,
    process_runner=None,
) -> dict[str, dict]:
    batch_code = """import json,os,subprocess,sys
runner,fixture_dir=sys.argv[1:3]
results={}
for name in sorted(os.listdir(fixture_dir)):
 if not name.endswith('.case.json'): continue
 path=os.path.join(fixture_dir,name)
 p=subprocess.run([runner,'--fixture',path,'--json'],text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 results[name]={'returncode':p.returncode,'stdout':p.stdout,'stderr':p.stderr}
print(json.dumps(results,separators=(',',':')))
"""
    argv = ["python3", "-c", batch_code, runner, fixture_dir]
    remote_command = f"cd {shlex.quote(remote_root)} && {shlex.join(argv)}"
    process = (process_runner or run)(ssh_argv(target, remote_command), timeout=1800.0)
    (output_dir / "native-batch.stdout.json").write_text(process.stdout)
    (output_dir / "native-batch.stderr.log").write_text(process.stderr)
    require_success(process, "remote native batch")
    try:
        results = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise Stage1Error(f"remote native batch did not return JSON: {error}") from error
    if not isinstance(results, dict) or len(results) != len(cases):
        raise Stage1Error("remote native batch returned a different number of results")
    parsed = {}
    failures = []
    for path, fixture, _ in cases:
        record = results.get(path.name)
        if not isinstance(record, dict) or not isinstance(record.get("stdout"), str) or not isinstance(record.get("stderr"), str):
            raise Stage1Error(f"remote native batch omitted process output for {path.name}")
        case_dir = output_dir / fixture["case_id"]
        case_dir.mkdir()
        result_path = case_dir / "native_x86.json"
        result_path.write_text(record["stdout"])
        (case_dir / "native_x86.stderr.log").write_text(record["stderr"])
        (case_dir / "native_x86.process.json").write_text(json.dumps({"returncode": record.get("returncode")}))
        if record.get("returncode") != 0:
            failures.append(f"{path.name}: exit {record.get('returncode')}")
        else:
            parsed[path.name] = parse_result(record["stdout"], result_path)
    if failures:
        raise Stage1Error("remote native execution failed: " + "; ".join(failures))
    return parsed


def local_machine() -> dict:
    rustc = require_success(run(["rustc", "--version"]), "rustc --version").strip()
    cargo = require_success(run(["cargo", "--version"]), "cargo --version").strip()
    uname = require_success(run(["uname", "-a"]), "uname -a").strip()
    cpu = None
    if platform.system() == "Darwin":
        result = run(["sysctl", "-n", "machdep.cpu.brand_string"])
        if result.returncode == 0:
            cpu = result.stdout.strip()
    return {
        "host_os": platform.platform(),
        "host_arch": platform.machine(),
        "cpu_model": cpu,
        "kernel": uname,
        "page_size": getattr(os, "sysconf", lambda _: None)("SC_PAGE_SIZE"),
        "rustc": rustc,
        "cargo": cargo,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cases", nargs="*", help="fixture JSON paths; defaults to S0 and S1-06 cases")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--output-root", default="work/stage1-oracle")
    args = parser.parse_args()
    if args.repeats < 2:
        parser.error("--repeats must be at least 2 for Stage 1 acceptance")
    if any(ch in args.host for ch in "@:/\n\r") or any(ch in args.user for ch in "@:/\n\r"):
        parser.error("--host and --user must be plain SSH host/user names")
    if any(ch in args.remote_root for ch in "\n\r:") or not args.remote_root.startswith("/"):
        parser.error("--remote-root must be an absolute POSIX path without newlines or colon")

    target = f"{args.user}@{args.host}"
    run_id = time.strftime("stage1-%Y%m%d-%H%M%S", time.gmtime()) + "-" + uuid.uuid4().hex[:8]
    output_root = (ROOT / args.output_root).resolve()
    output_dir = output_root / run_id
    output_dir.mkdir(parents=True, exist_ok=False)
    manifest_path = output_dir / "manifest.json"
    manifest = {
        "run_id": run_id,
        "status": "running",
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "source_fingerprint": None,
        "host": args.host,
        "user": args.user,
        "remote_root": args.remote_root,
        "repeats": args.repeats,
        "machines": {},
        "fixtures": [],
        "commands": [],
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")

    try:
        cases = prepare_cases(args.cases or list(DEFAULT_CASES), output_dir)
        manifest["inputs"] = [
            {"case_id": fixture["case_id"], "fixture_path": path.relative_to(ROOT).as_posix(),
             "fixture_sha256": digest, "seed": fixture.get("seed")}
            for path, fixture, digest in cases
        ]
        manifest["reproduce_argv"] = [sys.executable, "tests/cpu/stage1_oracle.py",
            "--host", args.host, "--user", args.user, "--remote-root", args.remote_root,
            "--repeats", str(args.repeats), *[str(path) for path, _, _ in cases]]
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")

        fingerprint = source_fingerprint(ROOT)
        remote_cd = f"cd {shlex.quote(args.remote_root)}"
        remote_fingerprint = ssh_run(
            target,
            f"{remote_cd} && python3 -c {shlex.quote(REMOTE_FINGERPRINT_CODE)}",
        ).strip()
        if remote_fingerprint != fingerprint:
            raise Stage1Error(
                "local and remote runner source fingerprints differ: "
                f"{fingerprint} != {remote_fingerprint}"
            )
        revision = f"sha256:{fingerprint}"
        local_info = local_machine()
        remote_rustc = ssh_run(
            target,
            f"{remote_cd} && . \"$HOME/.cargo/env\" && rustc --version",
        ).strip()
        remote_cargo = ssh_run(
            target,
            f"{remote_cd} && . \"$HOME/.cargo/env\" && cargo --version",
        ).strip()
        manifest["source_fingerprint"] = fingerprint
        manifest["machines"] = {"local": local_info, "remote_rustc": remote_rustc, "remote_cargo": remote_cargo}

        remote_tests = (
            f"{remote_cd} && . \"$HOME/.cargo/env\" && "
            "cargo test --offline -p od-isa -p od-testkit -p od-runner -p od-x86-runner"
        )
        manifest["commands"].append({"host": target, "command": remote_tests})
        remote_test_output = ssh_run(target, remote_tests, timeout=1800.0)
        manifest["remote_test_summary"] = [
            line.strip()
            for line in remote_test_output.splitlines()
            if "test result:" in line or "error:" in line
        ][-12:]
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")

        local_env = os.environ.copy()
        local_env["OPEN_DESKTOP_GIT_REVISION"] = revision
        local_env["RUSTC_VERSION"] = local_info["rustc"]
        local_build = run(["cargo", "build", "--offline", "-p", "od-runner"], env=local_env)
        manifest["commands"].append({"host": "local", "argv": ["cargo", "build", "--offline", "-p", "od-runner"]})
        require_success(local_build, "local od-runner build")

        remote_build = (
            f"{remote_cd} && . \"$HOME/.cargo/env\" && "
            f"OPEN_DESKTOP_GIT_REVISION={shlex.quote(revision)} "
            f"RUSTC_VERSION={shlex.quote(remote_rustc)} "
            "cargo build --offline -p od-x86-runner"
        )
        manifest["commands"].append({"host": target, "command": remote_build})
        ssh_run(target, remote_build, timeout=300.0)

        remote_run_dir = posixpath.join(args.remote_root, "work", "stage1-oracle", run_id)
        remote_fixture_dir = posixpath.join(remote_run_dir, "fixtures")
        remote_setup = f"mkdir -p -- {shlex.quote(remote_fixture_dir)}"
        ssh_run(target, remote_setup)
        archive_path = output_dir / "fixtures.tar.gz"
        with tarfile.open(archive_path, "w:gz") as archive:
            for path, _, _ in cases:
                archive.add(path, arcname=path.name, recursive=False)
        remote_archive = posixpath.join(remote_run_dir, "fixtures.tar.gz")
        destination = f"{target}:{remote_archive}"
        upload = run(
            ["scp", "-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", str(archive_path), destination],
            timeout=120.0,
        )
        manifest["commands"].append({"host": "local", "argv": ["scp", str(archive_path), destination]})
        require_success(upload, "fixture archive upload")
        unpack = (
            f"tar -xzf {shlex.quote(remote_archive)} -C {shlex.quote(remote_fixture_dir)}"
        )
        ssh_run(target, unpack)

        local_runner = ROOT / "target/debug/od-runner"
        remote_runner = posixpath.join(args.remote_root, "target/debug/od-x86-runner")
        for repeat in range(1, args.repeats + 1):
            repeat_dir = output_dir / f"repeat-{repeat:02d}"
            repeat_dir.mkdir()
            native_results = run_remote_batch(
                target,
                args.remote_root,
                remote_runner,
                remote_fixture_dir,
                cases,
                repeat_dir,
            )
            manifest["commands"].append(
                {"host": target, "command": f"remote native batch for {len(cases)} fixtures"}
            )
            for fixture_path, fixture, input_digest in cases:
                case_dir = repeat_dir / fixture["case_id"]
                local_results: dict[str, dict] = {}
                result_paths: dict[str, Path] = {}
                for engine in ("interpreter", "jit"):
                    result_path = case_dir / f"{engine}.json"
                    process = run(
                        [str(local_runner), "--fixture", str(fixture_path), "--engine", engine, "--json"],
                        env=local_env,
                        timeout=max(60.0, fixture["wall_timeout_ms"] / 1000 + 30.0),
                    )
                    result_path.write_text(process.stdout)
                    (case_dir / f"{engine}.stderr.log").write_text(process.stderr)
                    (case_dir / f"{engine}.process.json").write_text(json.dumps({"returncode": process.returncode}))
                    result = parse_result(process.stdout, result_path)
                    validate_result(result, fixture, input_digest, engine, f"local {engine}")
                    if process.returncode != 0:
                        raise Stage1Error(f"local {engine} exited {process.returncode}: {process.stderr[-1000:]}")
                    local_results[engine] = result
                    result_paths[engine] = result_path

                native_path = case_dir / "native_x86.json"
                native = native_results.get(fixture_path.name)
                if not isinstance(native, dict):
                    raise Stage1Error(f"remote native batch omitted {fixture_path.name}")
                native_path.write_text(json.dumps(native, indent=2) + "\n")
                validate_result(native, fixture, input_digest, "native_x86", "remote native x86")
                if native["build"]["revision"] != revision:
                    raise Stage1Error("remote result build revision does not match the source fingerprint")
                for engine, result in local_results.items():
                    if result["build"]["revision"] != revision:
                        raise Stage1Error(f"local {engine} result build revision does not match the source fingerprint")
                    report_path = case_dir / f"{engine}-vs-native.json"
                    report = compare(local_runner, result_paths[engine], native_path, fixture_path, report_path, local_env)
                    if not report.get("case_id") == fixture["case_id"]:
                        raise Stage1Error(f"{engine}/native comparison returned a different case")

                previous = output_dir / f"repeat-{repeat - 1:02d}" / fixture["case_id"] / "native_x86.json"
                if repeat > 1:
                    repeat_report = case_dir / "native-repeat-compare.json"
                    compare(local_runner, previous, native_path, fixture_path, repeat_report, local_env)

                entry = {
                    "repeat": repeat,
                    "case_id": fixture["case_id"],
                    "fixture_path": fixture_path.relative_to(ROOT).as_posix(),
                    "fixture_sha256": input_digest,
                    "fixture_schema_version": fixture["schema_version"],
                    "result_schema_version": native["schema_version"],
                    "build": {"local_interpreter": local_results["interpreter"]["build"],
                              "local_jit": local_results["jit"]["build"],
                              "native_x86": native["build"]},
                    "machines": {"local_interpreter": local_results["interpreter"]["machine"],
                                 "local_jit": local_results["jit"]["machine"],
                                 "native_x86": native["machine"]},
                    "differences": {"interpreter_vs_native": 0, "jit_vs_native": 0},
                }
                manifest["fixtures"].append(entry)
                if len(manifest["fixtures"]) % 25 == 0:
                    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
                print(f"repeat {repeat}/{args.repeats}: {fixture['case_id']} matched interpreter and JIT")

        manifest["status"] = "passed"
        manifest["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"manifest: {manifest_path}")
        return 0
    except (Stage1Error, OSError, KeyError, ValueError, json.JSONDecodeError) as error:
        manifest["status"] = "failed"
        manifest["error"] = str(error)
        manifest["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"stage1 oracle failed; manifest: {manifest_path}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
