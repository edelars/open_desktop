#!/usr/bin/env python3
"""Compare selected v2 CPU fixtures with physical Linux x86 native, without v2 JIT."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import posixpath
import re
import selectors
import shlex
import signal
import subprocess
import sys
import tarfile
import time
import uuid
from pathlib import Path

import stage1_oracle as oracle

ROOT = Path(__file__).resolve().parents[2]
PROFILE = "s2-long-integer-v1"
XORPD_PROFILE = "s2-long-native-xorpd-xmm0-xmm1-v1"
XORPD_CORPUS_ROOT = Path("work/stage2/s2-42a-xorpd")
XORPD_SEEDS = (0x243F6A8885A308D3, 0x13198A2E03707344,
               0xA4093822299F31D0, 0x082EFA98EC4E6C89)
XORPD_CASES_PER_SEED = 256
XORPD_MANIFEST_VERSION = 1
XORPD_GENERATOR_VERSION = "od-testkit-native-xorpd-v1"
XORPD_HAND_MANIFEST = Path("tests/fixtures/x86/stage2/s2_42p_xorpd_hand_controls.manifest.json")
XORPD_HAND_MANIFEST_SHA256 = "a841b2a9f6715ab56d28e2563efbb983369eee129176d0263c41a159ea1db517"
XORPD_HAND_CONTROL_COUNT = 6
XORPD_HAND_CONTROL_DIR = Path("tests/fixtures/x86/stage2/s2_42p_xorpd_hand")
XORPD_HAND_EXPECTED = {
    "s2_40b_xorpd_xmm0_xmm1_asymmetric_a": "f0e0d0c0b0a090807060504030201000",
    "s2_40b_xorpd_xmm0_xmm1_asymmetric_b": "5aa50ff0e1d2c3b4e11ec3f0f00f2d5a",
    "s2_42p_xorpd_zero_ones": "ffffffffffffffffffffffffffffffff",
    "s2_42p_xorpd_ones_zero": "ffffffffffffffffffffffffffffffff",
    "s2_42p_xorpd_alternating": "ffffffffffffffffffffffffffffffff",
    "s2_42p_xorpd_equal": "00000000000000000000000000000000",
}
MAX_FIXTURE_BYTES = 512 * 1024
MAX_RESULT_BYTES = 256 * 1024
POST_TIMEOUT_DRAIN_SECONDS = 0.25
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


def prepare_cases(names: list[str], output: Path, profile: str = PROFILE) -> list[tuple[Path, dict, str]]:
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
        if value.get("profile") != profile:
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


def _xorpd_manifest(path: Path) -> tuple[dict, bytes]:
    raw = path.read_bytes()
    value = read_json(raw)
    required = {"manifest_version", "generator_version", "profile", "seeds",
                "sequence", "case_digests", "corpus_digest"}
    if not isinstance(value, dict) or set(value) != required:
        raise Stage2Error("XORPD corpus manifest has missing or extra fields")
    expected_seeds = [f"0x{seed:016x}" for seed in XORPD_SEEDS]
    digests = value["case_digests"]
    if (type(value["manifest_version"]) is not int or value["manifest_version"] != XORPD_MANIFEST_VERSION
            or value["generator_version"] != XORPD_GENERATOR_VERSION
            or value["profile"] != XORPD_PROFILE or value["seeds"] != expected_seeds
            or value["sequence"] != f"0..{XORPD_CASES_PER_SEED} (exclusive)"
            or not isinstance(digests, list) or len(digests) != len(XORPD_SEEDS) * XORPD_CASES_PER_SEED
            or any(not isinstance(item, str) or not re.fullmatch(r"[0-9a-f]{64}", item) for item in digests)):
        raise Stage2Error("XORPD corpus manifest has an unsupported profile or shape")
    corpus_digest = hashlib.sha256("\n".join(digests).encode()).hexdigest()
    if value["corpus_digest"] != corpus_digest:
        raise Stage2Error("XORPD corpus manifest digest does not match its case hashes")
    return value, raw


def prepare_xorpd_corpus(corpus_root: Path, output: Path) -> tuple[list[tuple[Path, dict, str]], dict, str]:
    """Freeze the recorded S2-42a corpus; never generate or rewrite its manifest."""
    manifest, manifest_raw = _xorpd_manifest(corpus_root / "manifest.json")
    source_dir = corpus_root / "corpus"
    if source_dir.is_symlink() or not source_dir.is_dir():
        raise Stage2Error("XORPD corpus directory must be a real directory")
    expected_names = []
    for seed in XORPD_SEEDS:
        for sequence in range(XORPD_CASES_PER_SEED):
            case_id = f"s2_42a_xorpd_{seed:016x}_{sequence:04x}"
            expected_names.append(f"{case_id}.case.json")
    try:
        entries = list(source_dir.iterdir())
        if any(item.is_symlink() for item in entries):
            raise Stage2Error("XORPD corpus must not contain symlinks")
        actual_names = sorted(item.name for item in entries if item.is_file())
    except OSError as error:
        raise Stage2Error(f"missing or unreadable XORPD corpus directory: {error}") from error
    if actual_names != sorted(expected_names):
        raise Stage2Error("XORPD corpus has missing or extra case files")

    frozen = output / "fixtures"
    frozen.mkdir()
    cases, seen_inputs = [], set()
    for index, name in enumerate(expected_names):
        path = source_dir / name
        raw = path.read_bytes()
        if len(raw) > MAX_FIXTURE_BYTES:
            raise Stage2Error(f"fixture exceeds schema v2 JSON limit: {path}")
        digest = hashlib.sha256(raw).hexdigest()
        if digest != manifest["case_digests"][index]:
            raise Stage2Error(f"XORPD corpus case hash mismatch: {name}")
        fixture = read_json(raw)
        case_id = name.removesuffix(".case.json")
        seed = f"0x{XORPD_SEEDS[index // XORPD_CASES_PER_SEED]:016x}"
        if (not isinstance(fixture, dict) or fixture.get("case_id") != case_id
                or fixture.get("profile") != XORPD_PROFILE or fixture.get("schema_version") != 2
                or fixture.get("seed") != seed):
            raise Stage2Error(f"XORPD corpus case identity, schema, seed or profile mismatch: {name}")
        initial_state = fixture.get("initial_state")
        if not isinstance(initial_state, dict):
            raise Stage2Error(f"XORPD corpus case lacks initial state: {name}")
        sse = initial_state.get("sse", {})
        xmm = sse.get("xmm") if isinstance(sse, dict) else None
        if not isinstance(xmm, list) or len(xmm) != 16 or any(
                not isinstance(reg, str) or not re.fullmatch(r"[0-9a-f]{32}", reg) for reg in xmm):
            raise Stage2Error(f"XORPD corpus case lacks complete XMM input: {name}")
        input_key = tuple(xmm)
        if input_key in seen_inputs:
            raise Stage2Error(f"XORPD corpus contains duplicate XMM inputs: {name}")
        seen_inputs.add(input_key)
        frozen_path = frozen / name
        frozen_path.write_bytes(raw)
        cases.append((frozen_path, fixture, digest))
    (output / "xorpd-corpus-manifest.json").write_bytes(manifest_raw)
    return cases, manifest, hashlib.sha256(manifest_raw).hexdigest()


def prepare_xorpd_hand_controls(output: Path) -> tuple[list[tuple[Path, dict, str]], dict, str]:
    """Freeze the separately pinned six handwritten XORPD controls."""
    manifest_path = ROOT / XORPD_HAND_MANIFEST
    raw = manifest_path.read_bytes()
    manifest_sha256 = hashlib.sha256(raw).hexdigest()
    if manifest_sha256 != XORPD_HAND_MANIFEST_SHA256:
        raise Stage2Error("XORPD handwritten control manifest digest mismatch")
    value = read_json(raw)
    required = {"manifest_version", "profile", "control_count", "controls", "control_set_digest"}
    if not isinstance(value, dict) or set(value) != required:
        raise Stage2Error("XORPD handwritten control manifest has missing or extra fields")
    controls = value["controls"]
    if (type(value["manifest_version"]) is not int or value["manifest_version"] != 1
            or value["profile"] != XORPD_PROFILE
            or type(value["control_count"]) is not int or value["control_count"] != XORPD_HAND_CONTROL_COUNT
            or not isinstance(controls, list) or len(controls) != XORPD_HAND_CONTROL_COUNT):
        raise Stage2Error("XORPD handwritten control manifest has an unsupported version or shape")
    expected_digest = hashlib.sha256("\n".join(
        f"{item['case_id']}:{item['file']}:{item['sha256']}:{item['expected_xmm0']}"
        for item in controls if isinstance(item, dict)
    ).encode()).hexdigest()
    if value["control_set_digest"] != expected_digest:
        raise Stage2Error("XORPD handwritten control set digest mismatch")

    expected_ids = tuple(XORPD_HAND_EXPECTED)
    actual_ids = tuple(item.get("case_id") if isinstance(item, dict) else None for item in controls)
    if actual_ids != expected_ids:
        raise Stage2Error("XORPD handwritten control identity or order mismatch")
    expected_fields = {"case_id", "file", "sha256", "origin", "expected_xmm0", "input_xmm0", "input_xmm1"}
    expected_names = {f"{case_id}.case.json" for case_id in expected_ids
                      if case_id.startswith("s2_42p_")}
    hand_dir = ROOT / XORPD_HAND_CONTROL_DIR
    if hand_dir.is_symlink() or not hand_dir.is_dir():
        raise Stage2Error("XORPD handwritten control directory must be a real directory")
    entries = list(hand_dir.iterdir())
    if any(item.is_symlink() for item in entries) or {item.name for item in entries} != expected_names:
        raise Stage2Error("XORPD handwritten control directory has missing or extra files")

    frozen = output / "hand-controls"
    frozen.mkdir(parents=True)
    cases, seen_inputs = [], set()
    for item in controls:
        if not isinstance(item, dict) or set(item) != expected_fields:
            raise Stage2Error("XORPD handwritten control entry has missing or extra fields")
        case_id = item["case_id"]
        if (item["expected_xmm0"] != XORPD_HAND_EXPECTED[case_id]
                or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])
                or not re.fullmatch(r"[0-9a-f]{32}", item["input_xmm0"])
                or not re.fullmatch(r"[0-9a-f]{32}", item["input_xmm1"])):
            raise Stage2Error(f"XORPD handwritten control has invalid fixed identity: {case_id}")
        filename = f"{case_id}.case.json"
        expected_file = f"{XORPD_HAND_CONTROL_DIR.name}/{filename}" \
            if case_id.startswith("s2_42p_") else filename
        if item["file"] != expected_file:
            raise Stage2Error(f"XORPD handwritten control has unexpected source file: {case_id}")
        path = ROOT / "tests/fixtures/x86/stage2" / item["file"]
        if path.is_symlink() or not path.is_file():
            raise Stage2Error(f"XORPD handwritten control must be a regular fixture file: {filename}")
        fixture_raw = path.read_bytes()
        if hashlib.sha256(fixture_raw).hexdigest() != item["sha256"]:
            raise Stage2Error(f"XORPD handwritten control fixture digest mismatch: {filename}")
        fixture = read_json(fixture_raw)
        xmm = fixture.get("initial_state", {}).get("sse", {}).get("xmm")
        if (fixture.get("schema_version") != 2 or fixture.get("case_id") != case_id
                or fixture.get("profile") != XORPD_PROFILE or not isinstance(xmm, list) or len(xmm) != 16
                or xmm[:2] != [item["input_xmm0"], item["input_xmm1"]]):
            raise Stage2Error(f"XORPD handwritten control input identity mismatch: {filename}")
        actual_expected = "".join(f"{a ^ b:02x}" for a, b in zip(
            bytes.fromhex(item["input_xmm0"]), bytes.fromhex(item["input_xmm1"])))
        if actual_expected != item["expected_xmm0"]:
            raise Stage2Error(f"XORPD handwritten control raw-byte oracle mismatch: {case_id}")
        pair = (item["input_xmm0"], item["input_xmm1"])
        if pair in seen_inputs:
            raise Stage2Error(f"XORPD handwritten controls contain duplicate input pair: {case_id}")
        seen_inputs.add(pair)
        frozen_path = frozen / filename
        frozen_path.write_bytes(fixture_raw)
        cases.append((frozen_path, fixture, item["sha256"]))
    (output / "xorpd-hand-controls-manifest.json").write_bytes(raw)
    return cases, value, manifest_sha256


def validate_xorpd_hand_expected(result: dict, expected_xmm0: str, label: str) -> None:
    state = result["snapshot"]["value"]["state"]
    xmm = state.get("sse", {}).get("xmm")
    if not isinstance(xmm, list) or len(xmm) != 16 or xmm[0] != expected_xmm0:
        raise Stage2Error(f"{label}: exact handwritten XORPD XMM0 result mismatch")


def validate_result(result: dict, fixture: dict, digest: str, backend: str,
                    revision: str, compiler: str, label: str, profile: str = PROFILE) -> None:
    expected = {"schema_version": 2, "case_id": fixture["case_id"],
                "input_sha256": digest, "profile": profile, "backend": backend}
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
    machine = result.get("machine")
    if not isinstance(machine, dict) or not machine.get("host_os") or not machine.get("host_arch"):
        raise Stage2Error(f"{label}: machine provenance is unavailable")
    if backend == "native_x86" and (machine.get("host_os") != "linux" or machine.get("host_arch") != "x86_64"):
        raise Stage2Error(f"{label}: physical oracle must report Linux x86_64")
    observed = result.get("snapshot")
    if not isinstance(observed, dict) or observed.get("status") != "available":
        raise Stage2Error(f"{label}: full architectural snapshot unavailable")
    snapshot = observed.get("value")
    if not isinstance(snapshot, dict):
        raise Stage2Error(f"{label}: snapshot value must be an object")
    state = snapshot.get("state")
    if (not isinstance(state, dict) or state.get("mode") != "long64"
            or not isinstance(state.get("gpr"), list) or len(state["gpr"]) != 16
            or not isinstance(state.get("rip"), str) or not isinstance(state.get("rflags"), str)):
        raise Stage2Error(f"{label}: full long-mode GPR set unavailable")
    exception = snapshot.get("exception")
    if not isinstance(exception, dict) or exception.get("status") != "available":
        raise Stage2Error(f"{label}: exception observation unavailable")
    memory = snapshot.get("memory")
    if not isinstance(memory, list) or len(memory) != len(fixture["memory"]):
        raise Stage2Error(f"{label}: incomplete memory snapshot")
    flags = snapshot.get("rflags_defined")
    count = result.get("instructions_executed")
    if (not isinstance(flags, str) or not isinstance(count, dict)
            or count.get("status") not in ("available", "unavailable")):
        raise Stage2Error(f"{label}: flags-definedness or retirement observation is missing")
    if profile == XORPD_PROFILE:
        sse = state.get("sse")
        if (not isinstance(sse, dict) or not isinstance(sse.get("xmm"), list)
                or len(sse["xmm"]) != 16
                or any(not isinstance(reg, str) or not re.fullmatch(r"[0-9a-f]{32}", reg)
                       for reg in sse["xmm"])
                or not isinstance(sse.get("mxcsr"), str)):
            raise Stage2Error(f"{label}: full XMM0–15/MXCSR state is unavailable")
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

    @staticmethod
    def _stream_process(argv, *, cwd, env, timeout, stdout_consumer):
        stdout_parts, stderr_parts = [], []
        stdout_buffer = bytearray()
        started_at = time.monotonic()
        try:
            process = subprocess.Popen(argv, cwd=cwd, env=env, bufsize=0, start_new_session=True,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        except OSError as error:
            raise Stage2Error(f"could not run {argv[0]}: {error}") from error
        deadline = started_at + timeout if timeout is not None else None
        timed_out, drain_deadline = False, None

        def terminate_for_timeout():
            nonlocal timed_out, drain_deadline
            if not timed_out:
                timed_out = True
                drain_deadline = time.monotonic() + POST_TIMEOUT_DRAIN_SECONDS
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except OSError:
                if process.poll() is None:
                    try:
                        process.kill()
                    except ProcessLookupError:
                        pass

        with selectors.DefaultSelector() as selector:
            for name, stream in (("stdout", process.stdout), ("stderr", process.stderr)):
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, name)
            while selector.get_map():
                now = time.monotonic()
                if deadline is not None and now >= deadline:
                    terminate_for_timeout()
                if timed_out:
                    wait = max(0.0, drain_deadline - now)
                    if wait == 0:
                        break
                else:
                    wait = None if deadline is None else max(0.0, deadline - now)
                ready = selector.select(wait)
                if not ready:
                    if timed_out:
                        break
                    if deadline is not None and time.monotonic() >= deadline:
                        terminate_for_timeout()
                    continue
                for key, _ in ready:
                    if deadline is not None and time.monotonic() >= deadline:
                        terminate_for_timeout()
                    stream, name = key.fileobj, key.data
                    try:
                        chunk = os.read(stream.fileno(), 65536)
                    except BlockingIOError:
                        continue
                    if not chunk:
                        selector.unregister(stream)
                        if name == "stdout" and stdout_buffer:
                            stdout_consumer(stdout_buffer.decode("utf-8", errors="replace"), False)
                            stdout_buffer.clear()
                        stream.close()
                    elif name == "stdout":
                        stdout_parts.append(chunk)
                        stdout_buffer.extend(chunk)
                        while b"\n" in stdout_buffer:
                            line, _, tail = stdout_buffer.partition(b"\n")
                            stdout_buffer[:] = tail
                            stdout_consumer(line.decode("utf-8", errors="replace").rstrip("\r"), True)
                    else:
                        stderr_parts.append(chunk)
            for key in list(selector.get_map().values()):
                stream = key.fileobj
                selector.unregister(stream)
                stream.close()

        if not timed_out:
            try:
                if deadline is None:
                    process.wait()
                else:
                    process.wait(timeout=max(0.0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                terminate_for_timeout()
        if timed_out:
            try:
                process.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                terminate_for_timeout()
                try:
                    process.wait(timeout=1.0)
                except subprocess.TimeoutExpired as error:
                    raise Stage2Error(f"could not reap timed-out process {argv[0]}") from error
        stdout = b"".join(stdout_parts).decode("utf-8", errors="replace")
        stderr = b"".join(stderr_parts).decode("utf-8", errors="replace")
        if timed_out:
            expired = subprocess.TimeoutExpired(argv, timeout, output=stdout, stderr=stderr)
            raise Stage2Error(f"could not run {argv[0]}: timed out after {timeout}s") from expired
        return subprocess.CompletedProcess(argv, process.returncode, stdout, stderr)

    def run(self, argv, *, stdout_consumer=None, **kwargs):
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
        completed = False
        try:
            if stdout_consumer is None:
                process = oracle.run(argv, **kwargs)
            else:
                process = self._stream_process(argv, cwd=kwargs.pop("cwd", oracle.ROOT),
                    env=kwargs.pop("env", None), timeout=kwargs.pop("timeout", 60.0),
                    stdout_consumer=stdout_consumer)
                if kwargs:
                    raise Stage2Error(f"unsupported streaming process options: {sorted(kwargs)}")
            stdout, stderr = process.stdout, process.stderr
            record["returncode"] = process.returncode
            completed = True
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


def run_remote_xorpd_batch(target: str, remote_root: str, runner: str, fixture_dir: str,
                           cases: list[tuple[Path, dict, str]], output_dir: Path,
                           *, process_runner, timeout: float = 1800.0) -> dict[str, dict]:
    """Run native cases as flushed JSON lines so timeout evidence is case-granular."""
    batch_code = """import json,os,subprocess,sys
