#!/usr/bin/env python3
"""S4-34a: build the freestanding KVM method probe on the Mac and run it on the mini.

The image is assembled locally (clang + ld.lld + objcopy), copied together with
tools/kvm-runner/kvm_runner.c to the Linux host, the runner is compiled there with
gcc, the image is executed under /dev/kvm (VT-x, no own MMU) and the one-line JSON
result is validated and written as evidence.
"""

from __future__ import annotations

import argparse
import datetime as dt
import glob
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = ROOT / "tests/fixtures/guest/stage4/kvm_method"
RUNNER_SOURCE = ROOT / "tools/kvm-runner/kvm_runner.c"
DEFAULT_OUTPUT = ROOT / "docs/reports/data/stage4/s4-34a-kvm-method.json"
DEFAULT_HOST = "senya@192.168.1.14"
DEFAULT_REMOTE_DIR = "/home/senya/od-kvm"
GCC_FLAGS = ["-O2", "-Wall", "-Wextra", "-Werror", "-std=c11"]

IMAGE_GPA = 0x1000
EXPECTED_UART = "OK\n"
EXPECTED_ENTRY = {
    "rip": 0x1000,
    "cr3": 0x2000,
    "cr4": 0x20,
    "efer": 0x500,
    "cs_selector": 0x8,
    "cr0": 0x80000033,
    "cs_l": 1,
    "cs_db": 0,
    "cs_dpl": 0,
    "rsp": 0x8000,
    "ss_selector": 0x10,
}
EXPECTED_UART_PORT = "0x3f8"
EXPECTED_EXITS = [
    {"kind": "io", "port": EXPECTED_UART_PORT, "size": 1, "dir": "out", "data": "0x4f"},
    {"kind": "io", "port": EXPECTED_UART_PORT, "size": 1, "dir": "out", "data": "0x4b"},
    {"kind": "io", "port": EXPECTED_UART_PORT, "size": 1, "dir": "out", "data": "0x0a"},
    {"kind": "hlt"},
]
EXPECTED_KVM_API = 12

# case name -> (clang defines, remote image name, expectation)
CASES: dict[str, dict[str, Any]] = {
    "main": {
        "defines": [],
        "expect_exit": "hlt",
        "expect_uart": EXPECTED_UART,
        "injection": False,
        "description": "OK\\n then HLT",
    },
    "no_hlt": {
        "defines": ["INJECT_NO_HLT"],
        "expect_exit": "budget",
        "expect_uart": EXPECTED_UART,
        "injection": True,
        "description": "injection (a): OK\\n then jmp $, no HLT",
    },
    "wrong_str": {
        "defines": ["INJECT_WRONG_STR"],
        "expect_exit": "hlt",
        "expect_uart": "NO\n",
        "injection": True,
        "description": "injection (b): NO\\n then HLT",
    },
}


class BuildError(Exception):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256(path.read_bytes())


# --------------------------------------------------------------------------
# Pure evaluation (unit-tested without hardware)
# --------------------------------------------------------------------------


