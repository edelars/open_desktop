#!/usr/bin/env python3
"""Compare selected v2 CPU fixtures with physical Linux x86 native, without v2 JIT."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import posixpath
import re
import shlex
import sys
import tarfile
import time
import uuid
from pathlib import Path

import stage1_oracle as oracle

ROOT = Path(__file__).resolve().parents[2]
PROFILE = "s2-long-integer-v1"
MAX_FIXTURE_BYTES = 512 * 1024
MAX_RESULT_BYTES = 256 * 1024
DEFAULT_CASES = tuple(
    f"tests/fixtures/x86/stage2/hw_{name}.case.json"
    for name in ("mov_load_add_store", "mov_width_aliasing", "mov_rex_rip_relative")
)
Stage2Error = oracle.Stage1Error


def strict_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Stage2Error(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_json(raw: bytes | str):
    def nonfinite(value):
        raise Stage2Error(f"non-finite JSON number: {value}")
    return json.loads(raw, object_pairs_hook=strict_object, parse_constant=nonfinite)


def prepare_cases(names: list[str], output: Path) -> list[tuple[Path, dict, str]]:
    """Freeze exact bytes; Rust readers remain the authority for the full schema."""
    selected = []
    for name in names:
        path = (ROOT / name).resolve()
        if ROOT not in path.parents:
            raise Stage2Error(f"fixture must resolve inside the repository: {name}")
        selected.extend(sorted(path.glob("*.case.json")) if path.is_dir() else [path])
    if not selected:
        raise Stage2Error("no fixture JSON files were selected")
    prepared, filenames, ids = [], set(), set()
    for path in selected:
        if ROOT not in path.resolve().parents:
            raise Stage2Error(f"fixture resolves outside the repository: {path}")
        if not re.fullmatch(r"[A-Za-z0-9_-]+\.case\.json", path.name):
            raise Stage2Error(f"unsafe fixture filename: {path.name}")
        with path.open("rb") as stream:
            raw = stream.read(MAX_FIXTURE_BYTES + 1)
        if len(raw) > MAX_FIXTURE_BYTES:
            raise Stage2Error(f"fixture exceeds schema v2 JSON limit: {path}")
        value = read_json(raw)
        if not isinstance(value, dict):
            raise Stage2Error(f"fixture must be a JSON object: {path}")
        case_id = value.get("case_id")
        if not isinstance(case_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", case_id):
            raise Stage2Error(f"unsafe fixture case_id: {path}")
        if type(value.get("schema_version")) is not int or value["schema_version"] != 2:
            raise Stage2Error(f"schema v2 fixture required: {path}")
        if value.get("profile") != PROFILE:
            raise Stage2Error(f"unsupported S2 fixture profile: {path}")
        if type(value.get("wall_timeout_ms")) is not int or not 1 <= value["wall_timeout_ms"] <= 30_000:
            raise Stage2Error(f"invalid fixture wall timeout: {path}")
        if path.name in filenames or case_id in ids:
            raise Stage2Error(f"duplicate fixture filename or case_id: {path}")
        filenames.add(path.name)
        ids.add(case_id)
        prepared.append((path.name, value, hashlib.sha256(raw).hexdigest(), raw))
    frozen = output / "fixtures"
    frozen.mkdir()
    cases = []
    for name, value, digest, raw in prepared:
        path = frozen / name
        path.write_bytes(raw)
        cases.append((path, value, digest))
    return cases


def validate_result(result: dict, fixture: dict, digest: str, backend: str,
                    revision: str, compiler: str, label: str) -> None:
    expected = {"schema_version": 2, "case_id": fixture["case_id"],
                "input_sha256": digest, "profile": PROFILE, "backend": backend}
    for key, value in expected.items():
        if result.get(key) != value:
            raise Stage2Error(f"{label}: invalid {key}: {result.get(key)!r}, expected {value!r}")
    outcome = result.get("outcome", {})
    if not isinstance(outcome, dict) or outcome.get("kind") != fixture["expected_exit"]:
        raise Stage2Error(f"{label}: incomplete or unexpected outcome: {outcome!r}")
    build = result.get("build", {})
    if not isinstance(build, dict) or build.get("revision") != revision or build.get("compiler") != compiler:
        raise Stage2Error(f"{label}: build fingerprint or compiler does not match the recorded build")
    if not build.get("target") or not build.get("profile"):
        raise Stage2Error(f"{label}: missing build target/profile")
    if backend == "native_x86":
        machine = result.get("machine", {})
        if machine.get("host_os") != "linux" or machine.get("host_arch") != "x86_64":
            raise Stage2Error(f"{label}: physical oracle must report Linux x86_64")
    observed = result.get("snapshot", {})
    if not isinstance(observed, dict) or observed.get("status") != "available":
        raise Stage2Error(f"{label}: full architectural snapshot unavailable")
    snapshot = observed.get("value", {})
    state = snapshot.get("state", {})
    if state.get("mode") != "long64" or len(state.get("gpr", [])) != 16:
        raise Stage2Error(f"{label}: full long-mode GPR set unavailable")
    if snapshot.get("exception", {}).get("status") != "available":
        raise Stage2Error(f"{label}: exception observation unavailable")
    if len(snapshot.get("memory", [])) != len(fixture["memory"]):
        raise Stage2Error(f"{label}: incomplete memory snapshot")
    # The existing Rust comparator subsequently enforces all strict schema,
    # layout, flags-definedness, exact-value and expected-exit constraints.


def validate_comparison(report: dict, fixture: dict, digest: str) -> None:
    if (report.get("schema_version") != 2 or report.get("case_id") != fixture["case_id"]
            or report.get("input_sha256") != digest or report.get("matches") is not True
            or report.get("differences") != []):
        raise Stage2Error(f"{fixture['case_id']}: invalid or mismatching comparison report")


class Evidence:
    """Persist every orchestration subprocess before its success is interpreted."""
    def __init__(self, directory: Path, manifest: dict):
        self.directory, self.manifest = directory, manifest
        (directory / "commands").mkdir()

    def save(self):
        (self.directory / "manifest.json").write_text(json.dumps(self.manifest, indent=2) + "\n")

    def run(self, argv, **kwargs):
        index = len(self.manifest["commands"]) + 1
        prefix = self.directory / "commands" / f"{index:04d}"
        record = {"argv": argv, "cwd": str(kwargs.get("cwd", oracle.ROOT)),
                  "stdout": str(prefix.relative_to(self.directory)) + ".stdout.log",
                  "stderr": str(prefix.relative_to(self.directory)) + ".stderr.log",
                  "returncode": None}
        environment = kwargs.get("env") or {}
        record["build_environment"] = {key: environment[key] for key in
            ("OPEN_DESKTOP_GIT_REVISION", "RUSTC_VERSION") if key in environment}
        self.manifest["commands"].append(record)
        self.save()
        stdout = stderr = ""
        try:
            process = oracle.run(argv, **kwargs)
            stdout, stderr = process.stdout, process.stderr
            record["returncode"] = process.returncode
            return process
        except Stage2Error as error:
            cause = error.__cause__
            stdout = getattr(cause, "stdout", "") or ""
            stderr = getattr(cause, "stderr", "") or ""
            record["error"] = str(error)
            raise
        finally:
            for suffix, content in [("stdout", stdout), ("stderr", stderr)]:
                if isinstance(content, bytes):
                    content = content.decode("utf-8", errors="replace")
                prefix.with_suffix(f".{suffix}.log").write_text(content)
            self.save()

    def ssh(self, target: str, command: str, label: str, timeout=60.0) -> str:
        return oracle.require_success(self.run(oracle.ssh_argv(target, command), timeout=timeout), label)


def compare(evidence: Evidence, runner: Path, left: Path, right: Path,
            fixture_path: Path, fixture: dict, digest: str, report_path: Path, env: dict) -> dict:
    report = oracle.compare(runner, left, right, fixture_path, report_path, env,
                            process_runner=evidence.run)
    validate_comparison(report, fixture, digest)
    return report


def legacy_evidence(path: Path, fingerprint: str) -> dict:
    raw = path.read_bytes()
    value = read_json(raw)
    inputs, entries = value.get("inputs", []), value.get("fixtures", [])
    repeats = value.get("repeats", 0)
    if (value.get("status") != "passed" or value.get("source_fingerprint") != fingerprint
            or type(repeats) is not int or repeats < 2 or len(inputs) < 1010):
        raise Stage2Error("legacy manifest must pass >=1010 inputs twice on the same source fingerprint")
    ids = [item.get("case_id") for item in inputs]
    if len(set(ids)) != len(ids) or not all(isinstance(item, str) for item in ids):
        raise Stage2Error("legacy manifest has ambiguous input identities")
    seen = set()
    for entry in entries:
        key = (entry.get("case_id"), entry.get("repeat"))
        if key in seen or key[0] not in ids or type(key[1]) is not int or not 1 <= key[1] <= repeats:
            raise Stage2Error("legacy manifest has duplicate or unexpected repeat records")
        if entry.get("differences") != {"interpreter_vs_native": 0, "jit_vs_native": 0}:
            raise Stage2Error("legacy manifest did not pass every interpreter/JIT/native comparison")
        seen.add(key)
    if len(seen) != len(inputs) * repeats:
        raise Stage2Error("legacy manifest omitted repeat results")
    return {"status": "passed", "path": str(path), "sha256": hashlib.sha256(raw).hexdigest(),
            "inputs": len(inputs), "repeats": repeats, "source_fingerprint": fingerprint}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cases", nargs="*", help="explicit v2 files/directories; default: three hardware fixtures")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--output-root", default="work/stage2-oracle")
    parser.add_argument("--stage1-manifest", help="optional passed 1010-case v1 oracle manifest from the same sources")
    args = parser.parse_args(argv)
    if args.repeats < 2:
        parser.error("--repeats must be at least 2")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", args.host) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.-]*", args.user):
        parser.error("--host and --user must be plain SSH host/user names")
    if (not args.remote_root.startswith("/") or args.remote_root == "/"
            or posixpath.normpath(args.remote_root) != args.remote_root
            or any(ord(ch) < 32 or ch == ":" for ch in args.remote_root)):
        parser.error("--remote-root must be a normalized absolute POSIX checkout path")
    output_root = (ROOT / args.output_root).resolve()
    work = (ROOT / "work").resolve()
    if ROOT not in work.parents or (output_root != work and work not in output_root.parents):
        parser.error("--output-root must resolve inside repository work/")
    run_id = time.strftime("stage2-%Y%m%d-%H%M%S", time.gmtime()) + "-" + uuid.uuid4().hex[:8]
    output = output_root / run_id
    output.mkdir(parents=True)
    manifest = {"run_id": run_id, "status": "running",
                "scope": "Selected v2 CPU fixtures: interpreter vs physical Linux x86 native; no v2 JIT",
                "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "target": f"{args.user}@{args.host}", "remote_root": args.remote_root,
                "repeats": args.repeats, "commands": [], "inputs": [], "fixtures": [],
                "legacy_regression": {"status": "not_provided", "required_for_full_s2_09": True}}
    evidence = Evidence(output, manifest)
    evidence.save()
    try:
        cases = prepare_cases(args.cases or list(DEFAULT_CASES), output)
        manifest["inputs"] = [{"case_id": fixture["case_id"], "filename": path.name,
            "fixture_path": str(path.relative_to(ROOT)), "fixture_sha256": digest,
            "seed": fixture.get("seed"), "schema_version": 2, "profile": PROFILE}
            for path, fixture, digest in cases]
        manifest["reproduce_argv"] = [sys.executable, "tests/cpu/stage2_oracle.py", "--host", args.host,
            "--user", args.user, "--remote-root", args.remote_root, "--repeats", str(args.repeats),
            *(["--stage1-manifest", args.stage1_manifest] if args.stage1_manifest else []),
            *[str(path.relative_to(ROOT)) for path, _, _ in cases]]
        manifest["harness_sha256"] = {name: hashlib.sha256((ROOT / "tests/cpu" / name).read_bytes()).hexdigest()
                                      for name in ("stage1_oracle.py", "stage2_oracle.py")}
        fingerprint = oracle.source_fingerprint(ROOT)
        manifest["source_fingerprint"] = fingerprint
        evidence.save()
        if args.stage1_manifest:
            manifest["legacy_regression"] = legacy_evidence((ROOT / args.stage1_manifest).resolve(), fingerprint)
        target = manifest["target"]
        remote_cd = f"cd {shlex.quote(args.remote_root)}"
        fingerprint_command = f"{remote_cd} && python3 -c {shlex.quote(oracle.REMOTE_FINGERPRINT_CODE)}"
        if evidence.ssh(target, fingerprint_command, "remote fingerprint").strip() != fingerprint:
            raise Stage2Error("local and remote runner source fingerprints differ")
        revision = f"sha256:{fingerprint}"
        local_info = oracle.local_machine()
        remote_rustc = evidence.ssh(target, f'{remote_cd} && . "$HOME/.cargo/env" && rustc --version', "remote rustc").strip()
        remote_cargo = evidence.ssh(target, f'{remote_cd} && . "$HOME/.cargo/env" && cargo --version', "remote cargo").strip()
        remote_uname = evidence.ssh(target, "uname -a", "remote machine").strip()
        manifest["machines"] = {"local": local_info, "remote_rustc": remote_rustc,
                                "remote_cargo": remote_cargo, "remote_uname": remote_uname}
        env = os.environ.copy()
        env["OPEN_DESKTOP_GIT_REVISION"], env["RUSTC_VERSION"] = revision, local_info["rustc"]
        oracle.require_success(evidence.run(["cargo", "build", "--offline", "-p", "od-runner"], env=env), "local runner build")
        build = (f'{remote_cd} && . "$HOME/.cargo/env" && '
                 f"OPEN_DESKTOP_GIT_REVISION={shlex.quote(revision)} RUSTC_VERSION={shlex.quote(remote_rustc)} "
                 "cargo build --offline -p od-x86-runner")
        oracle.require_success(evidence.run(oracle.ssh_argv(target, build), timeout=300.0), "remote native build")
        if oracle.source_fingerprint(ROOT) != fingerprint:
            raise Stage2Error("local sources changed while building the selected suite")
        remote_run = posixpath.join(args.remote_root, "work/stage2-oracle", run_id)
        remote_fixtures = posixpath.join(remote_run, "fixtures")
        evidence.ssh(target, f"mkdir -p -- {shlex.quote(remote_fixtures)}", "remote fixture directory")
        archive_path = output / "fixtures.tar.gz"
        with tarfile.open(archive_path, "w:gz") as archive:
            for path, _, digest in cases:
                if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                    raise Stage2Error("frozen fixture changed before upload")
                archive.add(path, arcname=path.name, recursive=False)
        manifest["fixture_archive_sha256"] = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        evidence.save()
        remote_archive = posixpath.join(remote_run, "fixtures.tar.gz")
        oracle.require_success(evidence.run(["scp", "-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
            str(archive_path), f"{target}:{remote_archive}"], timeout=120.0), "frozen fixture upload")
        evidence.ssh(target, f"tar -xzf {shlex.quote(remote_archive)} -C {shlex.quote(remote_fixtures)}", "remote fixture extraction")
        runner = ROOT / "target/debug/od-runner"
        remote_runner = posixpath.join(args.remote_root, "target/debug/od-x86-runner")
        for repeat in range(1, args.repeats + 1):
            repeat_dir = output / f"repeat-{repeat:02d}"
            repeat_dir.mkdir()
            natives = oracle.run_remote_batch(target, args.remote_root, remote_runner, remote_fixtures,
                cases, repeat_dir, process_runner=evidence.run)
            for path, fixture, digest in cases:
                manifest["active_case"] = {"case_id": fixture["case_id"], "repeat": repeat}
                evidence.save()
                if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                    raise Stage2Error("frozen fixture changed during execution")
                directory = repeat_dir / fixture["case_id"]
                native_path = directory / "native_x86.json"
                if native_path.stat().st_size > MAX_RESULT_BYTES:
                    raise Stage2Error("native result exceeds v2 JSON limit")
                native = natives[path.name]
                validate_result(native, fixture, digest, "native_x86", revision, remote_rustc, "native result")
                argv = [str(runner), "--fixture", str(path), "--engine", "interpreter", "--json"]
                process = evidence.run(argv, env=env, timeout=max(60.0, fixture["wall_timeout_ms"] / 1000 + 30))
                local_path = directory / "interpreter.json"
                local_path.write_text(process.stdout)
                (directory / "interpreter.stderr.log").write_text(process.stderr)
                (directory / "interpreter.process.json").write_text(json.dumps({"returncode": process.returncode}))
                oracle.require_success(process, "local interpreter")
                if len(process.stdout.encode()) > MAX_RESULT_BYTES:
                    raise Stage2Error("interpreter result exceeds v2 JSON limit")
                local = oracle.parse_result(process.stdout, local_path)
                validate_result(local, fixture, digest, "interpreter", revision, local_info["rustc"], "interpreter result")
                compare(evidence, runner, local_path, native_path, path, fixture, digest,
                        directory / "interpreter-vs-native.json", env)
                if repeat > 1:
                    for engine in ("interpreter", "native_x86"):
                        previous = output / f"repeat-{repeat - 1:02d}" / fixture["case_id"] / f"{engine}.json"
                        compare(evidence, runner, previous, directory / f"{engine}.json", path, fixture, digest,
                                directory / f"{engine}-repeat-compare.json", env)
                manifest["fixtures"].append({"repeat": repeat, "case_id": fixture["case_id"],
                    "fixture_sha256": digest, "seed": fixture.get("seed"), "schema_version": 2,
                    "build": {"interpreter": local["build"], "native_x86": native["build"]},
                    "machines": {"interpreter": local["machine"], "native_x86": native["machine"]},
                    "snapshot_fields": {engine: {"gpr": 16, "rip": True, "rflags": True,
                        "rflags_defined": value["snapshot"]["value"]["rflags_defined"],
                        "memory_regions": len(value["snapshot"]["value"]["memory"]),
                        "exception": value["snapshot"]["value"]["exception"]["status"],
                        "instructions_executed": value["instructions_executed"]["status"]}
                        for engine, value in [("interpreter", local), ("native_x86", native)]},
                    "differences": {"interpreter_vs_native": 0},
                    "stable_repeat": repeat > 1})
                evidence.save()
                print(f"repeat {repeat}/{args.repeats}: {fixture['case_id']} matched interpreter/native")
        if (oracle.source_fingerprint(ROOT) != fingerprint
                or evidence.ssh(target, fingerprint_command, "final remote fingerprint").strip() != fingerprint):
            raise Stage2Error("runner sources changed during the selected suite")
        manifest.pop("active_case", None)
        manifest["status"] = "passed"
        manifest["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        evidence.save()
        print(f"manifest: {output / 'manifest.json'}")
        return 0
    except (Stage2Error, OSError, KeyError, TypeError, ValueError) as error:
        manifest["status"], manifest["error"] = "failed", str(error)
        manifest["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        evidence.save()
        print(f"stage2 oracle failed; manifest: {output / 'manifest.json'}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