runner,fixture_dir=sys.argv[1:3]
for name in sys.argv[3:]:
 path=os.path.join(fixture_dir,name)
 p=subprocess.run([runner,'--fixture',path,'--json'],text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 print(json.dumps({'filename':name,'returncode':p.returncode,'stdout':p.stdout,'stderr':p.stderr},separators=(',',':')),flush=True)
"""
    argv = ["python3", "-c", batch_code, runner, fixture_dir, *(path.name for path, _, _ in cases)]
    remote_command = f"cd {shlex.quote(remote_root)} && {shlex.join(argv)}"
    expected = {path.name: (path, fixture, digest) for path, fixture, digest in cases}
    parsed, seen, errors = {}, set(), []

    def consume(line, complete):
        if not complete:
            errors.append("truncated native batch record")
            return
        try:
            record = read_json(line)
        except (Stage2Error, ValueError) as error:
            errors.append(f"malformed native batch line: {error}")
            return
        name = record.get("filename") if isinstance(record, dict) else None
        if not isinstance(name, str) or name not in expected or name in seen:
            errors.append(f"unexpected or duplicate native batch case: {name!r}")
            return
        seen.add(name)
        _, fixture, _ = expected[name]
        case_dir = output_dir / fixture["case_id"]
        case_dir.mkdir(exist_ok=True)
        stdout = record.get("stdout")
        stderr = record.get("stderr")
        returncode = record.get("returncode")
        result_path = case_dir / "native_x86.json"
        result_path.write_text(stdout if isinstance(stdout, str) else "")
        (case_dir / "native_x86.stderr.log").write_text(stderr if isinstance(stderr, str) else "")
        (case_dir / "native_x86.process.json").write_text(json.dumps({"returncode": returncode}))
        if not isinstance(stdout, str) or not isinstance(stderr, str):
            errors.append(f"{name}: native batch record omitted stdout or stderr")
            return
        if type(returncode) is not int or returncode != 0:
            errors.append(f"{name}: native execution exit {returncode}")
            return
        try:
            parsed[name] = oracle.parse_result(stdout, result_path)
        except Stage2Error as error:
            errors.append(f"{name}: malformed native result: {error}")

    process = process_runner(oracle.ssh_argv(target, remote_command), timeout=timeout,
                             stdout_consumer=consume)
    (output_dir / "native-batch.stdout.jsonl").write_text(process.stdout)
    (output_dir / "native-batch.stderr.log").write_text(process.stderr)
    oracle.require_success(process, "remote native batch")
    if errors:
        raise Stage2Error("remote native execution failed: " + "; ".join(errors))
    if seen != set(expected):
        missing = sorted(set(expected) - seen)
        raise Stage2Error("remote native batch omitted cases: " + ", ".join(missing[:8]))
    return parsed


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
    parser.add_argument("--suite", choices=("integer", "xorpd-native-corpus"), default="integer",
                        help="strict selected suite; integer remains the default")
    parser.add_argument("--host", default="192.168.1.14")
    parser.add_argument("--user", default="senya")
    parser.add_argument("--remote-root", default="/home/senya/projects/open_desktop-stage1")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--output-root", default="work/stage2-oracle")
    parser.add_argument("--stage1-manifest", help="optional passed 1010-case v1 oracle manifest from the same sources")
    args = parser.parse_args(argv)
    if args.repeats < 2:
        parser.error("--repeats must be at least 2")
    if args.suite == "xorpd-native-corpus" and args.repeats != 2:
        parser.error("the strict XORPD corpus suite requires exactly two repeats")
    if args.suite == "xorpd-native-corpus" and args.cases:
        parser.error("the strict XORPD corpus suite uses the recorded S2-42a corpus; positional cases are not accepted")
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
    profile = XORPD_PROFILE if args.suite == "xorpd-native-corpus" else PROFILE
    manifest = {"run_id": run_id, "status": "running", "suite": args.suite, "profile": profile,
                "scope": ("Six pinned handwritten XORPD controls followed by the recorded 1024-case S2-42a corpus: strict interpreter and physical Linux x86 native, twice; no v2 JIT"
                          if args.suite == "xorpd-native-corpus" else
                          "Selected v2 CPU fixtures: interpreter vs physical Linux x86 native; no v2 JIT"),
                "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "target": f"{args.user}@{args.host}", "remote_root": args.remote_root,
                "repeats": args.repeats, "commands": [], "inputs": [], "fixtures": [],
                "legacy_regression": {"status": "not_provided", "required_for_full_s2_09": True}}
    evidence = Evidence(output, manifest)
    evidence.save()
    try:
        corpus_manifest = None
        hand_expected = {}
        if args.suite == "xorpd-native-corpus":
            generated_cases, corpus_manifest, corpus_manifest_digest = prepare_xorpd_corpus(
                ROOT / XORPD_CORPUS_ROOT, output)
            hand_cases, hand_manifest, hand_manifest_digest = prepare_xorpd_hand_controls(output)
            cases = hand_cases + generated_cases
            hand_expected = {item["case_id"]: item["expected_xmm0"] for item in hand_manifest["controls"]}
            manifest["corpus_manifest"] = {"path": str(XORPD_CORPUS_ROOT / "manifest.json"),
                "sha256": corpus_manifest_digest, "case_count": len(generated_cases),
                "corpus_digest": corpus_manifest["corpus_digest"], "physical_native": "not_run"}
            manifest["hand_control_manifest"] = {"path": str(XORPD_HAND_MANIFEST),
                "sha256": hand_manifest_digest, "control_set_digest": hand_manifest["control_set_digest"],
                "case_count": len(hand_cases), "physical_native": "not_run"}
            manifest["case_counts"] = {"generated": len(generated_cases),
                "handwritten": len(hand_cases), "total": len(cases)}
        else:
            cases = prepare_cases(args.cases or list(DEFAULT_CASES), output)
        manifest["inputs"] = [{"case_id": fixture["case_id"], "filename": path.name,
            "fixture_path": str(path.relative_to(ROOT)), "fixture_sha256": digest,
            "seed": fixture.get("seed"), "schema_version": 2, "profile": profile,
            **({"group": "handwritten" if fixture["case_id"] in hand_expected else "generated"}
               if args.suite == "xorpd-native-corpus" else {})}
            for path, fixture, digest in cases]
        manifest["reproduce_argv"] = [sys.executable, "tests/cpu/stage2_oracle.py", "--host", args.host,
            "--user", args.user, "--remote-root", args.remote_root, "--repeats", str(args.repeats)]
        if args.stage1_manifest:
            manifest["reproduce_argv"].extend(["--stage1-manifest", args.stage1_manifest])
        if args.suite == "xorpd-native-corpus":
            manifest["reproduce_argv"].extend(["--suite", args.suite])
        else:
            manifest["reproduce_argv"].extend(str(path.relative_to(ROOT)) for path, _, _ in cases)
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
        runner = ROOT / "target/debug/od-runner"
        if args.suite == "xorpd-native-corpus":
            oracle.require_success(evidence.run([str(runner), "--verify-native-xorpd-corpus"], env=env,
                timeout=300.0), "strict XORPD corpus verifier")
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
        remote_runner = posixpath.join(args.remote_root, "target/debug/od-x86-runner")
        for repeat in range(1, args.repeats + 1):
            repeat_dir = output / f"repeat-{repeat:02d}"
            repeat_dir.mkdir()
            if args.suite == "xorpd-native-corpus":
                manifest["corpus_manifest"]["physical_native"] = "running"
                manifest["hand_control_manifest"]["physical_native"] = "running"
                evidence.save()
            batch_runner = run_remote_xorpd_batch if args.suite == "xorpd-native-corpus" else oracle.run_remote_batch
            natives = batch_runner(target, args.remote_root, remote_runner, remote_fixtures,
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
                validate_result(native, fixture, digest, "native_x86", revision, remote_rustc,
                                "native result", profile)
                if fixture["case_id"] in hand_expected:
                    validate_xorpd_hand_expected(native, hand_expected[fixture["case_id"]], "native result")
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
                validate_result(local, fixture, digest, "interpreter", revision, local_info["rustc"],
                                "interpreter result", profile)
                if fixture["case_id"] in hand_expected:
                    validate_xorpd_hand_expected(local, hand_expected[fixture["case_id"]], "interpreter result")
                compare(evidence, runner, local_path, native_path, path, fixture, digest,
                        directory / "interpreter-vs-native.json", env)
                if repeat > 1:
                    for engine in ("interpreter", "native_x86"):
                        previous = output / f"repeat-{repeat - 1:02d}" / fixture["case_id"] / f"{engine}.json"
                        compare(evidence, runner, previous, directory / f"{engine}.json", path, fixture, digest,
                                directory / f"{engine}-repeat-compare.json", env)
                manifest["fixtures"].append({"repeat": repeat, "case_id": fixture["case_id"],
                    "fixture_sha256": digest, "seed": fixture.get("seed"), "schema_version": 2,
                    **({"group": "handwritten" if fixture["case_id"] in hand_expected else "generated"}
                       if args.suite == "xorpd-native-corpus" else {}),
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
        if args.suite == "xorpd-native-corpus":
            manifest["corpus_manifest"]["physical_native"] = "executed_and_matched"
            manifest["hand_control_manifest"]["physical_native"] = "executed_and_matched"
        manifest["status"] = "passed"
        manifest["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        evidence.save()
        print(f"manifest: {output / 'manifest.json'}")
        if args.suite == "xorpd-native-corpus":
            counts = manifest["case_counts"]
            print(f"cases: generated={counts['generated']} handwritten={counts['handwritten']} total={counts['total']}")
        return 0
    except (Stage2Error, OSError, KeyError, TypeError, ValueError) as error:
        if (args.suite == "xorpd-native-corpus" and "corpus_manifest" in manifest
                and manifest["corpus_manifest"]["physical_native"] == "running"):
            manifest["corpus_manifest"]["physical_native"] = "failed"
            manifest["hand_control_manifest"]["physical_native"] = "failed"
        manifest["status"], manifest["error"] = "failed", str(error)
        manifest["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        evidence.save()
        print(f"stage2 oracle failed; manifest: {output / 'manifest.json'}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