def _num(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    return None


def normalize_exits(exits: Any) -> Any:
    """Canonical form of the exit list: port/data hex strings become integers.

    The runner prints data with %llx (0xa for a newline), so the comparison must not
    depend on zero padding. Anything malformed is returned unchanged and fails the check.
    """
    if not isinstance(exits, list):
        return exits
    out: list[Any] = []
    for exit_rec in exits:
        if not isinstance(exit_rec, dict):
            return exits
        rec = dict(exit_rec)
        for key in ("port", "data"):
            if key in rec:
                value = rec[key]
                try:
                    rec[key] = int(value, 16) if isinstance(value, str) else value
                except ValueError:
                    return exits
        out.append(rec)
    return out


def evaluate_main_checks(
    result: dict[str, Any],
    *,
    local_image_sha: str,
    remote_image_sha: str,
) -> list[dict[str, Any]]:
    """Acceptance checks of the primary run (S4-34a plan, item 3)."""
    entry = result.get("entry") if isinstance(result.get("entry"), dict) else {}
    checks: list[dict[str, Any]] = []

    def add(name: str, expected: Any, actual: Any) -> None:
        checks.append({"name": name, "expected": expected, "actual": actual, "ok": expected == actual})

    add("exit", "hlt", result.get("exit"))
    add("uart_text", EXPECTED_UART, result.get("uart_text"))
    add("entry.rip", EXPECTED_ENTRY["rip"], _num(entry.get("rip")))
    add("entry.cr3", EXPECTED_ENTRY["cr3"], _num(entry.get("cr3")))
    add("entry.cr4", EXPECTED_ENTRY["cr4"], _num(entry.get("cr4")))
    add("entry.efer", EXPECTED_ENTRY["efer"], _num(entry.get("efer")))
    for field in ("cs_selector", "cr0", "cs_l", "cs_db", "cs_dpl", "rsp", "ss_selector"):
        add(f"entry.{field}", EXPECTED_ENTRY[field], _num(entry.get(field)))
    expected_exits = normalize_exits(EXPECTED_EXITS)
    actual_exits = normalize_exits(result.get("exits"))
    checks.append(
        {
            "name": "exits_sequence",
            "expected": EXPECTED_EXITS,
            "actual": result.get("exits"),
            "ok": expected_exits == actual_exits,
        }
    )
    add("exits_dropped", 0, _num(result.get("exits_dropped")))
    add("kvm_api", EXPECTED_KVM_API, _num(result.get("kvm_api")))
    add("image_sha256_matches_local", local_image_sha, remote_image_sha)
    return checks


def classify_case(
    name: str,
    result: dict[str, Any],
    *,
    local_image_sha: str,
    remote_image_sha: str,
) -> dict[str, Any]:
    """Return per-case verdict.

    status is accepted only when all main checks pass. For injections the
    verdict is expected to be rejected and `distinguished` says whether the
    result differs from the accepted one in the intended way (exit/uart).
    """
    spec = CASES[name]
    checks = evaluate_main_checks(result, local_image_sha=local_image_sha, remote_image_sha=remote_image_sha)
    status = "accepted" if all(c["ok"] for c in checks) else "rejected"
    verdict: dict[str, Any] = {"status": status, "checks": checks}
    if spec["injection"]:
        intended = result.get("exit") == spec["expect_exit"] and result.get("uart_text") == spec["expect_uart"]
        verdict["expected_status"] = "rejected"
        verdict["distinguished"] = bool(status == "rejected" and intended)
        verdict["expect_exit"] = spec["expect_exit"]
        verdict["expect_uart_text"] = spec["expect_uart"]
    else:
        verdict["expected_status"] = "accepted"
        verdict["distinguished"] = status == "accepted"
    return verdict


def overall_status(verdicts: dict[str, dict[str, Any]]) -> str:
    if "main" not in verdicts:
        return "rejected"
    if verdicts["main"]["status"] != "accepted":
        return "rejected"
    for name, verdict in verdicts.items():
        if name != "main" and not verdict.get("distinguished"):
            return "rejected"
    return "accepted"


def parse_runner_stdout(stdout: str) -> dict[str, Any]:
    """The runner must print exactly one JSON object line and nothing else."""
    lines = [line for line in stdout.splitlines() if line.strip()]
    if len(lines) != 1:
        raise BuildError(f"runner must print exactly one JSON line, got {len(lines)}")
    try:
        value = json.loads(lines[0])
    except json.JSONDecodeError as exc:
        raise BuildError(f"runner output is not JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise BuildError("runner output is not a JSON object")
    return value


# --------------------------------------------------------------------------
# Local image build
# --------------------------------------------------------------------------


def run_local(command: list[str], *, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(command, env=env, check=False, capture_output=True, text=True)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise BuildError(f"command failed ({result.returncode}): {command!r}\n{detail}")
    return (result.stdout + result.stderr).strip()


def find_tool(env_name: str, names: list[str], rust_relative: list[str]) -> Path:
    explicit = os.environ.get(env_name)
    if explicit:
        path = Path(explicit).expanduser()
        if path.is_file() and os.access(path, os.X_OK):
            return path.resolve()
        raise BuildError(f"{env_name} is not an executable file: {path}")
    for name in names:
        found = shutil.which(name)
        if found:
            return Path(found).resolve()
    sysroots: list[str] = []
    rustc = shutil.which("rustc")
    if rustc:
        probe = subprocess.run([rustc, "--print", "sysroot"], capture_output=True, text=True, check=False)
        if probe.returncode == 0 and probe.stdout.strip():
            sysroots.append(probe.stdout.strip())
    sysroots += sorted(glob.glob(str(Path.home() / ".rustup/toolchains/*")), reverse=True)
    for sysroot in sysroots:
        for rel in rust_relative:
            for candidate in glob.glob(str(Path(sysroot) / "lib/rustlib/*/bin" / rel)):
                path = Path(candidate)
                if path.is_file() and os.access(path, os.X_OK):
                    return path.resolve()
    raise BuildError(f"could not find {names[0]} (set {env_name})")


def tool_env(*tools: Path) -> dict[str, str]:
    """Rust-shipped LLVM tools on macOS need libLLVM.dylib from the sysroot lib dir."""
    env = os.environ.copy()
    libs: list[str] = []
    for tool in tools:
        for parent in tool.parents:
            if parent.name == "toolchains" or parent == parent.parent:
                break
            lib = parent / "lib"
            if lib.is_dir() and glob.glob(str(lib / "libLLVM*.dylib")):
                libs.append(str(lib))
                break
    if libs:
        inherited = env.get("DYLD_LIBRARY_PATH", "")
        parts = [*libs, *(p for p in inherited.split(os.pathsep) if p)]
        env["DYLD_LIBRARY_PATH"] = os.pathsep.join(dict.fromkeys(parts))
    return env


def tool_version(path: Path, flag: str, env: dict[str, str]) -> str:
    """First line of `<tool> --version` (empty string when the tool does not report one)."""
    result = subprocess.run([str(path), flag], env=env, check=False, capture_output=True, text=True)
    lines = [line for line in (result.stdout + result.stderr).splitlines() if line.strip()]
    return lines[0].strip() if lines else ""


def build_image(case: str, workdir: Path) -> tuple[bytes, dict[str, str]]:
    """Assemble, link and convert one variant to a raw image linked at 0x1000."""
    defines = CASES[case]["defines"]
    clang = find_tool("STAGE4_CLANG", ["clang"], [])
    ld = find_tool("STAGE4_LD_LLD", ["ld.lld"], ["gcc-ld/ld.lld"])
    objcopy = find_tool("STAGE4_OBJCOPY", ["llvm-objcopy"], ["llvm-objcopy", "rust-objcopy"])
    workdir.mkdir(parents=True, exist_ok=True)
    obj = workdir / f"{case}.o"
    elf = workdir / f"{case}.elf"
    raw = workdir / f"{case}.bin"
    cmd = [
        str(clang),
        "--target=x86_64-unknown-none-elf",
        "-c",
        "-ffreestanding",
        "-nostdlib",
        *[f"-D{d}" for d in defines],
        str(FIXTURE_DIR / "entry.S"),
        "-o",
        str(obj),
    ]
    run_local(cmd)
    env = tool_env(ld, objcopy)
    run_local([str(ld), "-T", str(FIXTURE_DIR / "layout.ld"), "-o", str(elf), str(obj)], env=env)
    run_local([str(objcopy), "-O", "binary", str(elf), str(raw)], env=env)
    data = raw.read_bytes()
    if not 0 < len(data) <= 0x1000:
        raise BuildError(f"image size {len(data)} outside 1..0x1000")
    home = str(Path.home())
    tools: dict[str, Any] = {}
    for name, path, flag in (("clang", clang, "--version"), ("ld", ld, "--version"), ("objcopy", objcopy, "--version")):
        tools[name] = {"path": str(path).replace(home, "~", 1), "version": tool_version(path, flag, env)}
    return data, tools


# --------------------------------------------------------------------------
# Remote execution
# --------------------------------------------------------------------------

Runner = Callable[[list[str], int], "subprocess.CompletedProcess[str]"]


def default_runner(command: list[str], timeout: int) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(command, check=False, capture_output=True, text=True, timeout=timeout)


class Remote:
    def __init__(self, host: str, remote_dir: str, runner: Runner = default_runner) -> None:
        self.host = host
        self.remote_dir = remote_dir
        self.runner = runner

    def ssh_command(self, remote_command: str) -> list[str]:
        return ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", self.host, remote_command]

    def ssh(self, remote_command: str, timeout: int = 60) -> "subprocess.CompletedProcess[str]":
        return self.runner(self.ssh_command(remote_command), timeout)

    def ssh_ok(self, remote_command: str, timeout: int = 60) -> str:
        result = self.ssh(remote_command, timeout)
        if result.returncode != 0:
            raise BuildError(
                f"ssh {remote_command!r} failed ({result.returncode}): {(result.stderr or result.stdout).strip()}"
            )
        return result.stdout

    def scp(self, local: Path, remote_name: str) -> None:
        dest = f"{self.host}:{self.remote_dir}/{remote_name}"
        result = self.runner(["scp", "-q", "-o", "BatchMode=yes", str(local), dest], 60)
        if result.returncode != 0:
            raise BuildError(f"scp {local} -> {dest} failed: {(result.stderr or result.stdout).strip()}")

    def remote_sha(self, remote_name: str) -> str:
        path = shlex.quote(f"{self.remote_dir}/{remote_name}")
        out = self.ssh_ok(f"sha256sum {path}")
        return out.split()[0]

    def compile_runner(self) -> str:
        d = shlex.quote(self.remote_dir)
        flags = " ".join(GCC_FLAGS)
        self.ssh_ok(f"cd {d} && gcc {flags} -o kvm_runner kvm_runner.c", timeout=120)
        return self.ssh_ok("gcc --version | head -1").strip()

    def run_image(self, remote_name: str, budget: int = 1000, deadline_ms: int = 2000) -> dict[str, Any]:
        d = shlex.quote(self.remote_dir)
        cmd = f"cd {d} && ./kvm_runner {shlex.quote(remote_name)} --budget {budget} --deadline-ms {deadline_ms}"
        result = self.ssh(cmd, timeout=60)
        if result.returncode not in (0,):
            raise BuildError(
                f"kvm_runner failed ({result.returncode}): {(result.stdout or '').strip()} {(result.stderr or '').strip()}"
            )
        return parse_runner_stdout(result.stdout)


# --------------------------------------------------------------------------
# Orchestration
# --------------------------------------------------------------------------


def sources_hashes() -> dict[str, str]:
    files = {
        "tests/fixtures/guest/stage4/kvm_method/entry.S": FIXTURE_DIR / "entry.S",
        "tests/fixtures/guest/stage4/kvm_method/layout.ld": FIXTURE_DIR / "layout.ld",
        "tools/kvm-runner/kvm_runner.c": RUNNER_SOURCE,
        "tools/kvm-runner/Makefile": ROOT / "tools/kvm-runner/Makefile",
        "tools/build/build_stage4_kvm_method.py": Path(__file__).resolve(),
    }
    return {name: sha256_file(path) for name, path in files.items()}


def orchestrate(
    remote: Remote,
    cases: list[str],
    workdir: Path,
    *,
    image_builder: Callable[[str, Path], tuple[bytes, dict[str, str]]] = build_image,
) -> dict[str, Any]:
    images: dict[str, bytes] = {}
    tools: dict[str, Any] = {}
    for case in cases:
        images[case], tools = image_builder(case, workdir)

    remote.ssh_ok(f"mkdir -p {shlex.quote(remote.remote_dir)}")
    remote.scp(RUNNER_SOURCE, "kvm_runner.c")
    runner_local_sha = sha256_file(RUNNER_SOURCE)
    runner_remote_sha = remote.remote_sha("kvm_runner.c")
    gcc_version = remote.compile_runner()
    runner_binary_sha = remote.remote_sha("kvm_runner")

    runs: dict[str, Any] = {}
    identity: dict[str, Any] = {}
    verdicts: dict[str, dict[str, Any]] = {}
    for case in cases:
        name = f"kvm_method_{case}.bin"
        local_path = workdir / f"{case}.bin"
        local_path.write_bytes(images[case])
        local_sha = sha256(images[case])
        remote.scp(local_path, name)
        remote_sha = remote.remote_sha(name)
        result = remote.run_image(name)
        if not identity:
            identity = {
                k: result.get(k) for k in ("hostname", "uname", "cpu_model", "vmx", "kvm_api")
            }
        verdict = classify_case(case, result, local_image_sha=local_sha, remote_image_sha=remote_sha)
        verdicts[case] = verdict
        runs[case] = {
            "description": CASES[case]["description"],
            "image_name": name,
            "image_size": len(images[case]),
            "image_sha256_local": local_sha,
            "image_sha256_remote": remote_sha,
            "run_count": result.get("run_count"),
            "intr_count": result.get("intr_count"),
            "exit": result.get("exit"),
            "stop_reason": result.get("stop_reason"),
            "uart_hex": result.get("uart_hex"),
            "uart_text": result.get("uart_text"),
            "entry": result.get("entry"),
            "exits": result.get("exits"),
            "exits_dropped": result.get("exits_dropped"),
            "verdict": verdict,
        }

    return {
        "runner_hashes": {
            "kvm_runner_c_local": runner_local_sha,
            "kvm_runner_c_remote": runner_remote_sha,
            "kvm_runner_binary_remote": runner_binary_sha,
            "gcc": gcc_version,
            "gcc_flags": GCC_FLAGS,
        },
        "tools": tools,
        "runs": runs,
        "verdicts": verdicts,
        "status": overall_status(verdicts),
        "identity": identity,
    }


def build_evidence(remote: Remote, cases: list[str], workdir: Path) -> dict[str, Any]:
    body = orchestrate(remote, cases, workdir)
    main = body["runs"].get("main", {})
    return {
        "slice": "S4-34a",
        "method": "kvm_host_runner",
        "environment": "real Intel VT-x via /dev/kvm on the Mac mini (Linux); no own MMU/CPU involved",
        "boundary": "single image, no interrupts, not S4-34 acceptance, no Linux, no reboot",
        "generated_at": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "method_status": body["status"],
        "s4_34": "not_accepted",
        "hardware_identity": body["identity"],
        "remote": {"host": remote.host, "dir": remote.remote_dir},
        "artifact_hashes": {
            "images": {c: {"sha256": body["runs"][c]["image_sha256_local"], "size": body["runs"][c]["image_size"]} for c in cases},
            "sources": sources_hashes(),
            "runner": body["runner_hashes"],
        },
        "tools": body["tools"],
        "entry_state": main.get("entry"),
        "exits": main.get("exits"),
        "uart": {"hex": main.get("uart_hex"), "text": main.get("uart_text")},
        "runs": body["runs"],
        "expected": {
            "exit": "hlt",
            "uart_text": EXPECTED_UART,
            "entry": EXPECTED_ENTRY,
            "exits": EXPECTED_EXITS,
            "kvm_api": EXPECTED_KVM_API,
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default=DEFAULT_HOST)
    parser.add_argument("--remote-dir", default=DEFAULT_REMOTE_DIR)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--workdir", type=Path, default=ROOT / "work/stage4/kvm_method")
    parser.add_argument(
        "--cases",
        nargs="+",
        choices=list(CASES),
        default=list(CASES),
        help="main run is always executed first; injections are run one per case",
    )
    args = parser.parse_args(argv)
    cases = ["main"] + [c for c in args.cases if c != "main"]
    try:
        evidence = build_evidence(Remote(args.host, args.remote_dir), cases, args.workdir)
    except (BuildError, subprocess.TimeoutExpired, OSError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"method_status={evidence['method_status']} (s4_34={evidence['s4_34']}): wrote {args.output}")
    for case in cases:
        run = evidence["runs"][case]
        print(
            f"  {case}: exit={run['exit']} uart={run['uart_text']!r} run_count={run['run_count']} "
            f"intr_count={run['intr_count']} "
            f"verdict={run['verdict']['status']}"
        )
    return 0 if evidence["method_status"] == "accepted" else 1


if __name__ == "__main__":
    raise SystemExit(main())
